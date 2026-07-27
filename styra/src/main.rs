//! Styra's terminal client: CLI, terminal lifecycle, and the event loop that
//! drives the application through Styra's JSON Unix-socket API.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{Stdout, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod app;
mod ui;

use app::{App, Focus, Status, View};
use styra_server::agent::Selection;
use styra_server::api::{CreateSession, CreateWorkspace, SessionInfo};
use styra_server::{
    Client, InteractionSummary, InteractionUpdate, LogEntry, SessionSummary, WorkspaceSummary,
};

/// Run an interactive, isolated agent session in a terminal interface.
#[derive(Parser)]
#[command(name = "styra", about, version)]
struct Cli {
    /// Styra server Unix socket (default: $XDG_RUNTIME_DIR/styra/styra.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Start the Styra daemon in the background and exit, without opening the
    /// interface. A no-op if one is already listening on the socket.
    #[arg(short = 'd', long = "daemon", conflicts_with = "stop")]
    daemon: bool,
    /// Stop the Styra daemon listening on the socket (if any) and exit. Any
    /// live interactions it owns are ended with it.
    #[arg(long)]
    stop: bool,
    /// Agent profile to launch a live session with, as
    /// `provider[:model][/effort]` (`codex`, `claude:opus`,
    /// `codex:gpt-5.6-sol/xhigh`); a bare provider leaves the agent on its own
    /// configured model and effort. Seeds the interface's launch picker, which
    /// can change all three before the first message. Not used with `--view`:
    /// a viewed session carries its own recorded profile and protocol.
    #[arg(long, default_value = "codex")]
    profile: String,
    /// Host directory mounted writable as the agent workspace (default: cwd).
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Permit agent networking (profiles may default this on).
    #[arg(long)]
    network: bool,
    /// Apply a Driva execution template on top of the profile (see `driva
    /// templates`); may be repeated to layer several, e.g. a toolchain
    /// template like `rust` alongside the agent profile.
    #[arg(long = "template", value_name = "NAME")]
    template: Vec<String>,
    /// Open a captured journal read-only instead of launching an agent: with
    /// a path, that session directly; bare (no path), a picker to browse and
    /// choose one from the server's store.
    #[arg(long, num_args = 0..=1, value_name = "SESSION")]
    view: Option<Option<PathBuf>>,
    #[command(subcommand)]
    command: Option<CliCommand>,
    /// Optional first message, sent to seed the opening turn.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Attach to the persistent shell inside a live session's sandbox.
    Shell {
        /// Live Styra session to attach to.
        #[arg(long)]
        session: String,
    },
}

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
            stored
                .summary
                .profile
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            stored.summary.id,
        );
        app.workspace_id = stored.summary.workspace_id;
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
        app.on_ended(styra_server::InteractionEnd {
            exit_code: None,
            error: None,
        });
        live = Live::Viewing;
    } else {
        // `--profile` is the operator's opening launch choice; parsing it here
        // rejects an unknown agent, model syntax, or effort level before the
        // terminal is taken over, rather than at the first message.
        let selection = Selection::parse(&cli.profile)
            .with_context(|| format!("invalid --profile {:?}", cli.profile))?;
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
                    cursor: 0,
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
        None => setup_terminal()?,
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
        ) {
            Ok(outcome) => outcome,
            Err(error) => break Err(error),
        };
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
                        let selection = app.selection.clone();
                        app = App::pending(selection);
                        app.workspace_id = Some(active_workspace.id.clone());
                        live = Live::Pending;
                    }
                }
            }
            RunOutcome::Seed(transcript) => {
                let selection = app.selection.clone();
                app = App::pending(selection);
                app.workspace_id = Some(active_workspace.id.clone());
                app.set_input(transcript);
                live = Live::Pending;
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
                if let Live::Running { session_id, .. } =
                    std::mem::replace(&mut live, Live::Pending)
                {
                    client.stop_interaction(&session_id).ok();
                }
                // The blank start screen is where a launch is chosen, so it
                // opens on whatever the stopped interaction ran with — the usual next
                // step is the same agent again, or a deliberate change to
                // another one.
                let selection = app.selection.clone();
                app = App::pending(selection);
                app.workspace_id = Some(active_workspace.id.clone());
            }
        }
    };

    restore_terminal(&mut terminal)?;
    if let Live::Running { session_id, .. } = live {
        client.stop_interaction(&session_id).ok();
    }
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

/// Ask the server for a session on `selection`. The selection's canonical name
/// is the wire profile (`provider[:model][/effort]`), so the server resolves the
/// same agent, model, and effort the operator picked, and records that name in
/// the session's journal.
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
    let mut sessions = client.list_legacy_sessions()?;
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
        profile: selection.name(),
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
    let mut app = App::new(info.profile.clone(), info.id.clone());
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
    let mut app = App::new(interaction.profile.clone(), interaction.id.clone());
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
    let mut app = App::new(
        stored
            .summary
            .profile
            .clone()
            .unwrap_or_else(|| "unknown".into()),
        stored.summary.id,
    );
    app.workspace_id = stored.summary.workspace_id;
    for event in stored.events {
        if !matches!(event, styra_server::event::AgentEvent::Unknown { .. }) {
            app.push_event(event);
        }
    }
    for line in stored.raw {
        app.push_raw(line);
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

fn run_workspace_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    workspaces: &[WorkspaceSummary],
) -> Result<Option<WorkspaceSummary>> {
    let mut selected = 0usize;
    loop {
        terminal.draw(|frame| ui::render_workspace_picker(frame, workspaces, selected))?;
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
                selected = (selected + 1).min(workspaces.len() - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter => return Ok(Some(workspaces[selected].clone())),
            _ => {}
        }
    }
}

/// The current-interactions picker loop: j/k or arrows to move, Enter to attach to a
/// live interaction, Esc or q to back out. Mirrors [`run_picker`] but over the
/// server's live interactions rather than the stored-session store.
fn run_interactions_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    interactions: &[InteractionSummary],
) -> Result<Option<InteractionSummary>> {
    let mut selected = 0usize;
    let mut preview_id = String::new();
    let mut preview_cursor = 0u64;
    let mut preview_updates = Vec::new();
    loop {
        let selected_interaction = &interactions[selected];
        if preview_id != selected_interaction.id {
            preview_id.clone_from(&selected_interaction.id);
            preview_cursor = 0;
            preview_updates.clear();
        }

        match client.updates(&preview_id, preview_cursor) {
            Ok(batch) => {
                preview_cursor = batch.next;
                preview_updates.extend(batch.updates.into_iter().filter_map(|sequenced| {
                    match sequenced.update {
                        // Raw lines duplicate decoded events and make the
                        // compact preview noisy. Everything human-facing is
                        // useful here: activity, diagnostics, and interaction end.
                        InteractionUpdate::Raw(_) => None,
                        update => Some(update),
                    }
                }));
            }
            Err(error) => {
                let message = format!("could not load current log: {error:#}");
                if !preview_updates
                    .last()
                    .is_some_and(
                        |update| matches!(update, InteractionUpdate::Log(entry) if entry.message == message),
                    )
                {
                    preview_updates.push(InteractionUpdate::Log(LogEntry::error(message)));
                }
            }
        }

        terminal.draw(|frame| {
            ui::render_interactions_picker(frame, interactions, selected, &preview_updates)
        })?;

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
                selected = (selected + 1).min(interactions.len() - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter => return Ok(Some(interactions[selected].clone())),
            _ => {}
        }
    }
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
    /// Start composing a new Session from an existing Session's transcript.
    Seed(String),
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
    workspace_id: &str,
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
            handle_launcher_key(app, key);
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
            let Some(workspace) = run_workspace_picker(terminal, &workspaces)? else {
                continue;
            };
            let sessions = client.list_sessions(&workspace.id)?;
            if sessions.is_empty() {
                return Ok(RunOutcome::OpenWorkspace {
                    workspace,
                    session_id: None,
                });
            }
            if let Some(id) = run_picker(terminal, &sessions)? {
                return Ok(RunOutcome::OpenWorkspace {
                    workspace,
                    session_id: Some(id),
                });
            }
            // Cancelling the Session picker leaves the current view untouched.
        }

        if std::mem::take(&mut app.seed_requested) {
            if app.session_id.is_empty() {
                app.push_log(LogEntry::warn("no Session to seed from"));
            } else {
                return Ok(RunOutcome::Seed(client.transcript(&app.session_id)?));
            }
        }

        if std::mem::take(&mut app.interactions_requested) {
            let interactions = client.list_interactions()?;
            if interactions.is_empty() {
                app.push_log(LogEntry::warn("no live interactions on the server"));
                continue;
            }
            if let Some(interaction) = run_interactions_picker(terminal, client, &interactions)? {
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
/// them, `Enter` to apply the choice (it never launches — the operator's first
/// message still does that), `Esc`/`q` to leave it as it was.
fn handle_launcher_key(app: &mut App, key: KeyEvent) {
    let Some(launcher) = app.launcher.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => launcher.next(),
        KeyCode::Char('k') | KeyCode::Up => launcher.prev(),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => launcher.next_column(),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => launcher.prev_column(),
        KeyCode::Enter => app.confirm_launcher(),
        KeyCode::Esc | KeyCode::Char('q') => app.cancel_launcher(),
        _ => {}
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
                client.stop_interaction(session_id).ok();
                app.push_log(LogEntry::info("stop requested; closing agent input"));
            }
            return;
        }
        // Only before a launch: a running session's agent and model are settled
        // facts about a process that is already up. `S` first, then `L`.
        KeyCode::Char('L') => return app.open_launcher(),
        KeyCode::Char('V') => return app.request_workspace(),
        KeyCode::Char('F') => return app.request_seed(),
        KeyCode::Char('A') => return app.request_interactions(),
        KeyCode::Char('S') => return app.request_reset(),
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

fn handle_input_key(
    app: &mut App,
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    live: &mut Live,
    key: KeyEvent,
) {
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
                    Live::Viewing => {
                        app.push_log(LogEntry::warn("not sent: viewed journal has no live agent"))
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
                                app.profile_name = info.profile;
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
                        }
                    }
                }
            }
        }
        KeyCode::Backspace => app.input_backspace(),
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
    fn legacy_prompt_launch_still_parses_without_a_subcommand() {
        let cli = Cli::try_parse_from(["styra", "--profile", "codex", "hello"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.prompt, vec!["hello"]);
    }
}
