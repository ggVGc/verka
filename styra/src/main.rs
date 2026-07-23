//! Styra's terminal entry point: CLI, terminal lifecycle, and the event loop
//! that wires the session threads to the application state and renderer.

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
use std::sync::mpsc::Receiver;
use std::time::Duration;

use styra::agent::{MountSpec, Profile, SandboxLayout, SessionMeta};
use styra::app::{App, Focus, View};
use styra::journal::Journal;
use styra::session::{LogEntry, Session, SessionSpec, SessionUpdate};
use styra::ui;

/// Run an interactive, isolated agent session in a terminal interface.
#[derive(Parser)]
#[command(name = "styra", about, version)]
struct Cli {
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
    /// choose one from those stored under .styra.
    #[arg(long, num_args = 0..=1, value_name = "SESSION")]
    view: Option<Option<PathBuf>>,
    /// Optional first message, sent to seed the opening turn.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let layout = SandboxLayout::default();

    // Bare `--view` (no path) needs an interactive terminal to browse
    // sessions in, so it is opened early only in that case; the other paths
    // below still report setup failures before taking over the terminal,
    // and the terminal the picker opened is reused below rather than torn
    // down and reopened.
    let mut terminal: Option<Terminal<CrosstermBackend<Stdout>>> = None;
    let view_target: Option<PathBuf> = match &cli.view {
        Some(Some(path)) => Some(path.clone()),
        Some(None) => {
            let sessions = styra::journal::list_sessions(&store_root()?)?;
            if sessions.is_empty() {
                println!(
                    "No sessions found under {}",
                    styra::journal::sessions_dir(&store_root()?).display()
                );
                return Ok(());
            }
            let mut term = setup_terminal()?;
            match run_picker(&mut term, &sessions) {
                Ok(Some(path)) => {
                    terminal = Some(term);
                    Some(path)
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

    // Build the application and, unless viewing, a live session up front so a
    // setup failure is reported plainly before the terminal is taken over.
    let mut app;
    let mut session: Option<Session> = None;
    let mut updates: Option<Receiver<SessionUpdate>> = None;

    if let Some(view) = &view_target {
        let meta = read_session_meta_or_error(view)?;
        let events = styra::journal::replay(view, meta.protocol)
            .with_context(|| format!("opening journal {}", view.display()))?;
        app = App::new(meta.profile, view.display().to_string());
        for event in events {
            // Skip carried-but-viewless traffic (e.g. app-server control
            // lines), matching what a live session shows; it stays available
            // in the raw view below.
            if !matches!(event, styra::event::AgentEvent::Unknown { .. }) {
                app.push_event(event);
            }
        }
        for line in styra::journal::replay_raw(view)
            .with_context(|| format!("opening journal {}", view.display()))?
        {
            app.push_raw(line);
        }
        // A replayed session has no live agent to end; mark it stopped.
        app.on_ended(styra::session::SessionEnd { exit_code: None, error: None });
    } else {
        let prompt = cli.prompt.join(" ");
        let seed = (!prompt.trim().is_empty()).then_some(prompt.as_str());
        let (new_app, spawned, receiver) = launch_live_session(&cli, &layout, seed)?;
        app = new_app;
        session = Some(spawned);
        updates = Some(receiver);
    }

    let mut terminal = match terminal {
        Some(terminal) => terminal,
        None => setup_terminal()?,
    };

    // Runs until the operator quits; a session switch stops the outgoing
    // session, seeds a fresh one from the picked session's transcript, and
    // loops back into run() with it, all inside the same terminal.
    let result = loop {
        let outcome = match run(&mut terminal, &mut app, session.as_ref(), updates.as_ref()) {
            Ok(outcome) => outcome,
            Err(error) => break Err(error),
        };
        match outcome {
            RunOutcome::Quit => break Ok(()),
            RunOutcome::Switch(seed_path) => {
                // Stopped and joined before the next session spawns; both
                // write under .styra and there is no reason to overlap them.
                drop(session.take());
                match switch_to_session(&cli, &layout, &seed_path) {
                    Ok((new_app, spawned, receiver)) => {
                        app = new_app;
                        session = Some(spawned);
                        updates = Some(receiver);
                    }
                    Err(error) => {
                        updates = None;
                        app.push_log(LogEntry::error(format!(
                            "could not switch session: {error:#}"
                        )));
                    }
                }
            }
        }
    };

    restore_terminal(&mut terminal)?;
    // Dropping the session here (after the terminal is restored) closes stdin
    // and joins the worker threads.
    drop(session);
    result
}

/// `.styra` under the current directory: the store `--view`, the picker, and
/// live sessions all read and write sessions under.
fn store_root() -> Result<PathBuf> {
    Ok(std::env::current_dir().context("determining the current directory")?.join(".styra"))
}

/// Read a stored session's recorded provenance, erroring plainly rather than
/// guessing a protocol: a session predating the `session.json` sidecar (or
/// missing it) cannot be decoded.
fn read_session_meta_or_error(path: &Path) -> Result<SessionMeta> {
    styra::journal::read_session_meta(path)
        .with_context(|| format!("reading session metadata for {}", path.display()))?
        .with_context(|| {
            format!(
                "session {} has no recorded agent metadata (session.json); \
                 it predates provenance tracking and cannot be replayed",
                path.display()
            )
        })
}

/// Launch a fresh live session on `cli.profile`: create its journal, spawn
/// the sandboxed agent process through Driva, and — if `seed` is given —
/// send it as the opening message. Used both for the CLI's trailing prompt
/// on first launch and a rendered transcript when switching sessions.
fn launch_live_session(
    cli: &Cli,
    layout: &SandboxLayout,
    seed: Option<&str>,
) -> Result<(App, Session, Receiver<SessionUpdate>)> {
    let profile = Profile::builtin(&cli.profile, layout)?;
    let workspace = resolve_workspace(cli.workspace.as_deref())?;
    let root = store_root()?;
    let (journal, session_id) = Journal::create_in_store(&root, &profile)?;
    let journal_path = journal.path().to_path_buf();
    let diagnostics = journal_path.parent().unwrap_or(&root).join("diagnostics.log");

    let mut profile = profile;
    profile.network = profile.network || cli.network;

    let spec = SessionSpec {
        profile: profile.clone(),
        working_directory: layout.workspace.clone(),
        workspace: MountSpec {
            source: workspace.clone(),
            destination: layout.workspace.clone(),
            writable: true,
        },
        // No extra temporary mounts: the profile's HOME lives under the
        // /tmp tmpfs Driva always provides.
        temporary_mounts: Vec::new(),
    };
    let backend = Box::new(driva::BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: Some(PathBuf::from("/")),
    });

    let (spawned, receiver) =
        Session::spawn(spec, backend, journal, session_id.clone(), diagnostics)?;
    let mut app = App::new(profile.name.clone(), session_id);
    app.set_workspace_root(workspace);
    app.push_log(LogEntry::info(format!("journal: {}", journal_path.display())));

    if let Some(seed) = seed.map(str::trim).filter(|seed| !seed.is_empty()) {
        spawned.send(seed)?;
    }

    Ok((app, spawned, receiver))
}

/// Render the picked stored session's journal to a transcript and launch a
/// fresh live session (still on `cli.profile`) seeded with it. See
/// `DESIGN.md`'s *Session switching* for why a rendered transcript rather
/// than a native protocol resume.
fn switch_to_session(
    cli: &Cli,
    layout: &SandboxLayout,
    seed_path: &Path,
) -> Result<(App, Session, Receiver<SessionUpdate>)> {
    let meta = read_session_meta_or_error(seed_path)?;
    let transcript = styra::journal::render_transcript(seed_path, meta.protocol)
        .with_context(|| format!("rendering a transcript for {}", seed_path.display()))?;
    launch_live_session(cli, layout, Some(&transcript))
}

/// The session picker loop: j/k or arrows to move, Enter to choose a
/// session, Esc or q to back out without picking one.
fn run_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    sessions: &[styra::journal::SessionSummary],
) -> Result<Option<PathBuf>> {
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
            KeyCode::Enter => return Ok(Some(sessions[selected].path.clone())),
            _ => {}
        }
    }
}

/// What the interactive loop returned control to `main` for.
enum RunOutcome {
    /// The operator quit.
    Quit,
    /// The operator picked a stored session to switch to.
    Switch(PathBuf),
}

/// The event loop: apply pending session updates, render, and handle input
/// until the operator quits or asks to switch sessions.
fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    session: Option<&Session>,
    updates: Option<&Receiver<SessionUpdate>>,
) -> Result<RunOutcome> {
    let mut pending_fold = false;
    loop {
        if let Some(updates) = updates {
            while let Ok(update) = updates.try_recv() {
                match update {
                    SessionUpdate::Event(event) => app.push_event(event),
                    SessionUpdate::Raw(line) => app.push_raw(line),
                    SessionUpdate::Log(entry) => app.push_log(entry),
                    SessionUpdate::Ended(end) => app.on_ended(end),
                }
            }
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
            Focus::List => handle_list_key(app, session, key, &mut pending_fold),
            Focus::Input => handle_input_key(app, session, key),
        }

        if app.should_quit {
            return Ok(RunOutcome::Quit);
        }

        if std::mem::take(&mut app.switch_requested) {
            let sessions = styra::journal::list_sessions(&store_root()?)?;
            if sessions.is_empty() {
                app.push_log(LogEntry::warn("no stored sessions to switch to"));
                continue;
            }
            if let Some(path) = run_picker(terminal, &sessions)? {
                return Ok(RunOutcome::Switch(path));
            }
            // Cancelled: the next iteration redraws the normal session view.
        }
    }
}

fn handle_list_key(app: &mut App, session: Option<&Session>, key: KeyEvent, pending_fold: &mut bool) {
    // Vim-style fold chord: `z` then `R` (expand all) or `M` (collapse all).
    if std::mem::take(pending_fold) {
        match key.code {
            KeyCode::Char('R') => app.expand_all(),
            KeyCode::Char('M') => app.collapse_all(),
            _ => {}
        }
        return;
    }
    // Keys common to both views.
    match key.code {
        KeyCode::Char('q') => return app.request_quit(),
        KeyCode::Char('i') => return app.enter_input(),
        KeyCode::Tab => return app.toggle_focus(),
        KeyCode::Char('r') => return app.toggle_raw(),
        KeyCode::Char('l') => return app.toggle_log(),
        KeyCode::Char('t') => return app.toggle_transcript(),
        KeyCode::Char('s') => {
            if let Some(session) = session {
                session.stop();
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
    }
}

fn handle_input_key(app: &mut App, session: Option<&Session>, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.enter_list(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => app.input_newline(),
        KeyCode::Enter => {
            if let Some(message) = app.take_message() {
                match session {
                    // The sent message returns as a UserMessage event, so it is
                    // not pushed here; send failures surface in the log view.
                    Some(session) if app.can_send() => {
                        if let Err(error) = session.send(&message) {
                            app.push_log(LogEntry::error(format!("send failed: {error:#}")));
                        }
                    }
                    Some(_) => app.push_log(LogEntry::warn(format!(
                        "not sent (session {}): {message}",
                        app.status.label()
                    ))),
                    None => app
                        .push_log(LogEntry::warn("not sent: viewed journal has no live agent")),
                }
            }
        }
        KeyCode::Backspace => app.input_backspace(),
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
