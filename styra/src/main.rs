//! Styra's terminal client: CLI, terminal lifecycle, and the event loop that
//! drives the application through Styra's JSON Unix-socket API.

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod app;
mod cli;
mod picker;
mod preferences;
mod terminal;
mod ui;

use app::{App, Focus, Status, View};
use cli::{Cli, CliCommand};
use styra_server::agent::Selection;
use styra_server::api::{CreateSession, CreateWorkspace, ResumeSession, SessionInfo};
use styra_server::{
    Client, InteractionSummary, InteractionUpdate, LogEntry, SessionSummary, WorkspaceSummary,
};

fn main() -> Result<()> {
    if let Some(result) = styra_server::broker::exit_if_requested() {
        return result;
    }
    // The connect-or-spawn path spawns the daemon by re-exec'ing *this* binary
    // with the serve sentinel in its environment; honour it before parsing the
    // client CLI so the re-exec'd copy becomes the server instead of a second
    // TUI. See `styra_server::spawn`.
    if let Some(result) = styra_server::serve_if_requested() {
        return result;
    }
    let cli = Cli::parse();
    let socket = match &cli.socket {
        Some(path) => path.clone(),
        None => styra_server::paths::default_socket()?,
    };

    // Daemon lifecycle commands run without opening the interface at all.
    if cli.stop {
        return stop_daemon(&socket);
    }
    if cli.daemon {
        return start_daemon(&socket);
    }

    // Connect-or-spawn: use the running server if one answers, otherwise start
    // a detached daemon bound to this socket and wait for it. The daemon
    // outlives this client, so live sessions survive detach/quit.
    let client = styra_server::ensure_server(&socket)
        .with_context(|| format!("Styra server is unavailable at {}", socket.display()))?;
    if let Some(CliCommand::Shell { session }) = &cli.command {
        return attach_shell(&client, session);
    }
    let host_path = resolve_workspace(cli.workspace.as_deref())?;
    let mut active_workspace = workspace_for_host(&client, &host_path)?;
    let preferences_path = preferences::default_path()?;

    // Bare `--view` (no path) needs an interactive terminal to browse
    // sessions in, so it is opened early only in that case; the other paths
    // below still report setup failures before taking over the terminal,
    // and the terminal the picker opened is reused below rather than torn
    // down and reopened.
    let mut terminal: Option<Terminal<CrosstermBackend<Stdout>>> = None;
    let view_target: Option<PathBuf> = match &cli.view {
        Some(Some(path)) => Some(path.clone()),
        Some(None) => {
            let sessions = all_sessions(&client)?;
            if sessions.is_empty() {
                println!("No sessions found by the Styra server");
                return Ok(());
            }
            let mut term = terminal::setup()?;
            match picker::run_session_picker(&mut term, &sessions) {
                Ok(Some(id)) => {
                    terminal = Some(term);
                    Some(PathBuf::from(id))
                }
                Ok(None) => {
                    terminal::restore(&mut term)?;
                    return Ok(());
                }
                Err(error) => {
                    terminal::restore(&mut term)?;
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
        app = App::new(stored.summary.selection, stored.summary.id);
        app.workspace_id = Some(stored.summary.workspace_id);
        // `stored.events[i]` and `stored.raw[i]` are decoded from the same
        // journal record (see `journal::replay`/`replay_raw`), so pushing
        // them in lockstep — raw line first, as a live session receives it —
        // gives each kept entry a `raw_index` that actually points at its
        // own wire line instead of leaving it unset.
        for (event, line) in stored.events.into_iter().zip(stored.raw) {
            app.push_raw(line);
            // Skip carried-but-viewless traffic (e.g. app-server control
            // lines), matching what a live session shows; it stays available
            // in the raw view above.
            if !matches!(event, styra_server::event::AgentEvent::Unknown { .. }) {
                app.push_event(event);
            }
        }
        // A replayed session has no live agent to end; mark it stopped.
        app.on_ended(styra_server::InteractionEnd {
            exit_code: None,
            error: None,
        });
        live = Live::Viewing;
    } else {
        let selection = preferences::load_or_default(&preferences_path)?;
        let prompt = cli.prompt.join(" ");
        let seed = (!prompt.trim().is_empty()).then_some(prompt.as_str());
        match seed {
            // A trailing prompt is input the operator already gave (as a CLI
            // argument), so it is fine to launch immediately.
            Some(seed) => {
                let (new_app, info) = launch_live_session(
                    &client,
                    &cli,
                    &active_workspace.id,
                    &selection,
                    Some(seed),
                )?;
                app = new_app;
                live = Live::Running {
                    session_id: info.id,
                    cursor: info.updates_after,
                };
            }
            // No seed: nothing has been said to an agent yet, so nothing is
            // launched yet either. The event loop spawns the session the
            // moment the operator submits their first message — on whatever
            // the launch picker holds by then.
            None => {
                app = App::pending(selection);
                app.workspace_id = Some(active_workspace.id.clone());
                live = Live::Pending;
            }
        }
    }

    let mut terminal = match terminal {
        Some(terminal) => terminal,
        None => terminal::setup()?,
    };

    // Runs until the operator quits. Workspace and Session selection only
    // changes what this client views; server-owned Interactions continue.
    let result = loop {
        let outcome = match run(
            &mut terminal,
            &mut app,
            &client,
            &cli,
            &active_workspace.id,
            &mut live,
            &preferences_path,
        ) {
            Ok(outcome) => outcome,
            Err(error) => break Err(error),
        };
        if let Some(session_id) = interaction_stopped_by(&outcome, &live) {
            client.stop_interaction(session_id).ok();
        }
        match outcome {
            RunOutcome::Quit => break Ok(()),
            RunOutcome::OpenWorkspace {
                workspace,
                session_id,
            } => {
                active_workspace = workspace;
                match session_id {
                    Some(session_id) => match open_session(&client, &session_id) {
                        Ok((new_app, new_live)) => {
                            app = new_app;
                            live = new_live;
                        }
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not open Session {session_id}: {error:#}"
                        ))),
                    },
                    None => {
                        let selection = preferences::load_or_default(&preferences_path)?;
                        app = App::pending(selection);
                        app.workspace_id = Some(active_workspace.id.clone());
                        live = Live::Pending;
                    }
                }
            }
            // Attach to another live interaction. The outgoing one is left running on
            // the server (interactions outlive a client); we just stop viewing it.
            RunOutcome::Attach(interaction) => {
                let id = interaction.id.clone();
                match attach_live_interaction(&client, interaction) {
                    Ok((new_app, new_live)) => {
                        app = new_app;
                        live = new_live;
                    }
                    Err(error) => {
                        app.push_log(LogEntry::error(format!(
                            "could not attach to interaction {id}: {error:#}"
                        )));
                    }
                }
            }
            // Stop the current interaction and return to the blank start screen.
            RunOutcome::Reset => {
                live = Live::Pending;
                // A reset returns to the standing launch default, independent
                // of the selection recorded by the Session just left.
                let selection = preferences::load_or_default(&preferences_path)?;
                app = App::pending(selection);
                app.workspace_id = Some(active_workspace.id.clone());
            }
        }
    };

    terminal::restore(&mut terminal)?;
    result
}

fn attach_shell(client: &Client, session: &str) -> Result<()> {
    let shell = client
        .shell(session)
        .with_context(|| format!("opening sandbox shell for session {session}"))?;
    let error = Command::new(&shell.tmux)
        .arg("-S")
        .arg(&shell.socket)
        .args(["attach-session", "-t", "shell"])
        .exec();
    Err(error).with_context(|| {
        format!(
            "attaching to session {session} with {}",
            shell.tmux.display()
        )
    })
}

/// `--daemon`: bring up the background daemon and return. Reuses the ordinary
/// connect-or-spawn path, so it is idempotent — if one is already listening,
/// it is left as-is rather than started twice.
fn start_daemon(socket: &Path) -> Result<()> {
    if Client::new(socket).health().is_ok() {
        println!("styra daemon already running on {}", socket.display());
        return Ok(());
    }
    styra_server::ensure_server(socket)
        .with_context(|| format!("starting the Styra daemon on {}", socket.display()))?;
    println!("started styra daemon on {}", socket.display());
    Ok(())
}

/// `--stop`: ask the daemon on `socket` to shut down. Reports plainly when
/// none is listening rather than treating it as an error.
fn stop_daemon(socket: &Path) -> Result<()> {
    let client = Client::new(socket);
    if client.health().is_err() {
        println!("no styra daemon running on {}", socket.display());
        return Ok(());
    }
    client
        .shutdown()
        .with_context(|| format!("stopping the Styra daemon on {}", socket.display()))?;
    println!("stopped styra daemon on {}", socket.display());
    Ok(())
}

/// Ask the server for a session with the provider, model, and effort selected
/// by the operator. The launch profile is resolved internally by the server.
fn workspace_for_host(client: &Client, host_path: &Path) -> Result<WorkspaceSummary> {
    let canonical = host_path.canonicalize()?;
    if let Some(workspace) = client
        .list_workspaces()?
        .into_iter()
        .find(|workspace| workspace.host_path == canonical)
    {
        return Ok(workspace);
    }
    client.create_workspace(&CreateWorkspace {
        host_path: canonical,
        name: None,
    })
}

fn all_sessions(client: &Client) -> Result<Vec<SessionSummary>> {
    let mut sessions = Vec::new();
    for workspace in client.list_workspaces()? {
        sessions.extend(client.list_sessions(&workspace.id)?);
    }
    sessions.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(sessions)
}

fn create_session(
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    selection: &Selection,
    seed: Option<&str>,
) -> Result<SessionInfo> {
    client.create_session(&CreateSession {
        workspace_id: workspace_id.to_owned(),
        selection: selection.clone(),
        network: cli.network,
        templates: cli.template.clone(),
        message: seed.map(str::to_owned),
    })
}

/// Spawn a session and wrap it in a fresh `App`. Used for the CLI's trailing
/// prompt on first launch, the only case where the agent starts before the
/// event loop takes over.
fn launch_live_session(
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    selection: &Selection,
    seed: Option<&str>,
) -> Result<(App, SessionInfo)> {
    let info = create_session(client, cli, workspace_id, selection, seed)?;
    let mut app = App::new(info.selection.clone(), info.id.clone());
    app.workspace_id = Some(info.workspace_id.clone());
    app.set_workspace_root(info.workspace.clone());
    app.set_driva_options(info.driva.clone());
    app.push_log(LogEntry::info(format!(
        "journal: {}",
        info.journal_path.display()
    )));
    Ok((app, info))
}

/// Attach to a live interaction: rebuild an `App` from its summary and replay the
/// updates the server has accumulated for it, so the view matches what the interaction
/// has done so far and the event loop can continue polling from the cursor.
fn attach_live_interaction(
    client: &Client,
    interaction: InteractionSummary,
) -> Result<(App, Live)> {
    let mut app = App::new(interaction.selection.clone(), interaction.id.clone());
    app.workspace_id = Some(interaction.workspace_id.clone());
    app.set_workspace_root(interaction.workspace.clone());
    app.set_driva_options(interaction.driva.clone());
    let batch = client.updates(&interaction.id, 0)?;
    let cursor = batch.next;
    for sequenced in batch.updates {
        apply_update(&mut app, sequenced.update);
    }
    Ok((
        app,
        Live::Running {
            session_id: interaction.id,
            cursor,
        },
    ))
}

fn open_session(client: &Client, session_id: &str) -> Result<(App, Live)> {
    if let Some(interaction) = client
        .list_interactions()?
        .into_iter()
        .find(|interaction| interaction.id == session_id)
    {
        return attach_live_interaction(client, interaction);
    }
    let stored = client.stored_session(session_id)?;
    let mut app = App::new(stored.summary.selection, stored.summary.id);
    app.workspace_id = Some(stored.summary.workspace_id);
    // See the matching loop in `main`'s `--view` handling for why raw and
    // event are pushed together, index for index, rather than as two
    // separate passes.
    for (event, line) in stored.events.into_iter().zip(stored.raw) {
        app.push_raw(line);
        if !matches!(event, styra_server::event::AgentEvent::Unknown { .. }) {
            app.push_event(event);
        }
    }
    app.on_ended(styra_server::InteractionEnd {
        exit_code: None,
        error: None,
    });
    Ok((app, Live::Viewing))
}

fn session_id_from_target(target: &Path) -> Result<String> {
    target
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .with_context(|| format!("invalid session target {}", target.display()))
}


/// What the interactive loop returned control to `main` for.
enum RunOutcome {
    /// The operator quit.
    Quit,
    /// The operator chose a Workspace and optionally one of its Sessions.
    OpenWorkspace {
        workspace: WorkspaceSummary,
        session_id: Option<String>,
    },
    /// The operator picked a live interaction to attach this client to. The outgoing
    /// interaction is left running on the server, not stopped.
    Attach(InteractionSummary),
    /// The operator stopped the current interaction and asked to return to the blank
    /// start screen.
    Reset,
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
    /// A replayed journal (`--view`, a reopened Session, or one this client
    /// lost its connection to); no live agent is attached. Sending a message
    /// resumes it through the provider's native mechanism.
    Viewing,
}

/// Return the running interaction an in-client transition explicitly stops.
///
/// Quitting the TUI is a detach: the daemon keeps owning the interaction so a
/// later client can reattach through the Workspace/Session or Interactions
/// picker. Reset is the destructive transition and deliberately stops it.
fn interaction_stopped_by<'a>(outcome: &RunOutcome, live: &'a Live) -> Option<&'a str> {
    match (outcome, live) {
        (RunOutcome::Reset, Live::Running { session_id, .. }) => Some(session_id),
        _ => None,
    }
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
    workspace_id: &str,
    live: &mut Live,
    preferences_path: &Path,
) -> Result<RunOutcome> {
    let mut pending_fold = false;
    loop {
        let mut disconnected = false;
        if let Live::Running { session_id, cursor } = live {
            match client.updates(session_id, *cursor) {
                Ok(batch) => {
                    *cursor = batch.next;
                    for sequenced in batch.updates {
                        apply_update(app, sequenced.update);
                    }
                }
                Err(error) => {
                    app.push_log(LogEntry::error(format!("update poll failed: {error:#}")));
                    app.on_ended(styra_server::InteractionEnd {
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

        // A queued message is dispatched only after the preceding turn has
        // completed. Keep it queued if the interaction was stopped or the
        // send fails, so Esc can preserve it for a later resume.
        if let Live::Running { session_id, .. } = live {
            if app.status == Status::Idle {
                if let Some(message) = app.take_queued_message() {
                    match client.send_message(session_id, &message) {
                        Ok(()) => app.status = Status::Running,
                        Err(error) => {
                            app.queue_message(message);
                            app.push_log(LogEntry::error(format!("queued send failed: {error:#}")));
                        }
                    }
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

        // The launch picker is modal: while it is open it owns every key, so
        // neither focus's bindings can fire behind it.
        if app.launcher.is_some() {
            handle_launcher_key(app, key, preferences_path);
            continue;
        }

        match app.focus {
            Focus::List => handle_list_key(app, client, live, key, &mut pending_fold),
            Focus::Input => handle_input_key(app, client, cli, workspace_id, live, key),
        }

        if app.should_quit {
            return Ok(RunOutcome::Quit);
        }

        if std::mem::take(&mut app.workspace_requested) {
            let workspaces = client.list_workspaces()?;
            if workspaces.is_empty() {
                app.push_log(LogEntry::warn("no Workspaces to open"));
                continue;
            }
            let Some(workspace) = picker::run_workspace_picker(terminal, &workspaces)? else {
                continue;
            };
            let sessions = client.list_sessions(&workspace.id)?;
            if sessions.is_empty() {
                return Ok(RunOutcome::OpenWorkspace {
                    workspace,
                    session_id: None,
                });
            }
            if let Some(id) = picker::run_session_picker(terminal, &sessions)? {
                return Ok(RunOutcome::OpenWorkspace {
                    workspace,
                    session_id: Some(id),
                });
            }
            // Cancelling the Session picker leaves the current view untouched.
        }

        if std::mem::take(&mut app.interactions_requested) {
            let interactions = client.list_interactions()?;
            if interactions.is_empty() {
                app.push_log(LogEntry::warn("no live interactions on the server"));
                continue;
            }
            if let Some(interaction) = picker::run_interactions_picker(terminal, client, &interactions)? {
                return Ok(RunOutcome::Attach(interaction));
            }
            // Cancelled: the next iteration redraws the normal session view.
        }

        if std::mem::take(&mut app.reset_requested) {
            return Ok(RunOutcome::Reset);
        }
    }
}

/// Apply one session update to the app. Shared by the live event loop and by
/// [`attach_live_interaction`], which replays an interaction's accumulated updates the same way.
fn apply_update(app: &mut App, update: InteractionUpdate) {
    match update {
        InteractionUpdate::Event(event) => app.push_event(event),
        InteractionUpdate::Raw(line) => app.push_raw(line),
        InteractionUpdate::Log(entry) => app.push_log(entry),
        InteractionUpdate::Ended(end) => app.on_ended(end),
    }
}

/// Keys for the launch picker: `j`/`k` within a column, `Tab`/`h`/`l` between
/// them, `Enter` to save and apply the standing default (it never launches —
/// the operator's first message still does that), `Esc`/`q` to leave it as it
/// was.
fn handle_launcher_key(app: &mut App, key: KeyEvent, preferences_path: &Path) {
    let Some(launcher) = app.launcher.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => launcher.next(),
        KeyCode::Char('k') | KeyCode::Up => launcher.prev(),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => launcher.next_column(),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => launcher.prev_column(),
        KeyCode::Enter => {
            app.confirm_launcher();
            if let Err(error) = preferences::save(preferences_path, &app.selection) {
                app.push_log(LogEntry::error(format!(
                    "could not save launch defaults: {error:#}"
                )));
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => app.cancel_launcher(),
        _ => {}
    }
}

fn handle_list_key(
    app: &mut App,
    client: &Client,
    live: &mut Live,
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
    // Keys common to both views. `i` is excluded while the full-screen
    // preview is up: it renders with no input box at all, so entering input
    // focus would leave keystrokes going nowhere visible.
    match key.code {
        KeyCode::Char('q') => return app.request_quit(),
        KeyCode::Char('s') => return pause_interaction(app, client, live),
        KeyCode::Char('i') if app.view != View::Preview => return app.enter_input(),
        KeyCode::Char('r') => return app.toggle_raw(),
        KeyCode::Char('l') => return app.toggle_log(),
        KeyCode::Char('t') => return app.toggle_transcript(),
        KeyCode::Char('d') => return app.toggle_driva(),
        KeyCode::Char('P') => return app.toggle_fullscreen_preview(),
        // Only before a launch: a running session's agent and model are settled
        // facts about a process that is already up. `S` first, then `L`.
        KeyCode::Char('L') => return app.open_launcher(),
        KeyCode::Char('V') => return app.request_workspace(),
        KeyCode::Char('A') => return app.request_interactions(),
        KeyCode::Char('S') => return app.request_reset(),
        _ => {}
    }
    match app.view {
        View::Events => match key.code {
            KeyCode::PageDown if app.show_preview => app.preview_page_down(),
            KeyCode::PageUp if app.show_preview => app.preview_page_up(),
            KeyCode::Char('j') | KeyCode::Down => app.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
            KeyCode::Char('J') => app.select_next_line(),
            KeyCode::Char('K') => app.select_prev_line(),
            KeyCode::Char(' ') | KeyCode::Enter => app.toggle_expand(),
            KeyCode::Char('o') => app.expand_selected(),
            KeyCode::Char('g') => app.select_first(),
            KeyCode::Char('G') => app.select_last(),
            KeyCode::Char('z') => *pending_fold = true,
            KeyCode::Char('m') => app.toggle_minor(),
            KeyCode::Char('p') => app.toggle_preview(),
            KeyCode::Char('C') => app.collapse_all(),
            _ => {}
        },
        View::Raw => match key.code {
            KeyCode::PageDown => app.raw_preview_page_down(),
            KeyCode::PageUp => app.raw_preview_page_up(),
            KeyCode::Char('j') | KeyCode::Down => app.raw_select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.raw_select_prev(),
            KeyCode::Char('g') => app.raw_select_first(),
            KeyCode::Char('G') => app.raw_select_last(),
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
            KeyCode::PageDown => app.preview_page_down(),
            KeyCode::PageUp => app.preview_page_up(),
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

fn handle_input_key(
    app: &mut App,
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    live: &mut Live,
    key: KeyEvent,
) {
    match key.code {
        // Escape leaves the message box and returns to the list view.
        KeyCode::Esc => app.enter_list(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => app.input_newline(),
        KeyCode::Enter => {
            if let Some(message) = app.take_message() {
                match live {
                    // The sent message returns as a UserMessage event, so it is
                    // not pushed here; send failures surface in the log view.
                    Live::Running { .. } if app.status == Status::Running => {
                        app.queue_message(message);
                        app.push_log(LogEntry::info(format!(
                            "message queued ({} waiting)",
                            app.queued_message_count()
                        )));
                    }
                    Live::Running { session_id, .. } if app.status == Status::Idle => {
                        if let Err(error) = client.send_message(session_id, &message) {
                            app.push_log(LogEntry::error(format!("send failed: {error:#}")));
                        }
                    }
                    // Stopped, ended, or merely viewed (reopened from the
                    // picker, or `--view`'d from disk): resume the Session
                    // through its provider's native mechanism, then deliver
                    // the message to the revived agent.
                    Live::Running { .. } | Live::Viewing => {
                        resume_and_send(app, client, cli, live, message)
                    }
                    // The operator's first message: this is what actually
                    // starts the agent. Nothing was launched or sent before
                    // this point.
                    // The launch picker's standing choice is what starts here.
                    Live::Pending => {
                        let selection = app.selection.clone();
                        match create_session(client, cli, workspace_id, &selection, Some(&message))
                        {
                            Ok(info) => {
                                app.selection = info.selection;
                                app.workspace_id = Some(info.workspace_id);
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
                                    cursor: info.updates_after,
                                };
                            }
                            Err(error) => {
                                app.push_log(LogEntry::error(format!(
                                    "could not launch the agent: {error:#}"
                                )));
                                // Don't lose what they typed; let them retry.
                                app.set_input(message);
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Up => app.input_history_previous(),
        KeyCode::Down => app.input_history_next(),
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_delete_word()
        }
        // The start screen opens in input focus, so the launch picker is
        // reachable from here too rather than only after an `Esc`. A control
        // chord because plain letters are message text.
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => app.open_launcher(),
        KeyCode::Char(ch) => app.input_char(ch),
        _ => {}
    }
}

/// Resume `app.session_id` through its provider's native mechanism, then
/// deliver `message` to the freshly revived agent. Used when the operator
/// sends a new message to a Session that is stopped, ended, or only being
/// viewed (no live agent attached).
fn resume_and_send(app: &mut App, client: &Client, cli: &Cli, live: &mut Live, message: String) {
    if app.session_id.is_empty() {
        app.push_log(LogEntry::warn("not sent: no Session to resume"));
        return;
    }
    match client.resume_session(&ResumeSession {
        id: app.session_id.clone(),
        network: cli.network,
        templates: cli.template.clone(),
    }) {
        Ok(info) => {
            app.set_workspace_root(info.workspace);
            app.set_driva_options(info.driva);
            app.push_log(LogEntry::info("resumed with provider-native context"));
            let session_id = info.id;
            app.session_id = session_id.clone();
            app.status = Status::Running;
            *live = Live::Running {
                session_id: session_id.clone(),
                cursor: info.updates_after,
            };
            if let Err(error) = client.send_message(&session_id, &message) {
                app.push_log(LogEntry::error(format!("send failed: {error:#}")));
            }
        }
        Err(error) => {
            app.push_log(LogEntry::error(format!(
                "could not resume Session {}: {error:#}",
                app.session_id
            )));
            // Don't lose what they typed; let them retry.
            app.set_input(message);
        }
    }
}

fn pause_interaction(app: &mut App, client: &Client, live: &mut Live) {
    // `c` in the main interaction is an interrupt, like the agent clients'
    // own escape key. Close this agent's input stream but keep the TUI ready
    // to launch a fresh interaction on the next message.
    if let Live::Running { session_id, .. } = live {
        if let Err(error) = client.stop_interaction(session_id) {
            app.push_log(LogEntry::error(format!("pause failed: {error:#}")));
        } else {
            let cleared = app.clear_queued_messages();
            app.status = Status::Stopped;
            app.push_log(LogEntry::info(if cleared == 0 {
                "interaction paused; send a new message to start again".into()
            } else {
                format!(
                    "interaction paused; cleared {cleared} queued message(s); send a new message to start again"
                )
            }));
            *live = Live::Pending;
        }
    } else {
        app.enter_list();
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


#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn shell_subcommand_requires_and_captures_a_session() {
        let cli = Cli::try_parse_from(["styra", "shell", "--session", "styra-123"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(CliCommand::Shell { session }) if session == "styra-123"
        ));
    }

    #[test]
    fn trailing_prompt_launch_parses_without_a_subcommand() {
        let cli = Cli::try_parse_from(["styra", "hello"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.prompt, vec!["hello"]);
    }

    #[test]
    fn profile_is_not_a_command_line_option() {
        assert!(Cli::try_parse_from(["styra", "--profile", "codex"]).is_err());
    }

    #[test]
    fn quitting_detaches_but_reset_stops_the_running_interaction() {
        let live = Live::Running {
            session_id: "styra-live".into(),
            cursor: 7,
        };

        assert_eq!(interaction_stopped_by(&RunOutcome::Quit, &live), None);
        assert_eq!(
            interaction_stopped_by(&RunOutcome::Reset, &live),
            Some("styra-live")
        );
    }

    #[test]
    fn confirming_the_launcher_saves_the_selection_for_the_next_start() {
        let root = std::env::temp_dir().join(format!(
            "styra-launch-default-handler-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        let path = root.join("defaults.json");
        let selection =
            Selection::parse("claude:claude-sonnet-5/max").expect("valid test selection");
        let mut app = App::pending(selection.clone());
        app.open_launcher();

        handle_launcher_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &path,
        );

        assert_eq!(preferences::load_or_default(&path).unwrap(), selection);
        std::fs::remove_dir_all(root).ok();
    }
}
