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
mod app;
mod cli;
mod event_loop;
mod keys;
mod picker;
mod preferences;
mod session;
mod terminal;
mod ui;

use app::App;
use cli::{Cli, CliCommand};
use event_loop::RunOutcome;
use session::Live;
use styra_server::{Client, LogEntry, WorkspaceSummary};

fn refresh_workspace_name(app: &mut App, client: &Client, active: &WorkspaceSummary) {
    let workspace = app
        .workspace_id
        .as_deref()
        .filter(|id| *id == active.id)
        .map(|_| active.clone())
        .or_else(|| {
            let id = app.workspace_id.as_deref()?;
            client
                .list_workspaces()
                .ok()?
                .into_iter()
                .find(|workspace| workspace.id == id)
        });
    app.workspace_name = workspace.as_ref().map(session::workspace_display_name);
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
            let sessions = session::all_sessions(&client)?;
            if sessions.is_empty() {
                println!("No sessions found by the Styra server");
                return Ok(());
            }
            let mut term = terminal::setup()?;
            match picker::run_session_picker(&mut term, &client, &sessions) {
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
        let workspaces = client.list_workspaces()?;
        if let Some(workspace) = session::find_workspace_for_host(&workspaces, &current_directory) {
            workspace
        } else {
            let mut term = match terminal.take() {
                Some(term) => term,
                None => terminal::setup()?,
            };
            let choice = match picker::run_workspace_picker(&mut term, &workspaces) {
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
                // Keep the summary returned by the list request. Fetching it
                // again records an access on the server, which makes the same
                // picker immediately reorder when it is reopened with `V`.
                picker::WorkspaceChoice::Existing(workspace) => Ok(workspace),
                picker::WorkspaceChoice::CreateCurrentDirectory => {
                    client.create_workspace(&styra_server::protocol::CreateWorkspace {
                        host_path: current_directory,
                        name: None,
                    })
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

    if let Some(view) = &view_target {
        // `--view` is explicitly read-only, so this replays the journal rather
        // than going through `open_session`, which would attach to a live
        // interaction if one happened to be serving the Session.
        let id = session::session_id_from_target(view)?;
        (app, live) = session::open_stored(&client, &id)?;
    } else {
        let selection = preferences::load_or_default(&preferences_path)?;
        let prompt = cli.prompt.join(" ");
        let seed = (!prompt.trim().is_empty()).then_some(prompt.as_str());
        match seed {
            // A trailing prompt is input the operator already gave (as a CLI
            // argument), so it is fine to launch immediately.
            Some(seed) => {
                let (new_app, info) = session::launch_live_session(
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
    refresh_workspace_name(&mut app, &client, &active_workspace);

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
            &cli,
            &active_workspace.id,
            &mut live,
            &preferences_path,
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
                active_workspace = workspace;
                match session_id {
                    Some(session_id) => match session::open_session(&client, &session_id) {
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
            // Switch only this client's view. In particular, a running
            // interaction for the Session being left remains server-owned.
            RunOutcome::OpenSession(session_id) => {
                match session::open_session(&client, &session_id) {
                    Ok((new_app, new_live)) => {
                        app = new_app;
                        live = new_live;
                    }
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not open Session {session_id}: {error:#}"
                    ))),
                }
            }
            // Attach to another live interaction. The outgoing one is left running on
            // the server (interactions outlive a client); we just stop viewing it.
            RunOutcome::Attach(interaction) => {
                let id = interaction.id.clone();
                match session::attach_live_interaction(&client, interaction) {
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
        refresh_workspace_name(&mut app, &client, &active_workspace);
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
    use styra_server::agent::Selection;

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
    fn quitting_and_switching_detach_but_reset_stops_the_running_interaction() {
        let live = Live::Running {
            session_id: "styra-live".into(),
            cursor: 7,
        };

        assert_eq!(event_loop::interaction_stopped_by(&RunOutcome::Quit, &live), None);
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

        assert_eq!(preferences::load_or_default(&path).unwrap(), selection);
        std::fs::remove_dir_all(root).ok();
    }
}
