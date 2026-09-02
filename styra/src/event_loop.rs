use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::app::{App, Focus, LaunchPolicy, Request, Status};
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
        workspace: Box<WorkspaceSummary>,
        session_id: Option<String>,
    },
    OpenSession(String),
    Reset,
    NewSession,
}

const INTERACTIONS_REFRESH: Duration = Duration::from_millis(250);

pub struct RunContext<'a> {
    pub workspace_id: &'a str,
    pub standing_launch: &'a LaunchPolicy,
    pub preferences_path: &'a Path,
    pub config: &'a Config,
}

/// Make `interaction` the screen's current session without leaving the main
/// event loop. The navigator and operator-owned display choices survive; the
/// interaction timeline itself is rebuilt from the server exactly as it was
/// when the old standalone picker returned a selection.
fn make_interaction_current(
    app: &mut App,
    live: &mut Live,
    client: &Client,
    standing_launch: &LaunchPolicy,
    interaction: InteractionSummary,
) {
    if interaction.id == app.session_id {
        return;
    }
    let id = interaction.id.clone();
    match session::attach_live_interaction(client, interaction) {
        Ok((mut next, next_live)) => {
            next.interactions = std::mem::take(&mut app.interactions);
            next.interactions.select_id(&id);
            next.timeline.conversation_only = app.timeline.conversation_only;
            next.show_preview = app.show_preview;
            next.preview_mode = app.preview_mode;
            next.preview_target = app.preview_target;
            next.recent_models = app.recent_models.clone();
            next.launch.interaction = standing_launch.clone();
            if let Some(workspace) = client.list_workspaces().ok().and_then(|workspaces| {
                workspaces
                    .into_iter()
                    .find(|workspace| Some(workspace.id.as_str()) == next.workspace_id.as_deref())
            }) {
                next.workspace_name = Some(session::workspace_display_name(&workspace));
                next.launch.set_workspace(workspace.launch);
            }
            next.view = crate::app::View::Events;
            next.focus = Focus::List;
            *app = next;
            *live = next_live;
        }
        Err(error) => {
            app.interactions.select_id(&app.session_id);
            app.push_log(LogEntry::error(format!(
                "could not make interaction {id} current: {error:#}"
            )));
        }
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
    let mut pending_fold = false;
    let mut interactions_refreshed = Instant::now();
    loop {
        app.expire_action_messages();
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

        if app.interactions.open && interactions_refreshed.elapsed() >= INTERACTIONS_REFRESH {
            interactions_refreshed = Instant::now();
            if let Ok(interactions) = client.list_interactions() {
                let current = app.session_id.clone();
                let workspace_id = app.workspace_id.clone();
                app.interactions
                    .refresh(interactions, &current, workspace_id.as_deref());
            }
        }

        if let Live::Running { session_id, .. } = live {
            if app.status == Status::Idle {
                if let Some(message) = app.take_queued_message() {
                    // Sent as it was composed: a message queued asking for a
                    // shape still asks for it when the agent frees up.
                    let turn = session::turn(&message.text, &app.selection, message.contract);
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
        // Moving its cursor changes the current timeline immediately; Enter
        // merely closes it because there is no separate attach confirmation.
        if app.interactions.open && app.focus == Focus::List {
            match key.code {
                KeyCode::Char('a') | KeyCode::Esc | KeyCode::Enter => {
                    app.interactions.open = false;
                    continue;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    app.interactions.select_next(app.workspace_id.as_deref());
                    if let Some(interaction) = app.interactions.selected().cloned() {
                        make_interaction_current(app, live, client, standing_launch, interaction);
                    }
                    continue;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.interactions
                        .select_previous(app.workspace_id.as_deref());
                    if let Some(interaction) = app.interactions.selected().cloned() {
                        make_interaction_current(app, live, client, standing_launch, interaction);
                    }
                    continue;
                }
                KeyCode::Char('w') => {
                    let current = app.session_id.clone();
                    let workspace_id = app.workspace_id.clone();
                    app.interactions
                        .toggle_workspace_scope(&current, workspace_id.as_deref());
                    continue;
                }
                KeyCode::Char('i') => {}
                _ => app.interactions.open = false,
            }
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
                app.view = crate::app::View::Events;
                app.focus = Focus::List;
                app.interactions.open(interactions, &current);
                interactions_refreshed = Instant::now();
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
                    app.set_answer(Err("no session to answer from yet".into()));
                    continue;
                }
                let answer = match contract {
                    Some(contract) => client.turn_answer_as(&id, contract),
                    None => client.turn_answer(&id),
                };
                app.set_answer(answer.map_err(|error| format!("{error:#}")));
            }
            // The quota log is the server's: it reads the figures off every
            // interaction's wire, so one client asking gets every session's
            // readings rather than only this one's.
            Some(Request::Quota) => match client.quota_log() {
                Ok(readings) => app.set_quota(readings),
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
