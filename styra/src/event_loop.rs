use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::activity::Status;
use crate::app::{App, Focus, LaunchPolicy, Request};
use crate::config::Config;
use crate::keymap::HELP;
use crate::keys;
use crate::launch::{self, LaunchScope};
use crate::loader::{LoadEvent, Loads};
use crate::notes;
use crate::picker;
use crate::preferences;
use crate::session::{self, Live};
use crate::ui;
use styra_server::{Client, InteractionSnapshotScope, LogEntry, WorkspaceSummary};

/// What the interactive loop returned control to `main` for.
pub enum RunOutcome {
    Quit,
    OpenWorkspace {
        workspace: Box<WorkspaceSummary>,
        session_id: Option<String>,
    },
    OpenSession(String),
    Reset,
    NewSession,
}

const INTERACTIONS_REFRESH: Duration = Duration::from_millis(250);
const INTERACTION_RECENT_UPDATES: usize = 5;

pub struct RunContext<'a> {
    pub workspace_id: &'a str,
    pub standing_launch: &'a LaunchPolicy,
    pub preferences_path: &'a Path,
    pub config: &'a Config,
}

/// Global actions which operate on the current interaction without dismissing
/// its navigator. They fall through to the ordinary list-key handler below.
fn interaction_navigator_passthrough(code: &KeyCode) -> bool {
    matches!(code, KeyCode::Char('i') | KeyCode::Char('S'))
}

/// Apply an incoming interaction payload only when it still belongs to this
/// Styra instance's active view. The generation also rejects an older preview
/// of the same interaction after a full load has been requested.
fn apply_interaction_load(
    app: &mut App,
    live: &mut Live,
    standing_launch: &LaunchPolicy,
    loads: &mut Loads,
    event: LoadEvent,
) {
    if !loads.accepts(&event) {
        return;
    }
    let request_id = Loads::request_id_of(&event).to_owned();
    loads.answered();
    let snapshot = match event.result {
        Ok(snapshot) if loads.matches(&snapshot, &request_id) => snapshot,
        Ok(_) => return,
        Err(error) => {
            loads.settle_on(app.session_id.clone());
            app.interactions.select_id(&app.session_id);
            app.push_log(LogEntry::error(format!(
                "could not load interaction {}: {error:#}",
                event.id
            )));
            return;
        }
    };

    let (mut next, next_live) = session::app_from_interaction_snapshot(snapshot);
    next.adopt(app.take_operator_state());
    next.interactions.select_id(loads.active_id());
    // Only the newest few updates were asked for, so the list starts out with
    // holes an unfiltered view would show as gaps; see DESIGN.md. This is the
    // one display choice a switch overrides.
    next.timeline.conversation_only = true;
    next.launch.interaction = standing_launch.clone();
    if let Some(workspace) = next
        .interactions
        .workspaces
        .iter()
        .find(|workspace| Some(workspace.id.as_str()) == next.workspace.id.as_deref())
    {
        next.workspace.name = Some(session::workspace_display_name(workspace));
        next.launch.set_workspace(workspace.launch.clone());
    }
    next.view = crate::app::View::Events;
    next.focus = Focus::List;
    *app = next;
    *live = next_live;
}

/// Fill the raw history omitted by lightweight list navigation and open the
/// raw view. Rebuilding once here restores event-to-wire indices as well as
/// the lines themselves, so branching and raw selection retain their exact
/// semantics after lazy loading.
fn open_raw_history(
    app: &mut App,
    live: &mut Live,
    client: &Client,
    standing_launch: &LaunchPolicy,
) {
    let id = app.session_id.clone();
    let Some(interaction) = app
        .interactions
        .items
        .iter()
        .find(|interaction| interaction.id == id)
        .cloned()
    else {
        app.push_log(LogEntry::error(
            "could not find the current live interaction",
        ));
        return;
    };
    match session::attach_live_interaction(client, interaction) {
        Ok((mut next, next_live)) => {
            next.adopt(app.take_operator_state());
            next.launch.interaction = standing_launch.clone();
            // The same Interaction in the same Workspace, so its identity is
            // already known and there is nothing to ask the server for.
            next.workspace.name.clone_from(&app.workspace.name);
            next.launch.workspace = app.launch.workspace.clone();
            next.toggle_raw();
            *app = next;
            *live = next_live;
        }
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not load raw history for interaction {id}: {error:#}"
        ))),
    }
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
    live: &mut Live,
    context: RunContext<'_>,
) -> Result<RunOutcome> {
    let RunContext {
        workspace_id,
        standing_launch,
        preferences_path,
        config,
    } = context;
    // The model column's ordering is remembered across runs, so pick it up
    // before the picker can be opened.
    app.recent_models = preferences::load_recent_models(preferences_path);
    // This belongs to the client process, not the server: two Styra instances
    // may browse different interactions and independently reject payloads that
    // are late for their own current selection.
    let (mut loads, interaction_events) = Loads::start(client.clone(), app.session_id.clone());
    let mut pending_fold = false;
    let mut interactions_refreshed = Instant::now();
    loop {
        while let Ok(event) = interaction_events.try_recv() {
            apply_interaction_load(app, live, standing_launch, &mut loads, event);
        }
        app.notices.expire();
        notes::ensure_loaded(app, client, workspace_id);
        // Workspace launch policy is a server-owned read model. Refresh it
        // independently of input so edits from another Styra client flow into
        // this Driva view and invalidate its planned options.
        if let Ok(policy) = client.workspace_launch(workspace_id) {
            if policy != app.launch.workspace {
                app.launch.sync_workspace(policy);
            }
        }
        session::ensure_driva_plan(app, client, workspace_id);
        let mut disconnected = false;
        if let Live::Running { session_id, cursor } = live {
            if loads.is_on(session_id) {
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
        }
        if disconnected {
            *live = Live::Viewing;
        }

        if app.interactions.open && interactions_refreshed.elapsed() >= INTERACTIONS_REFRESH {
            interactions_refreshed = Instant::now();
            if let Ok(interactions) = client.list_interactions() {
                let current = app.session_id.clone();
                let workspace_id = app.workspace.id.clone();
                app.interactions
                    .refresh(interactions, &current, workspace_id.as_deref());
            }
        }

        if let Live::Running { session_id, .. } = live {
            if loads.is_on(session_id) && app.activity.status == Status::Idle {
                if let Some(message) = app.outbox.take_queued() {
                    // Sent as it was composed: a message queued asking for a
                    // shape still asks for it when the agent frees up.
                    let turn = session::turn(&message.text, &app.selection, message.contract);
                    match client.send_turn(session_id, turn) {
                        Ok(()) => {
                            app.activity.status = Status::Running;
                            let waiting = app.outbox.queued_count();
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
                            app.outbox.queue(message);
                            app.push_log(LogEntry::error(format!("queued send failed: {error:#}")));
                        }
                    }
                }
            }
        }

        app.activity.note_progress();
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
        // So is the message editor's path prompt, whose second question is
        // answered by a bare letter that means something else everywhere else.
        if app.insert.is_some() {
            keys::handle_insert_key(app, key);
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

        // The embedded interaction list owns navigation while it is open.
        // Moving its cursor changes the current timeline immediately with a
        // short conversation tail. Enter confirms it by loading the complete
        // interaction before closing the navigator.
        if app.interactions.open && app.focus == Focus::List {
            match key.code {
                KeyCode::Char('a') | KeyCode::Esc => {
                    app.interactions.open = false;
                    continue;
                }
                KeyCode::Enter => {
                    if let Some(interaction) = app.interactions.selected().cloned() {
                        app.interactions.open = false;
                        loads.request(interaction.id, InteractionSnapshotScope::Full);
                    }
                    continue;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    app.interactions.select_next(app.workspace.id.as_deref());
                    if let Some(interaction) = app.interactions.selected().cloned() {
                        loads.request(
                            interaction.id,
                            InteractionSnapshotScope::Preview {
                                limit: INTERACTION_RECENT_UPDATES,
                            },
                        );
                    }
                    continue;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.interactions
                        .select_previous(app.workspace.id.as_deref());
                    if let Some(interaction) = app.interactions.selected().cloned() {
                        loads.request(
                            interaction.id,
                            InteractionSnapshotScope::Preview {
                                limit: INTERACTION_RECENT_UPDATES,
                            },
                        );
                    }
                    continue;
                }
                KeyCode::Char('w') => {
                    let current = app.session_id.clone();
                    let workspace_id = app.workspace.id.clone();
                    app.interactions
                        .toggle_workspace_scope(&current, workspace_id.as_deref());
                    continue;
                }
                KeyCode::Char('D') => {
                    let Some(interaction) = app.interactions.selected().cloned() else {
                        continue;
                    };
                    if interaction.accepting {
                        app.show_action_message("only stopped interactions can be deleted");
                        continue;
                    }
                    if let Err(error) = client.close_interaction(&interaction.id) {
                        app.push_log(LogEntry::error(format!(
                            "could not delete interaction {}: {error:#}",
                            interaction.id
                        )));
                        continue;
                    }
                    let workspace_id = app.workspace.id.clone();
                    let Some(next) = app
                        .interactions
                        .remove_and_select_next(&interaction.id, workspace_id.as_deref())
                    else {
                        app.interactions.open = false;
                        return Ok(RunOutcome::Reset);
                    };
                    loads.request(
                        next.id,
                        InteractionSnapshotScope::Preview {
                            limit: INTERACTION_RECENT_UPDATES,
                        },
                    );
                    continue;
                }
                code if interaction_navigator_passthrough(&code) => {}
                _ => app.interactions.open = false,
            }
        }

        // Selection changes the local active id immediately, before its
        // asynchronous payload has rebuilt `app`. Do not let a fast follow-up
        // key act on the interaction that this instance has just left.
        if !loads.is_on(&app.session_id) {
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
                        session::create_workspace(client, host_path, None)?
                    }
                };
                let mut sessions = client.list_sessions(&workspace.id)?;
                if sessions.is_empty() {
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace: Box::new(workspace),
                        session_id: None,
                    });
                }
                if let Some(id) = picker::run_session_picker(terminal, client, &mut sessions)? {
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace: Box::new(workspace),
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
                let interactions = client.list_interactions()?;
                if interactions.is_empty() {
                    app.push_log(LogEntry::warn("no live interactions on the server"));
                    continue;
                }
                let current = app.session_id.clone();
                let workspaces = client.list_workspaces()?;
                app.view = crate::app::View::Events;
                app.focus = Focus::List;
                app.interactions.open(interactions, workspaces, &current);
                loads.settle_on(current);
                interactions_refreshed = Instant::now();
            }
            Some(Request::Raw) => {
                loads.settle_on(app.session_id.clone());
                open_raw_history(app, live, client, standing_launch);
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
            Some(Request::ChangeWorkspaceLaunch {
                change,
                clear_interaction,
            }) => match client.change_workspace_launch(workspace_id, change) {
                Ok(policy) if clear_interaction => {
                    app.launch.adopt_workspace(policy);
                    app.show_action_message(
                        "moved into this Workspace's policy — every launch here starts from it",
                    );
                }
                Ok(policy) => app.launch.sync_workspace(policy),
                Err(error) => app.push_log(LogEntry::error(format!(
                    "could not change the Workspace launch policy: {error:#}"
                ))),
            },
            // Parsing is the server's, since it holds the session's recorded
            // contract and the journal the answer is read from; this client
            // only asks and renders.
            Some(Request::Answer { contract }) => {
                let id = app.session_id.clone();
                if id.is_empty() {
                    app.answer.set(Err("no session to answer from yet".into()));
                    continue;
                }
                let answer = match contract {
                    Some(contract) => client.turn_answer_as(&id, contract),
                    None => client.turn_answer(&id),
                };
                app.answer.set(answer.map_err(|error| format!("{error:#}")));
            }
            // The quota log is the server's: it reads the figures off every
            // interaction's wire, so one client asking gets every session's
            // readings rather than only this one's.
            Some(Request::Quota) => match client.quota_log() {
                Ok(readings) => app.quota.replace(readings),
                Err(error) => app.push_log(LogEntry::error(format!(
                    "could not read the quota log: {error:#}"
                ))),
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader;
    use std::path::PathBuf;
    use styra_server::protocol::{SequencedUpdate, Updates};
    use styra_server::InteractionSnapshot;
    use styra_server::{DrivaOptions, InteractionActivity, InteractionSummary, InteractionUpdate};

    fn app(id: &str) -> App {
        App::new(styra_server::agent::Selection::parse("codex").unwrap(), id)
    }

    fn snapshot(id: &str, scope: InteractionSnapshotScope) -> InteractionSnapshot {
        InteractionSnapshot {
            request_id: "request-3".into(),
            interaction: InteractionSummary {
                id: id.into(),
                name: Some("target session".into()),
                workspace_id: "workspace".into(),
                selection: styra_server::agent::Selection::parse("codex").unwrap(),
                workspace: PathBuf::from("/workspace"),
                driva: DrivaOptions {
                    isolation_backend: "none".into(),
                    command: vec![],
                    working_directory: PathBuf::from("/workspace"),
                    network: false,
                    mounts: vec![],
                },
                accepting: true,
                activity: InteractionActivity::Running,
                last_message: Some("payload body".into()),
            },
            background_work: false,
            updates: Updates {
                updates: vec![SequencedUpdate {
                    sequence: 12,
                    update: InteractionUpdate::Event(
                        styra_server::event::AgentEvent::AgentMessage {
                            text: "payload body".into(),
                        },
                    ),
                }],
                next: 12,
            },
            queued: vec![],
            scope,
        }
    }

    #[test]
    fn matching_payload_populates_the_main_view_without_a_navigator() {
        let mut app = app("before");
        let mut live = Live::Viewing;
        let mut loads = loader::waiting_for("target", "request-3", 3);

        apply_interaction_load(
            &mut app,
            &mut live,
            &LaunchPolicy::default(),
            &mut loads,
            loader::load_event(
                "request-3",
                3,
                "target",
                Ok(snapshot(
                    "target",
                    InteractionSnapshotScope::Preview { limit: 5 },
                )),
            ),
        );

        assert_eq!(app.session_id, "target");
        assert_eq!(app.timeline.entries.len(), 1);
        assert!(app.raw.needs_hydration());
        assert_eq!(
            live,
            Live::Running {
                session_id: "target".into(),
                cursor: 12,
            }
        );
        assert!(!app.interactions.open);
        assert!(!loads.is_waiting());
    }

    #[test]
    fn incoming_interaction_payloads_are_scoped_to_this_clients_active_view() {
        let mut app = app("current");
        let mut live = Live::Viewing;
        let mut loads = loader::waiting_for("current", "request-4", 4);

        apply_interaction_load(
            &mut app,
            &mut live,
            &LaunchPolicy::default(),
            &mut loads,
            loader::load_event(
                "request-4",
                4,
                "another-clients-view",
                Err(anyhow::anyhow!("must never reach the current log")),
            ),
        );

        assert_eq!(app.session_id, "current");
        assert!(app.log.is_empty());
        assert_eq!(loads.active_id(), "current");
    }

    #[test]
    fn an_older_preview_cannot_replace_a_newer_load_of_the_same_interaction() {
        let mut app = app("before");
        let mut live = Live::Viewing;
        let mut loads = loader::waiting_for("target", "request-8", 8);

        apply_interaction_load(
            &mut app,
            &mut live,
            &LaunchPolicy::default(),
            &mut loads,
            loader::load_event(
                "request-7",
                7,
                "target",
                Err(anyhow::anyhow!("obsolete preview")),
            ),
        );

        assert_eq!(app.session_id, "before");
        assert!(app.log.is_empty());
        assert_eq!(loads.active_id(), "target");
    }

    #[test]
    fn stopping_is_a_navigator_passthrough_action() {
        assert!(interaction_navigator_passthrough(&KeyCode::Char('S')));
        assert!(interaction_navigator_passthrough(&KeyCode::Char('i')));
        assert!(!interaction_navigator_passthrough(&KeyCode::Char('l')));
    }
}
