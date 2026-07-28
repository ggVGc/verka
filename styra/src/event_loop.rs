use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::Path;
use std::time::Duration;

use crate::app::{App, Focus, Status};
use crate::cli::Cli;
use crate::keys;
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
    Attach(InteractionSummary),
    Reset,
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
) -> Result<RunOutcome> {
    let mut pending_fold = false;
    loop {
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
                    match client.send_message(session_id, &message) {
                        Ok(()) => {
                            app.status = Status::Running;
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

        if app.launcher.is_some() {
            keys::handle_launcher_key(app, key, preferences_path);
            continue;
        }

        match app.focus {
            Focus::List => keys::handle_list_key(app, client, live, key, &mut pending_fold),
            Focus::Input => keys::handle_input_key(app, client, cli, workspace_id, live, key),
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
        }

        if std::mem::take(&mut app.interactions_requested) {
            let interactions = client.list_interactions()?;
            if interactions.is_empty() {
                app.push_log(LogEntry::warn("no live interactions on the server"));
                continue;
            }
            if let Some(interaction) =
                picker::run_interactions_picker(terminal, client, &interactions)?
            {
                return Ok(RunOutcome::Attach(interaction));
            }
        }

        if std::mem::take(&mut app.reset_requested) {
            return Ok(RunOutcome::Reset);
        }
    }
}
