//! Styra's terminal client: CLI, terminal lifecycle, and the event loop that
//! drives the application through Styra's JSON Unix-socket API.

use anyhow::{Context, Result};
use clap::Parser;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
mod activity;
mod answer;
mod app;
mod cli;
mod clipboard;
mod composer;
mod config;
mod event_loop;
mod files;
mod help;
mod ingest;
mod insert;
mod interactions;
mod keymap;
mod keys;
mod launch;
mod launcher;
mod loader;
mod mount;
mod notes;
mod notices;
mod outbox;
mod picker;
mod preferences;
mod preview;
mod raw;
mod session;
mod tail;
mod terminal;
mod timeline;
mod ui;
mod workspace;

use app::{App, LaunchPolicy};
use cli::{Cli, CliCommand};
use config::Config;
use event_loop::RunOutcome;
use session::Live;
use styra_server::{Client, LogEntry, WorkspaceSummary};

/// Point the app at the Workspace it is now showing: its display name, and the
/// standing launch policy every interaction started there is layered onto.
///
/// Both are resolved from Workspace metadata for the same reason — a Session
/// carries only the durable Workspace id — and both have to be refreshed at the
/// same moments, which is why one function does it. Following a Session into
/// another Workspace without this would leave the Driva view attributing that
/// Workspace's launches to the policy of the one just left.
fn refresh_workspace_context(app: &mut App, client: &Client, active: &WorkspaceSummary) {
    let workspace = app
        .workspace
        .id
        .as_deref()
        .filter(|id| *id == active.id)
        .map(|_| active.clone())
        .or_else(|| {
            let id = app.workspace.id.as_deref()?;
            client
                .list_workspaces()
                .ok()?
                .into_iter()
                .find(|workspace| workspace.id == id)
        });
    app.workspace.name = workspace.as_ref().map(session::workspace_display_name);
    if let Some(workspace) = workspace {
        app.launch.set_workspace(workspace.launch);
    }
}

fn workspace_for_new_session(
    app: &App,
    active: &WorkspaceSummary,
    workspaces: &[WorkspaceSummary],
) -> WorkspaceSummary {
    let Some(workspace_id) = app.workspace.id.as_deref() else {
        return active.clone();
    };
    if workspace_id == active.id {
        return active.clone();
    }
    workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .cloned()
        .unwrap_or_else(|| active.clone())
}

fn pending_app(
    selection: styra_server::agent::Selection,
    launch: LaunchPolicy,
    workspace: &WorkspaceSummary,
) -> App {
    let mut app = App::pending(selection);
    app.launch.interaction = launch;
    app.launch.set_workspace(workspace.launch.clone());
    app.workspace.id = Some(workspace.id.clone());
    app.workspace.enter(workspace.host_path.clone());
    app
}

/// What a new interaction starts from: the operator's saved defaults with this
/// invocation's flags over them.
///
/// `--network` only ever grants, matching the way the server ORs it with the
/// profile's and template's own policy. A `--template` replaces the saved list
/// rather than adding to it, because the flag names a whole layering and the
/// order within it is significant.
///
/// This is one launch's *own* policy, not the whole of it: whatever standing
/// policy the Workspace being launched in carries is layered underneath, on the
/// server. A client that has saved nothing therefore launches under exactly the
/// Workspace's policy.
///
/// Re-read rather than cached, so a policy saved with `D` during the session
/// is what the next blank screen offers.
fn standing_launch(
    preferences_path: &Path,
    cli: &Cli,
) -> Result<(styra_server::agent::Selection, LaunchPolicy)> {
    let defaults = preferences::load_or_default(preferences_path)?;
    let mut launch = defaults.launch;
    if cli.network {
        launch.network = Some(true);
    }
    if !cli.template.is_empty() {
        launch.templates = cli.template.clone();
    }
    Ok((defaults.selection, launch))
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
    let config = Config::default();
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
        return match session {
            Some(session) => attach_shell(&client, session),
            None => browse_shells(&client),
        };
    }
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
            let mut sessions = session::all_sessions(&client)?;
            if sessions.is_empty() {
                println!("No sessions found by the Styra server");
                return Ok(());
            }
            let mut term = terminal::setup()?;
            match picker::run_session_picker(&mut term, &client, &mut sessions) {
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

    // An ordinary interactive launch enters the Workspace associated with the
    // current directory when one exists. Otherwise it starts with the durable
    // Workspace list. Explicit CLI targets retain their direct behavior.
    let mut active_workspace = if cli.workspace.is_none()
        && cli.view.is_none()
        && cli.command.is_none()
    {
        let current_directory = session::resolve_workspace(None)?;
        let mut workspaces = client.list_workspaces()?;
        if let Some(workspace) = session::find_workspace_for_host(&workspaces, &current_directory) {
            workspace
        } else {
            let mut term = match terminal.take() {
                Some(term) => term,
                None => terminal::setup()?,
            };
            let choice = match picker::run_workspace_picker(&mut term, &client, &mut workspaces) {
                Ok(Some(choice)) => choice,
                Ok(None) => {
                    terminal::restore(&mut term)?;
                    return Ok(());
                }
                Err(error) => {
                    terminal::restore(&mut term)?;
                    return Err(error);
                }
            };
            let workspace = match choice {
                // Fetching the Workspace again records the access, which is
                // what floats it to the top of the picker next time. The
                // summary the picker already holds stands in if the server
                // cannot answer.
                picker::WorkspaceChoice::Existing(workspace) => {
                    Ok(client.workspace(&workspace.id).unwrap_or(workspace))
                }
                picker::WorkspaceChoice::CreateCurrentDirectory => {
                    session::create_workspace(&client, current_directory, None)
                }
            };
            let workspace = match workspace {
                Ok(workspace) => workspace,
                Err(error) => {
                    terminal::restore(&mut term)?;
                    return Err(error);
                }
            };
            terminal = Some(term);
            workspace
        }
    } else {
        let host_path = session::resolve_workspace(cli.workspace.as_deref())?;
        session::workspace_for_host(&client, &host_path)?
    };

    // Build the application and, unless viewing or awaiting the operator's
    // first message, a live session up front so a setup failure is reported
    // plainly before the terminal is taken over.
    let mut app;
    let mut live: Live;
    // The launch inputs this client works with, kept alongside `app` because
    // every rebuilt App adopts them: they belong to the operator's session at
    // the terminal, not to whichever Session is on screen.
    let (standing_selection, mut launch) = standing_launch(&preferences_path, &cli)?;

    if let Some(view) = &view_target {
        // `--view` is explicitly read-only, so this replays the journal rather
        // than going through `open_session`, which would attach to a live
        // interaction if one happened to be serving the Session.
        let id = session::session_id_from_target(view)?;
        (app, live) = session::open_stored(&client, &id)?;
        app.launch.interaction = launch.clone();
    } else {
        let selection = standing_selection;
        let prompt = cli.prompt.join(" ");
        let seed = (!prompt.trim().is_empty()).then_some(prompt.as_str());
        match seed {
            // A trailing prompt is input the operator already gave (as a CLI
            // argument), so it is fine to launch immediately.
            Some(seed) => {
                let (new_app, info) = session::launch_live_session(
                    &client,
                    &launch,
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
            // launched yet either. Existing interactions are reached from the
            // main screen with `a`; there is no separate startup picker.
            None => {
                app = pending_app(selection, launch.clone(), &active_workspace);
                live = Live::Pending;
            }
        }
    }
    refresh_workspace_context(&mut app, &client, &active_workspace);

    let mut terminal = match terminal {
        Some(terminal) => terminal,
        None => terminal::setup()?,
    };

    // Runs until the operator quits. Workspace and Session selection only
    // changes what this client views; server-owned Interactions continue.
    let result = loop {
        let outcome = match event_loop::run(
            &mut terminal,
            &mut app,
            &client,
            &mut live,
            event_loop::RunContext {
                workspace_id: &active_workspace.id,
                standing_launch: &launch,
                preferences_path: &preferences_path,
                config: &config,
            },
        ) {
            Ok(outcome) => outcome,
            Err(error) => break Err(error),
        };
        if let Some(session_id) = event_loop::interaction_stopped_by(&outcome, &live) {
            client.stop_interaction(session_id).ok();
        }
        match outcome {
            RunOutcome::Quit => break Ok(()),
            RunOutcome::OpenWorkspace {
                workspace,
                session_id,
            } => {
                active_workspace = *workspace;
                match session_id {
                    Some(session_id) => match session::open_session(&client, &session_id) {
                        Ok((mut new_app, new_live)) => {
                            new_app.launch.interaction = launch.clone();
                            app = new_app;
                            live = new_live;
                        }
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not open Session {session_id}: {error:#}"
                        ))),
                    },
                    None => {
                        let (selection, standing) = standing_launch(&preferences_path, &cli)?;
                        launch = standing;
                        app = pending_app(selection, launch.clone(), &active_workspace);
                        live = Live::Pending;
                    }
                }
            }
            // Switch only this client's view. In particular, a running
            // interaction for the Session being left remains server-owned.
            RunOutcome::OpenSession(session_id) => {
                match session::open_session(&client, &session_id) {
                    Ok((mut new_app, new_live)) => {
                        new_app.launch.interaction = launch.clone();
                        app = new_app;
                        live = new_live;
                    }
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not open Session {session_id}: {error:#}"
                    ))),
                }
            }
            // Return to the blank start screen. Reset has already stopped the
            // current interaction and returns to the standing launch default.
            RunOutcome::Reset => {
                live = Live::Pending;
                let (selection, standing) = standing_launch(&preferences_path, &cli)?;
                launch = standing;
                app = pending_app(selection, launch.clone(), &active_workspace);
            }
            // A new interaction inherits the context currently being viewed,
            // even when it was reached through the global Session or
            // Interaction picker and therefore belongs to another Workspace.
            // The outgoing interaction remains server-owned and keeps running.
            RunOutcome::NewSession => {
                live = Live::Pending;
                let selection = app.selection.clone();
                // The sandbox policy is part of that inherited context: a new
                // session started from here begins with whatever the operator
                // had built up, rather than resetting to the saved default.
                launch = app.launch.interaction.clone();
                active_workspace =
                    workspace_for_new_session(&app, &active_workspace, &client.list_workspaces()?);
                app = pending_app(selection, launch.clone(), &active_workspace);
            }
        }
        refresh_workspace_context(&mut app, &client, &active_workspace);
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

fn browse_shells(client: &Client) -> Result<()> {
    let live_ids = client
        .list_interactions()?
        .into_iter()
        .map(|interaction| interaction.id)
        .collect::<std::collections::HashSet<_>>();
    let mut sessions = session::all_sessions(client)?
        .into_iter()
        .filter(|session| live_ids.contains(&session.id))
        .collect::<Vec<_>>();
    if sessions.is_empty() {
        println!("No live sessions found by the Styra server");
        return Ok(());
    }

    let mut terminal = terminal::setup()?;
    let choice = picker::run_session_picker(&mut terminal, client, &mut sessions);
    terminal::restore(&mut terminal)?;
    match choice? {
        Some(session) => attach_shell(client, &session),
        None => Ok(()),
    }
}

/// `--daemon`: bring up the background daemon and return.
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

/// `--stop`: ask the daemon on `socket` to shut down.
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

#[cfg(test)]
mod cli_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use styra_server::agent::{Provider, Selection};

    fn workspace(id: &str, host_path: &str) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: None,
            notes: String::new(),
            host_path: host_path.into(),
            git_repository: None,
            path: format!("/store/{id}").into(),
            session_count: 0,
            age: "now".into(),
            created_at_ms: 0,
            last_accessed_at_ms: 0,
            launch: Default::default(),
        }
    }

    #[test]
    fn shell_subcommand_captures_an_explicit_session() {
        let cli = Cli::try_parse_from(["styra", "shell", "--session", "styra-123"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(CliCommand::Shell { session: Some(session) }) if session == "styra-123"
        ));
    }

    #[test]
    fn shell_subcommand_without_a_session_opens_the_browser() {
        let cli = Cli::try_parse_from(["styra", "shell"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(CliCommand::Shell { session: None })
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
    fn quitting_and_switching_detach_but_reset_stops_the_running_interaction() {
        let live = Live::Running {
            session_id: "styra-live".into(),
            cursor: 7,
        };

        assert_eq!(
            event_loop::interaction_stopped_by(&RunOutcome::Quit, &live),
            None
        );
        assert_eq!(
            event_loop::interaction_stopped_by(
                &RunOutcome::OpenSession("styra-other".into()),
                &live,
            ),
            None
        );
        assert_eq!(
            event_loop::interaction_stopped_by(&RunOutcome::Reset, &live),
            Some("styra-live")
        );
        assert_eq!(
            event_loop::interaction_stopped_by(&RunOutcome::NewSession, &live),
            None
        );
    }

    #[test]
    fn new_session_uses_the_workspace_of_the_interaction_being_viewed() {
        let active = workspace("first", "/work/first");
        let viewed = workspace("viewed", "/work/viewed");
        let selection = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
        let mut app = App::pending(selection.clone());
        app.workspace.id = Some(viewed.id.clone());

        let inherited = workspace_for_new_session(&app, &active, &[active.clone(), viewed.clone()]);
        let pending = pending_app(
            selection.clone(),
            app.launch.interaction.clone(),
            &inherited,
        );

        assert_eq!(inherited, viewed);
        assert_eq!(pending.workspace.id.as_deref(), Some("viewed"));
        assert_eq!(pending.workspace.root(), Some(Path::new("/work/viewed")));
        assert_eq!(pending.selection, selection);
    }

    /// A new session started from a Workspace begins on that Workspace's own
    /// standing policy, while the operator's own inputs travel with them.
    #[test]
    fn a_new_session_starts_from_the_policy_of_the_workspace_it_lands_in() {
        let selection = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
        let mut landed_in = workspace("second", "/work/second");
        landed_in.launch = styra_server::LaunchPolicy {
            templates: vec!["rust".into()],
            ..styra_server::LaunchPolicy::default()
        };
        let carried = styra_server::LaunchPolicy {
            templates: vec!["browser".into()],
            ..styra_server::LaunchPolicy::default()
        };

        let pending = pending_app(selection, carried.clone(), &landed_in);

        assert_eq!(pending.launch.workspace, landed_in.launch);
        assert_eq!(pending.launch.interaction, carried);
        assert_eq!(
            pending.launch.effective().templates,
            vec!["rust".to_owned(), "browser".to_owned()]
        );
    }

    #[test]
    fn confirming_the_launcher_selects_for_this_workspace_without_saving_a_default() {
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

        keys::handle_launcher_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &path,
        );

        assert_eq!(app.selection, selection);
        assert!(!path.exists());
        std::fs::remove_dir_all(root).ok();
    }

    /// The picker moves on plain `j`/`k`, the keys its own hint names, and
    /// confirming remembers the model so the next picker lists it first —
    /// without touching the saved defaults, which only `D` writes.
    #[test]
    fn j_and_k_move_the_launcher_and_confirming_remembers_the_model() {
        let root =
            std::env::temp_dir().join(format!("styra-launch-recent-models-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let path = root.join("defaults.json");
        let mut app = App::pending(Selection::parse("claude").expect("valid test selection"));
        app.open_launcher();
        let opened_on = app.launcher.as_ref().unwrap().model;

        // Into the model column, then two rows down and one back up.
        for code in [
            KeyCode::Char('l'),
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
        ] {
            keys::handle_launcher_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE), &path);
        }
        let moved_to = app.launcher.as_ref().unwrap().selection().model;
        assert_eq!(moved_to, Provider::Claude.models()[opened_on + 1]);

        keys::handle_launcher_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &path,
        );
        assert_eq!(app.selection.model, moved_to);
        assert!(!path.exists(), "Enter does not save a default selection");
        assert_eq!(
            preferences::load_recent_models(&path),
            vec![moved_to.clone()]
        );

        // A picker opened with that ordering puts the model at the top.
        app.recent_models = preferences::load_recent_models(&path);
        app.open_launcher();
        let launcher = app.launcher.as_ref().unwrap();
        assert_eq!(launcher.models().first(), Some(&moved_to));
        assert_eq!(launcher.model, 0, "and opens on it");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_launcher_can_explicitly_save_the_selection_as_the_default() {
        let root = std::env::temp_dir().join(format!(
            "styra-launch-default-handler-explicit-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        let path = root.join("defaults.json");
        let selection =
            Selection::parse("claude:claude-sonnet-5/max").expect("valid test selection");
        let mut app = App::pending(selection.clone());
        app.open_launcher();

        keys::handle_launcher_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
            &path,
        );

        assert_eq!(
            preferences::load_or_default(&path).unwrap().selection,
            selection
        );
        std::fs::remove_dir_all(root).ok();
    }
}
