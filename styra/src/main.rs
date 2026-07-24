//! Styra's terminal client: CLI, terminal lifecycle, and the event loop that
//! drives the application through Styra's JSON Unix-socket API.

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{Stdout, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod app;
mod ui;

use app::{App, Focus, Status, View};
use styra_server::api::{CreateSession, SessionInfo};
use styra_server::{Client, LogEntry, SessionUpdate};

/// Run an interactive, isolated agent session in a terminal interface.
#[derive(Parser)]
#[command(name = "styra", about, version)]
struct Cli {
    /// Styra server Unix socket (default: $XDG_RUNTIME_DIR/styra/styra.sock).
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Agent profile to launch a live session with. Not used with `--view`:
    /// a viewed session carries its own recorded profile and protocol.
    #[arg(long, default_value = "codex")]
    profile: String,
    /// Host directory mounted writable as the agent workspace (default: cwd).
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Permit agent networking (profiles may default this on).
    #[arg(long)]
    network: bool,
    /// Open a captured journal read-only instead of launching an agent: with
    /// a path, that session directly; bare (no path), a picker to browse and
    /// choose one from the server's store.
    #[arg(long, num_args = 0..=1, value_name = "SESSION")]
    view: Option<Option<PathBuf>>,
    /// Optional first message, sent to seed the opening turn.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = match &cli.socket {
        Some(path) => path.clone(),
        None => styra_server::paths::default_socket()?,
    };
    let client = Client::new(&socket);
    client.health().with_context(|| {
        format!(
            "Styra server is unavailable at {}; start `styra-server` in this project",
            socket.display()
        )
    })?;

    // Bare `--view` (no path) needs an interactive terminal to browse
    // sessions in, so it is opened early only in that case; the other paths
    // below still report setup failures before taking over the terminal,
    // and the terminal the picker opened is reused below rather than torn
    // down and reopened.
    let mut terminal: Option<Terminal<CrosstermBackend<Stdout>>> = None;
    let view_target: Option<PathBuf> = match &cli.view {
        Some(Some(path)) => Some(path.clone()),
        Some(None) => {
            let sessions = client.list_sessions()?;
            if sessions.is_empty() {
                println!("No sessions found by the Styra server");
                return Ok(());
            }
            let mut term = setup_terminal()?;
            match run_picker(&mut term, &sessions) {
                Ok(Some(id)) => {
                    terminal = Some(term);
                    Some(PathBuf::from(id))
                }
                Ok(None) => {
                    restore_terminal(&mut term)?;
                    return Ok(());
                }
                Err(error) => {
                    restore_terminal(&mut term)?;
                    return Err(error);
                }
            }
        }
        None => None,
    };

    // Build the application and, unless viewing or awaiting the operator's
    // first message, a live session up front so a setup failure is reported
    // plainly before the terminal is taken over.
    let mut app;
    let mut live: Live;

    if let Some(view) = &view_target {
        let id = session_id_from_target(view)?;
        let stored = client.stored_session(&id)?;
        app = App::new(
            stored.summary.profile.clone().unwrap_or_else(|| "unknown".into()),
            stored.summary.id,
        );
        for event in stored.events {
            // Skip carried-but-viewless traffic (e.g. app-server control
            // lines), matching what a live session shows; it stays available
            // in the raw view below.
            if !matches!(event, styra_server::event::AgentEvent::Unknown { .. }) {
                app.push_event(event);
            }
        }
        for line in stored.raw {
            app.push_raw(line);
        }
        // A replayed session has no live agent to end; mark it stopped.
        app.on_ended(styra_server::SessionEnd { exit_code: None, error: None });
        live = Live::Viewing;
    } else {
        let prompt = cli.prompt.join(" ");
        let seed = (!prompt.trim().is_empty()).then_some(prompt.as_str());
        match seed {
            // A trailing prompt is input the operator already gave (as a CLI
            // argument), so it is fine to launch immediately.
            Some(seed) => {
                let (new_app, info) = launch_live_session(&client, &cli, Some(seed))?;
                app = new_app;
                live = Live::Running { session_id: info.id, cursor: 0 };
            }
            // No seed: nothing has been said to an agent yet, so nothing is
            // launched yet either. The event loop spawns the session the
            // moment the operator submits their first message.
            None => {
                app = App::pending(cli.profile.clone());
                live = Live::Pending;
            }
        }
    }

    let mut terminal = match terminal {
        Some(terminal) => terminal,
        None => setup_terminal()?,
    };

    // Runs until the operator quits; a session switch stops the outgoing
    // session, prefills the message box with the picked session's transcript,
    // and loops back into run() pending a fresh launch, all inside the same
    // terminal.
    let result = loop {
        let outcome = match run(&mut terminal, &mut app, &client, &cli, &mut live) {
            Ok(outcome) => outcome,
            Err(error) => break Err(error),
        };
        match outcome {
            RunOutcome::Quit => break Ok(()),
            RunOutcome::Switch(seed_id) => {
                if let Live::Running { session_id, .. } =
                    std::mem::replace(&mut live, Live::Pending)
                {
                    client.stop_session(&session_id).ok();
                }
                match client.transcript(&seed_id) {
                    Ok(transcript) => {
                        app = App::pending(cli.profile.clone());
                        app.set_input(transcript);
                    }
                    Err(error) => {
                        app.push_log(LogEntry::error(format!(
                            "could not switch session: {error:#}"
                        )));
                    }
                }
            }
        }
    };

    restore_terminal(&mut terminal)?;
    if let Live::Running { session_id, .. } = live {
        client.stop_session(&session_id).ok();
    }
    result
}

fn create_session(client: &Client, cli: &Cli, seed: Option<&str>) -> Result<SessionInfo> {
    let workspace = resolve_workspace(cli.workspace.as_deref())?;
    client.create_session(&CreateSession {
        profile: cli.profile.clone(),
        workspace,
        network: cli.network,
        message: seed.map(str::to_owned),
    })
}

/// Spawn a session and wrap it in a fresh `App`. Used for the CLI's trailing
/// prompt on first launch, the only case where the agent starts before the
/// event loop takes over.
fn launch_live_session(
    client: &Client,
    cli: &Cli,
    seed: Option<&str>,
) -> Result<(App, SessionInfo)> {
    let info = create_session(client, cli, seed)?;
    let mut app = App::new(info.profile.clone(), info.id.clone());
    app.set_workspace_root(info.workspace.clone());
    app.set_driva_options(info.driva.clone());
    app.push_log(LogEntry::info(format!("journal: {}", info.journal_path.display())));
    Ok((app, info))
}

fn session_id_from_target(target: &Path) -> Result<String> {
    target
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .with_context(|| format!("invalid session target {}", target.display()))
}

/// The session picker loop: j/k or arrows to move, Enter to choose a
/// session, Esc or q to back out without picking one.
fn run_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    sessions: &[styra_server::SessionSummary],
) -> Result<Option<String>> {
    let mut selected = 0usize;
    loop {
        terminal.draw(|frame| ui::render_picker(frame, sessions, selected))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(sessions.len() - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter => return Ok(Some(sessions[selected].id.clone())),
            _ => {}
        }
    }
}

/// What the interactive loop returned control to `main` for.
enum RunOutcome {
    /// The operator quit.
    Quit,
    /// The operator picked a stored session to switch to.
    Switch(String),
}

/// The live-agent side of the interactive loop: no process yet (awaiting the
/// operator's first message), a spawned agent, or a replayed journal with no
/// live agent to send to.
enum Live {
    /// Nothing has been launched; the event loop spawns the session itself
    /// the moment the operator submits a message from `Focus::Input`.
    Pending,
    /// A server-owned agent process, addressed by id. `cursor` makes polling
    /// incremental and preserves the server's update order.
    Running { session_id: String, cursor: u64 },
    /// A replayed journal (`--view`); there is no live agent to launch.
    Viewing,
}

/// The event loop: apply pending session updates, render, and handle input
/// until the operator quits or asks to switch sessions. `cli` and `layout`
/// are only needed to spawn a session lazily out of `Live::Pending` once the
/// operator writes a first message.
fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    client: &Client,
    cli: &Cli,
    live: &mut Live,
) -> Result<RunOutcome> {
    let mut pending_fold = false;
    loop {
        let mut disconnected = false;
        if let Live::Running { session_id, cursor } = live {
            match client.updates(session_id, *cursor) {
                Ok(batch) => {
                    *cursor = batch.next;
                    for sequenced in batch.updates {
                        match sequenced.update {
                    SessionUpdate::Event(event) => app.push_event(event),
                    SessionUpdate::Raw(line) => app.push_raw(line),
                    SessionUpdate::Log(entry) => app.push_log(entry),
                    SessionUpdate::Ended(end) => app.on_ended(end),
                        }
                    }
                }
                Err(error) => {
                    app.push_log(LogEntry::error(format!("update poll failed: {error:#}")));
                    app.on_ended(styra_server::SessionEnd {
                        exit_code: None,
                        error: Some(error.to_string()),
                    });
                    disconnected = true;
                }
            }
        }
        if disconnected {
            *live = Live::Viewing;
        }

        terminal.draw(|frame| ui::render(frame, app))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match app.focus {
            Focus::List => handle_list_key(app, client, live, key, &mut pending_fold),
            Focus::Input => handle_input_key(app, client, cli, live, key),
        }

        if app.should_quit {
            return Ok(RunOutcome::Quit);
        }

        if std::mem::take(&mut app.switch_requested) {
            let sessions = client.list_sessions()?;
            if sessions.is_empty() {
                app.push_log(LogEntry::warn("no stored sessions to switch to"));
                continue;
            }
            if let Some(id) = run_picker(terminal, &sessions)? {
                return Ok(RunOutcome::Switch(id));
            }
            // Cancelled: the next iteration redraws the normal session view.
        }
    }
}

fn handle_list_key(
    app: &mut App,
    client: &Client,
    live: &Live,
    key: KeyEvent,
    pending_fold: &mut bool,
) {
    // Vim-style fold chord: `z` then `R` (expand all) or `M` (collapse all).
    if std::mem::take(pending_fold) {
        match key.code {
            KeyCode::Char('R') => app.expand_all(),
            KeyCode::Char('M') => app.collapse_all(),
            _ => {}
        }
        return;
    }
    // Keys common to both views. `i`/`Tab` are excluded while the
    // full-screen preview is up: it renders with no input box at all, so
    // switching focus into one would leave keystrokes going nowhere visible.
    match key.code {
        KeyCode::Char('q') => return app.request_quit(),
        KeyCode::Char('i') if app.view != View::Preview => return app.enter_input(),
        KeyCode::Tab if app.view != View::Preview => return app.toggle_focus(),
        KeyCode::Char('r') => return app.toggle_raw(),
        KeyCode::Char('l') => return app.toggle_log(),
        KeyCode::Char('t') => return app.toggle_transcript(),
        KeyCode::Char('d') => return app.toggle_driva(),
        KeyCode::Char('P') => return app.toggle_fullscreen_preview(),
        KeyCode::Char('s') => {
            if let Live::Running { session_id, .. } = live {
                client.stop_session(session_id).ok();
                app.push_log(LogEntry::info("stop requested; closing agent input"));
            }
            return;
        }
        KeyCode::Char('V') => return app.request_switch(),
        _ => {}
    }
    match app.view {
        View::Events => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
            KeyCode::Char('J') => app.select_next_line(),
            KeyCode::Char('K') => app.select_prev_line(),
            KeyCode::Char(' ') | KeyCode::Enter => app.toggle_expand(),
            KeyCode::Char('o') => app.expand_selected(),
            KeyCode::Char('c') => app.collapse_selected(),
            KeyCode::Char('g') => app.select_first(),
            KeyCode::Char('G') => app.select_last(),
            KeyCode::Char('z') => *pending_fold = true,
            KeyCode::Char('m') => app.toggle_minor(),
            KeyCode::Char('p') => app.toggle_preview(),
            KeyCode::Char('C') => app.collapse_all(),
            _ => {}
        },
        View::Raw => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.raw_scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => app.raw_scroll_up(),
            KeyCode::Char('g') => app.raw_to_top(),
            KeyCode::Char('G') => app.raw_to_bottom(),
            _ => {}
        },
        View::Log => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.log_scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => app.log_scroll_up(),
            KeyCode::Char('g') => app.log_to_top(),
            KeyCode::Char('G') => app.log_to_bottom(),
            _ => {}
        },
        View::Transcript => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.transcript_scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => app.transcript_scroll_up(),
            KeyCode::Char('g') => app.transcript_to_top(),
            KeyCode::Char('G') => app.transcript_to_bottom(),
            _ => {}
        },
        // A short, static summary; nothing to scroll.
        View::Driva => {}
        // Browsing between entries updates which one's content is shown.
        View::Preview => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
            KeyCode::Char('J') => app.select_next_line(),
            KeyCode::Char('K') => app.select_prev_line(),
            KeyCode::Char('g') => app.select_first(),
            KeyCode::Char('G') => app.select_last(),
            _ => {}
        },
    }
}

fn handle_input_key(app: &mut App, client: &Client, cli: &Cli, live: &mut Live, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.enter_list(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => app.input_newline(),
        KeyCode::Enter => {
            if let Some(message) = app.take_message() {
                match live {
                    // The sent message returns as a UserMessage event, so it is
                    // not pushed here; send failures surface in the log view.
                    Live::Running { session_id, .. } if app.can_send() => {
                        if let Err(error) = client.send_message(session_id, &message) {
                            app.push_log(LogEntry::error(format!("send failed: {error:#}")));
                        }
                    }
                    Live::Running { .. } => app.push_log(LogEntry::warn(format!(
                        "not sent (session {}): {message}",
                        app.status.label()
                    ))),
                    Live::Viewing => app
                        .push_log(LogEntry::warn("not sent: viewed journal has no live agent")),
                    // The operator's first message: this is what actually
                    // starts the agent. Nothing was launched or sent before
                    // this point.
                    Live::Pending => match create_session(client, cli, Some(&message)) {
                        Ok(info) => {
                            app.profile_name = info.profile;
                            app.session_id = info.id.clone();
                            app.set_workspace_root(info.workspace);
                            app.set_driva_options(info.driva);
                            app.push_log(LogEntry::info(format!(
                                "journal: {}",
                                info.journal_path.display()
                            )));
                            app.status = Status::Running;
                            *live = Live::Running {
                                session_id: info.id,
                                cursor: 0,
                            };
                        }
                        Err(error) => {
                            app.push_log(LogEntry::error(format!(
                                "could not launch the agent: {error:#}"
                            )));
                            // Don't lose what they typed; let them retry.
                            app.set_input(message);
                        }
                    },
                }
            }
        }
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_delete_word()
        }
        KeyCode::Char(ch) => app.input_char(ch),
        _ => {}
    }
}

fn resolve_workspace(workspace: Option<&std::path::Path>) -> Result<PathBuf> {
    let raw = match workspace {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("determining the current directory")?,
    };
    raw.canonicalize()
        .with_context(|| format!("workspace directory {} must exist", raw.display()))
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("entering the alternate screen")?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout)).context("initialising terminal")?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().ok();
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    terminal.backend_mut().flush().ok();
    Ok(())
}
