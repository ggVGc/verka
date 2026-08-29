use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::Path;
use std::time::Duration;

use crate::app::{App, Focus, Request, Status};
use crate::config::Config;
use crate::keymap::HELP;
use crate::keys;
use crate::launch::{self, LaunchScope};
use crate::notes;
use crate::picker;
use crate::preferences;
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
    workspace_id: &str,
    live: &mut Live,
    preferences_path: &Path,
    config: &Config,
) -> Result<RunOutcome> {
    // The model column's ordering is remembered across runs, so pick it up
    // before the picker can be opened.
    app.recent_models = preferences::load_recent_models(preferences_path);
    let mut pending_fold = false;
    loop {
        app.expire_action_messages();
        notes::ensure_loaded(app, client, workspace_id);
        session::ensure_driva_plan(app, client, workspace_id);
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
                    // Sent as it was composed: a message queued asking for a
                    // shape still asks for it when the agent frees up.
                    let turn = session::turn(&message.text, app, message.contract);
                    match client.send_turn(session_id, turn) {
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

        app.note_progress();
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
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                    app.show_keybinds = false;
                    app.keybinds_scroll = 0;
                }
                // The reference is taller than a short terminal, so the
                // sections at the end have to be reachable.
                KeyCode::Char('j') | KeyCode::Down => app.scroll_keybinds(1),
                KeyCode::Char('k') | KeyCode::Up => app.scroll_keybinds(-1),
                KeyCode::PageDown => app.scroll_keybinds(10),
                KeyCode::PageUp => app.scroll_keybinds(-10),
                KeyCode::Char('g') => app.keybinds_scroll = 0,
                _ => {}
            }
            continue;
        }
        // The notes editor is modal, and everything printable typed into it is
        // note text — including `?`, so it is handled ahead of the reference.
        if app.notes.is_open() {
            notes::handle_key(app, client, key);
            continue;
        }
        // So is the Driva view's mount prompt: what is typed into it is part
        // of a path, including the characters that are shortcuts elsewhere.
        if app.launch.prompt.is_some() {
            keys::handle_mount_prompt_key(app, key);
            continue;
        }
        // In input focus, `?` is message text rather than a shortcut.
        if app.focus == Focus::List && key.code == KeyCode::Char(HELP.chars().next().unwrap()) {
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
                Focus::List => keys::handle_list_key(
                    app,
                    client,
                    live,
                    key,
                    &mut pending_fold,
                    preferences_path,
                ),
                Focus::Input => keys::handle_input_key(app, client, workspace_id, live, key),
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
                    // Looking the Workspace up again records the access, which
                    // is what floats it to the top of the picker next time. The
                    // summary the picker already holds stands in if the server
                    // cannot answer.
                    picker::WorkspaceChoice::Existing(workspace) => {
                        client.workspace(&workspace.id).unwrap_or(workspace)
                    }
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
            Some(Request::OpenSession(id)) => return Ok(RunOutcome::OpenSession(id)),
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
            Some(Request::Templates) => {
                let templates = match client.list_templates(workspace_id) {
                    Ok(templates) => templates,
                    Err(error) => {
                        app.push_log(LogEntry::error(format!(
                            "could not list Driva templates: {error:#}"
                        )));
                        continue;
                    }
                };
                if templates.is_empty() {
                    app.push_log(LogEntry::warn("no Driva templates are available"));
                    continue;
                }
                // Which templates the picker starts from is the layer being
                // edited. For the Workspace's own that is exactly its list. For
                // this interaction's, the picker offers and returns the whole
                // layering a launch would apply — the Workspace's templates
                // included, since those are as much part of what the operator is
                // choosing as their own. Turning that choice back into an
                // overlay is `App`'s job.
                let current = match app.launch.scope {
                    LaunchScope::Workspace => app.launch.workspace.templates.clone(),
                    LaunchScope::Interaction => app.launch.effective().templates,
                };
                let chosen = picker::run_template_picker(terminal, &templates, &current)?;
                if let Some(chosen) = chosen {
                    launch::set_templates(app, chosen);
                }
            }
            Some(Request::StoreWorkspaceLaunch { announce }) => {
                // The Workspace's own layer, as edited in the driva view, sent
                // to the server that owns it. Raised by each such edit, so the
                // policy on screen is the policy the next launch merges — the
                // launch paths send only the overlay and read this from the
                // Workspace, so a change kept here alone would not be applied.
                let policy = app.launch.workspace.clone();
                match client.set_workspace_launch(workspace_id, &policy) {
                    Ok(workspace) => {
                        app.launch.workspace_stored(workspace.launch);
                        if announce {
                            app.show_action_message(
                                "stored with the Workspace — every launch here starts from it",
                            );
                        }
                    }
                    // The edit stays on screen, marked as not stored: it is what
                    // the operator asked for, and `W` retries. What it is not is
                    // part of the effective policy, which the view says.
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not save the Workspace launch policy: {error:#}"
                    ))),
                }
            }
            Some(Request::PromoteLaunchToWorkspace) => {
                // What is stored is the merge, not the overlay: the operator is
                // keeping the policy the view shows. The overlay is then
                // redundant and `Launch::adopt_workspace` clears it, so the
                // effective policy is unchanged by the act of storing it.
                let policy = app.launch.effective();
                match client.set_workspace_launch(workspace_id, &policy) {
                    Ok(workspace) => {
                        app.launch.adopt_workspace(workspace.launch);
                        app.show_action_message(
                            "moved into this Workspace's policy — every launch here starts from it",
                        );
                    }
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not save the Workspace launch policy: {error:#}"
                    ))),
                }
            }
            // Parsing is the server's, since it holds the session's recorded
            // contract and the journal the answer is read from; this client
            // only asks and renders.
            Some(Request::Answer { contract }) => {
                let id = app.session_id.clone();
                if id.is_empty() {
                    app.set_answer(Err("no session to answer from yet".into()));
                    continue;
                }
                let answer = match contract {
                    Some(contract) => client.turn_answer_as(&id, contract),
                    None => client.turn_answer(&id),
                };
                app.set_answer(answer.map_err(|error| format!("{error:#}")));
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
