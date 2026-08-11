use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::Path;
use std::time::Duration;

use crate::app::{App, Focus, Request, Status};
use crate::cli::Cli;
use crate::config::Config;
use crate::keys;
use crate::notes;
use crate::picker;
use crate::session::{self, Live};
use crate::ui;
use styra_server::{Client, InteractionSummary, LogEntry, WorkspaceSummary};

/// What the interactive loop returned control to `main` for.
pub enum RunOutcome {
    Quit,
    OpenWorkspace {
        workspace: WorkspaceSummary,
        session_id: Option<String>,
    },
    OpenSession(String),
    Attach(InteractionSummary),
    Reset,
    NewSession,
}

/// Return the running interaction an in-client transition explicitly stops.
pub fn interaction_stopped_by<'a>(outcome: &RunOutcome, live: &'a Live) -> Option<&'a str> {
    match (outcome, live) {
        (RunOutcome::Reset, Live::Running { session_id, .. }) => Some(session_id),
        _ => None,
    }
}

/// The event loop: apply pending session updates, render, and handle input
/// until the operator quits or asks to switch sessions.
pub fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    live: &mut Live,
    preferences_path: &Path,
    config: &Config,
) -> Result<RunOutcome> {
    let mut pending_fold = false;
    loop {
        app.expire_action_messages();
        notes::ensure_loaded(app, client, workspace_id);
        let mut disconnected = false;
        if let Live::Running { session_id, cursor } = live {
            match client.updates(session_id, *cursor) {
                Ok(batch) => {
                    *cursor = batch.next;
                    for sequenced in batch.updates {
                        session::apply_update(app, sequenced.update);
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

        if let Live::Running { session_id, .. } = live {
            if app.status == Status::Idle {
                if let Some(message) = app.take_queued_message() {
                    match client.send_message_with_selection(session_id, &message, &app.selection) {
                        Ok(()) => {
                            app.status = Status::Running;
                            let waiting = app.queued_message_count();
                            app.show_action_message(if waiting == 0 {
                                "sent queued message automatically".into()
                            } else {
                                format!(
                                    "sent queued message automatically ({waiting} still waiting)"
                                )
                            });
                            if let Err(error) = client.take_queued_message(session_id) {
                                app.push_log(LogEntry::error(format!(
                                    "could not clear sent message from the durable queue: {error:#}"
                                )));
                            }
                        }
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

        // While the reference is open it is modal, so none of the commands
        // described by it can accidentally act on the session underneath.
        if app.show_keybinds {
            if matches!(
                key.code,
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
            ) {
                app.show_keybinds = false;
            }
            continue;
        }
        // The notes editor is modal, and everything printable typed into it is
        // note text — including `?`, so it is handled ahead of the reference.
        if app.notes.is_open() {
            notes::handle_key(app, client, key);
            continue;
        }
        // In input focus, `?` is message text rather than a shortcut.
        if app.focus == Focus::List && key.code == KeyCode::Char('?') {
            app.show_keybinds = true;
            continue;
        }

        // The picker raises requests of its own (applying a model change to the
        // live session), so it falls through to the request match below rather
        // than skipping straight to the next frame.
        if app.launcher.is_some() {
            keys::handle_launcher_key(app, key, preferences_path);
        } else {
            match app.focus {
                Focus::List => keys::handle_list_key(app, client, live, key, &mut pending_fold),
                Focus::Input => keys::handle_input_key(app, client, cli, workspace_id, live, key),
            }
        }

        // A picker that the operator backs out of leaves the session as it was,
        // so those arms fall through to the next frame rather than returning.
        match app.take_request() {
            None => {}
            Some(Request::Quit) => return Ok(RunOutcome::Quit),
            Some(Request::Workspace) => {
                let mut workspaces = client.list_workspaces()?;
                let Some(choice) = picker::run_workspace_picker(terminal, client, &mut workspaces)?
                else {
                    continue;
                };
                let workspace = match choice {
                    // The picker already has a complete summary. Looking it up
                    // again records an access and changes the ordering the next
                    // time this same view is opened.
                    picker::WorkspaceChoice::Existing(workspace) => workspace,
                    picker::WorkspaceChoice::CreateCurrentDirectory => {
                        let host_path = session::resolve_workspace(None)?;
                        client.create_workspace(&styra_server::protocol::CreateWorkspace {
                            host_path,
                            name: None,
                        })?
                    }
                };
                let mut sessions = client.list_sessions(&workspace.id)?;
                if sessions.is_empty() {
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace,
                        session_id: None,
                    });
                }
                if let Some(id) = picker::run_session_picker(terminal, client, &mut sessions)? {
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace,
                        session_id: Some(id),
                    });
                }
            }
            Some(Request::Sessions) => {
                let mut sessions = client.list_sessions(workspace_id)?;
                if sessions.is_empty() {
                    app.push_log(LogEntry::warn("no sessions found in the current Workspace"));
                    continue;
                }
                if let Some(id) = picker::run_session_picker(terminal, client, &mut sessions)? {
                    return Ok(RunOutcome::OpenSession(id));
                }
            }
            Some(Request::Interactions) => {
                let mut interactions = client.list_interactions()?;
                if interactions.is_empty() {
                    app.push_log(LogEntry::warn("no live interactions on the server"));
                    continue;
                }
                let workspaces = client.list_workspaces()?;
                if let Some(interaction) = picker::run_interactions_picker(
                    terminal,
                    client,
                    &mut interactions,
                    &workspaces,
                )? {
                    return Ok(RunOutcome::Attach(interaction));
                }
            }
            Some(Request::Reset) => return Ok(RunOutcome::Reset),
            Some(Request::NewSession) => return Ok(RunOutcome::NewSession),
            Some(Request::ApplySelection) => {
                let Live::Running { session_id, .. } = live else {
                    continue;
                };
                let selection = app.selection.clone();
                match client.set_session_selection(session_id, &selection) {
                    Ok(()) => app.show_action_message(format!("model set to {}", selection.model)),
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not switch to {}: {error:#}",
                        selection.model
                    ))),
                }
            }
            Some(Request::EditFile) => {
                let Some(path) = app.selected_file_path() else {
                    continue;
                };
                match crate::terminal::open_editor(&config.terminal, &config.editor, &path) {
                    Ok(()) => app.show_action_message(format!(
                        "opened {} in {} ({})",
                        path.display(),
                        config.editor,
                        config.terminal
                    )),
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not open {} in {} using {}: {error:#}",
                        path.display(),
                        config.editor,
                        config.terminal
                    ))),
                }
            }
        }
    }
}
