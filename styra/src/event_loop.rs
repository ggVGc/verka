use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::activity::Status;
use crate::app::{App, Focus, LaunchPolicy, Request};
use crate::config::Config;
use crate::keymap::HELP;
use crate::keys;
use crate::launch::{self, LaunchScope};
use crate::picker;
use crate::preferences;
use crate::session::{self, Attachment};
use crate::ui;
use styra_server::{
    Client, InteractionSummary, LogEntry, TemplateSummary, WorkspaceLaunchChange, WorkspaceSummary,
};

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

/// Launch-policy work is serialized off the terminal thread. Serialization
/// preserves the operator's edit order; the channel back to the root loop lets
/// it keep rendering and consuming input while the daemon or filesystem is
/// slow.
enum LaunchEffect {
    ListTemplates {
        request_id: u64,
        workspace_id: String,
    },
    ChangeWorkspace {
        workspace_id: String,
        change: WorkspaceLaunchChange,
        clear_interaction: bool,
    },
}

enum LaunchEffectResult {
    Templates {
        request_id: u64,
        workspace_id: String,
        result: std::result::Result<Vec<TemplateSummary>, String>,
    },
    WorkspaceChanged {
        workspace_id: String,
        clear_interaction: bool,
        result: std::result::Result<LaunchPolicy, String>,
    },
}

struct LaunchEffects {
    send: Option<Sender<LaunchEffect>>,
    receive: Receiver<LaunchEffectResult>,
    worker: Option<thread::JoinHandle<()>>,
    next_template_request: AtomicU64,
}

impl LaunchEffects {
    fn new(client: Client) -> Self {
        let (send, jobs) = mpsc::channel();
        let (results, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("styra-launch-effects".into())
            .spawn(move || {
                while let Ok(effect) = jobs.recv() {
                    let response = match effect {
                        LaunchEffect::ListTemplates {
                            request_id,
                            workspace_id,
                        } => {
                            let result = client
                                .list_templates(&workspace_id)
                                .map_err(|error| format!("{error:#}"));
                            LaunchEffectResult::Templates {
                                request_id,
                                workspace_id,
                                result,
                            }
                        }
                        LaunchEffect::ChangeWorkspace {
                            workspace_id,
                            change,
                            clear_interaction,
                        } => {
                            let result = client
                                .change_workspace_launch(&workspace_id, change)
                                .map_err(|error| format!("{error:#}"));
                            LaunchEffectResult::WorkspaceChanged {
                                workspace_id,
                                clear_interaction,
                                result,
                            }
                        }
                    };
                    if results.send(response).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning launch-effect worker");
        Self {
            send: Some(send),
            receive,
            worker: Some(worker),
            next_template_request: AtomicU64::new(1),
        }
    }

    fn submit(&self, effect: LaunchEffect) {
        // The receiver lives for the duration of the event loop. A panic here
        // means the worker itself died, which is not a recoverable UI error.
        self.send
            .as_ref()
            .expect("launch-effect sender missing")
            .send(effect)
            .expect("launch-effect worker stopped");
    }

    fn submit_templates(&self, workspace_id: String) -> u64 {
        let request_id = self.next_template_request.fetch_add(1, Ordering::Relaxed);
        self.submit(LaunchEffect::ListTemplates {
            request_id,
            workspace_id,
        });
        request_id
    }

    fn apply_ready(&self, app: &mut App, workspace_id: &str) {
        while let Ok(response) = self.receive.try_recv() {
            match response {
                LaunchEffectResult::Templates {
                    request_id,
                    workspace_id: answered_for,
                    result,
                } => {
                    let current_picker = app.template_picker.as_ref().is_some_and(|picker| {
                        picker.request_id == request_id
                            && picker.workspace_id == answered_for
                            && workspace_id == answered_for
                    });
                    if !current_picker {
                        continue;
                    }
                    match result {
                        Ok(templates) if templates.is_empty() => {
                            app.template_picker = None;
                            app.push_log(LogEntry::warn("no Driva templates are available"));
                        }
                        Ok(templates) => {
                            if let Some(picker) = app.template_picker.as_mut() {
                                picker.loaded(templates);
                            }
                        }
                        Err(error) => {
                            app.template_picker = None;
                            app.push_log(LogEntry::error(format!(
                                "could not list Driva templates: {error}"
                            )));
                        }
                    }
                }
                LaunchEffectResult::WorkspaceChanged {
                    workspace_id: answered_for,
                    clear_interaction,
                    result,
                } => {
                    if workspace_id != answered_for {
                        continue;
                    }
                    app.workspace_launch_pending = app.workspace_launch_pending.saturating_sub(1);
                    match result {
                        Ok(policy) if clear_interaction => {
                            app.launch.adopt_workspace(policy);
                            app.show_action_message(
                                "moved into this Workspace's policy — every launch here starts from it",
                            );
                        }
                        Ok(policy) => {
                            app.launch.sync_workspace(policy);
                            app.show_action_message("Workspace launch policy saved");
                        }
                        Err(error) => {
                            let message =
                                format!("could not change the Workspace launch policy: {error}");
                            app.show_action_message(message.clone());
                            app.push_log(LogEntry::error(message));
                        }
                    }
                }
            }
        }
    }
}

impl Drop for LaunchEffects {
    fn drop(&mut self) {
        // Finish already-accepted edits before this run is allowed to leave.
        // Otherwise quitting immediately after Enter could terminate the
        // process between enqueueing a durable Workspace edit and writing it.
        self.send.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn submit_workspace_launch(
    effects: &LaunchEffects,
    app: &mut App,
    workspace_id: String,
    change: WorkspaceLaunchChange,
    clear_interaction: bool,
) {
    app.workspace_launch_pending += 1;
    effects.submit(LaunchEffect::ChangeWorkspace {
        workspace_id,
        change,
        clear_interaction,
    });
}

pub struct RunContext<'a> {
    pub standing_launch: &'a LaunchPolicy,
    pub preferences_path: &'a Path,
    pub config: &'a Config,
}

/// Global actions which operate on the current interaction without dismissing
/// its navigator. They fall through to the ordinary list-key handler below.
fn interaction_navigator_passthrough(code: &KeyCode) -> bool {
    matches!(code, KeyCode::Char('i') | KeyCode::Char('S'))
}

/// Make `interaction` current immediately. Its complete state is loaded on the
/// event-loop thread, then replaces the previous interaction's state in one
/// step. There is no separate loader-owned target or pending-load state.
fn make_interaction_current(
    app: &mut App,
    live: &mut Attachment,
    client: &Client,
    standing_launch: &LaunchPolicy,
    interaction: InteractionSummary,
) {
    if interaction.id == app.session_id {
        return;
    }
    let id = interaction.id.clone();
    match session::attach_live_interaction(client, &id) {
        Ok((mut next, next_live)) => {
            next.adopt(app.take_operator_state());
            next.launch.interaction = standing_launch.clone();
            if let Some(workspace) = next
                .interactions
                .workspaces
                .iter()
                .find(|workspace| Some(workspace.id.as_str()) == next.workspace.id.as_deref())
                .cloned()
            {
                next.show_workspace(&workspace);
            }
            next.view = crate::app::View::Events;
            next.focus = Focus::List;
            *app = next;
            *live = next_live;
        }
        Err(error) => {
            app.push_log(LogEntry::error(format!(
                "could not make interaction {id} current: {error:#}"
            )));
        }
    }
}

/// Return the running interaction an in-client transition explicitly stops.
pub fn stops_current_interaction(outcome: &RunOutcome, live: &Attachment) -> bool {
    match (outcome, live) {
        (RunOutcome::Reset, Attachment::Attached { .. }) => true,
        _ => false,
    }
}

/// The event loop: apply pending session updates, render, and handle input
/// until the operator quits or asks to switch sessions.
pub fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    client: &Client,
    live: &mut Attachment,
    context: RunContext<'_>,
) -> Result<RunOutcome> {
    let RunContext {
        standing_launch,
        preferences_path,
        config,
    } = context;
    // The model column's ordering is remembered across runs, so pick it up
    // before the picker can be opened.
    app.recent_models = preferences::load_recent_models(preferences_path);
    let launch_effects = LaunchEffects::new(client.clone());
    let mut pending_fold = false;
    let mut interactions_refreshed = Instant::now();
    loop {
        let workspace_id = app.workspace.id.clone().unwrap_or_default();
        app.notices.expire();
        launch_effects.apply_ready(app, &workspace_id);
        // Workspace launch policy is a server-owned read model. Refresh it
        // independently of input so edits from another Styra client flow into
        // this Driva view and invalidate its planned options.
        if let Ok(policy) = client.workspace_launch(&workspace_id) {
            if policy != app.launch.workspace {
                app.launch.sync_workspace(policy);
            }
        }
        session::ensure_driva_plan(app, client, &workspace_id);
        let mut disconnected = false;
        if let Attachment::Attached { cursor } = live {
            match client.updates(&app.session_id, *cursor) {
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
            *live = Attachment::Detached;
        }

        if app.interactions.open && interactions_refreshed.elapsed() >= INTERACTIONS_REFRESH {
            interactions_refreshed = Instant::now();
            if let Ok(interactions) = client.list_interactions() {
                app.interactions.refresh(interactions);
            }
        }

        if let Attachment::Attached { .. } = live {
            if app.activity.status == Status::Idle && app.outbox.queued_count() > 0 {
                match client.send_queued_message(&app.session_id) {
                    Ok((Some(_), queued)) => {
                        app.outbox.replace_queued(queued);
                        app.activity.status = Status::Running;
                        let waiting = app.outbox.queued_count();
                        app.show_action_message(if waiting == 0 {
                            "sent queued message automatically".into()
                        } else {
                            format!("sent queued message automatically ({waiting} still waiting)")
                        });
                    }
                    Ok((None, queued)) => app.outbox.replace_queued(queued),
                    Err(error) => {
                        app.push_log(LogEntry::error(format!("queued send failed: {error:#}")));
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

        // Template discovery and choice are part of this root loop. Even the
        // loading state is cancellable, and no nested event loop can defer the
        // edit produced by Enter until some unrelated future keypress.
        if let Some(template_picker) = app.template_picker.as_mut() {
            let action = template_picker.handle_key(key.code);
            match action {
                picker::TemplatePickerAction::None => {}
                picker::TemplatePickerAction::Cancel => app.template_picker = None,
                picker::TemplatePickerAction::Unchanged => {
                    app.template_picker = None;
                    app.show_action_message(
                        "templates unchanged — Space toggles the highlighted template",
                    );
                }
                picker::TemplatePickerAction::Apply { scope, chosen } => {
                    app.template_picker = None;
                    launch::set_templates_for(app, scope, chosen);
                }
            }
            // An Apply can have queued a Workspace effect. Dispatch it now,
            // before returning to drawing, without waiting for another key.
            if let Some(Request::ChangeWorkspaceLaunch {
                change,
                clear_interaction,
            }) = app.take_workspace_launch_request()
            {
                submit_workspace_launch(
                    &launch_effects,
                    app,
                    workspace_id,
                    change,
                    clear_interaction,
                );
            }
            continue;
        }

        // While the reference is open it is modal, so none of the commands
        // described by it can accidentally act on the session underneath.
        if app.help.is_open() {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => app.help.close(),
                // The reference is taller than a short terminal, so the
                // sections at the end have to be reachable.
                KeyCode::Char('j') | KeyCode::Down => app.help.line_down(),
                KeyCode::Char('k') | KeyCode::Up => app.help.line_up(),
                KeyCode::PageDown => app.help.page_down(),
                KeyCode::PageUp => app.help.page_up(),
                KeyCode::Char('g') => app.help.scroll_to_top(),
                _ => {}
            }
            continue;
        }
        // The message editor's path prompt is modal, and its second question is
        // answered by a bare letter that means something else everywhere else,
        // so it is handled ahead of the reference.
        if app.insert.is_some() {
            keys::handle_insert_key(app, key);
            continue;
        }
        // So is the Driva view's mount prompt: what is typed into it is part
        // of a path, including the characters that are shortcuts elsewhere.
        if app.launch.prompt.is_some() {
            keys::handle_mount_prompt_key(app, key);
            // Confirming a Workspace mount closes the prompt and emits an
            // effect. It must leave on this key, not sit behind the next one.
            if let Some(Request::ChangeWorkspaceLaunch {
                change,
                clear_interaction,
            }) = app.take_workspace_launch_request()
            {
                submit_workspace_launch(
                    &launch_effects,
                    app,
                    workspace_id,
                    change,
                    clear_interaction,
                );
            }
            continue;
        }
        // In input focus, `?` is message text rather than a shortcut.
        if app.focus == Focus::List && key.code == KeyCode::Char(HELP.chars().next().unwrap()) {
            app.help.open();
            continue;
        }

        // The embedded interaction list owns navigation while it is open.
        // Moving its cursor makes that interaction current immediately. Enter
        // only closes the navigator; there is no preview or deferred attach.
        if app.interactions.open && app.focus == Focus::List {
            match key.code {
                KeyCode::Char('a') | KeyCode::Esc | KeyCode::Enter => {
                    app.interactions.open = false;
                    continue;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if let Some(interaction) = app
                        .interactions
                        .next(&app.session_id, app.workspace.id.as_deref())
                    {
                        make_interaction_current(app, live, client, standing_launch, interaction);
                    }
                    continue;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if let Some(interaction) = app
                        .interactions
                        .previous(&app.session_id, app.workspace.id.as_deref())
                    {
                        make_interaction_current(app, live, client, standing_launch, interaction);
                    }
                    continue;
                }
                KeyCode::Char('w') => {
                    app.interactions.toggle_workspace_scope();
                    continue;
                }
                KeyCode::Char('D') => {
                    let Some(interaction) = app.interactions.current(&app.session_id).cloned()
                    else {
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
                    make_interaction_current(app, live, client, standing_launch, next);
                    continue;
                }
                code if interaction_navigator_passthrough(&code) => {}
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
                Focus::Input => keys::handle_input_key(app, client, &workspace_id, live, key),
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
            Some(Request::SetWorktreesEnabled(enabled)) => {
                match client.set_workspace_worktrees_enabled(&workspace_id, enabled) {
                    Ok(workspace) => {
                        app.show_workspace(&workspace);
                        app.show_action_message(format!(
                            "worktree creation {} for future launches",
                            if enabled { "enabled" } else { "disabled" }
                        ));
                    }
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not change worktree creation: {error:#}"
                    ))),
                }
            }
            Some(Request::OpenSession(id)) => return Ok(RunOutcome::OpenSession(id)),
            Some(Request::Sessions) => {
                let mut sessions = client.list_sessions(&workspace_id)?;
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
                let workspaces = client.list_workspaces()?;
                app.view = crate::app::View::Events;
                app.focus = Focus::List;
                app.interactions.open(interactions, workspaces);
                interactions_refreshed = Instant::now();
            }
            Some(Request::Reset) => return Ok(RunOutcome::Reset),
            Some(Request::NewSession) => return Ok(RunOutcome::NewSession),
            Some(Request::ApplySelection) => {
                let Attachment::Attached { .. } = live else {
                    continue;
                };
                let selection = app.selection.clone();
                match client.set_session_selection(&app.session_id, &selection) {
                    Ok(()) => app.show_action_message(format!("model set to {}", selection.model)),
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not switch to {}: {error:#}",
                        selection.model
                    ))),
                }
            }
            Some(Request::Templates) => {
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
                let request_id = launch_effects.submit_templates(workspace_id.clone());
                app.template_picker = Some(picker::TemplatePicker::loading(
                    request_id,
                    workspace_id.clone(),
                    app.launch.scope,
                    current,
                ));
            }
            Some(Request::ChangeWorkspaceLaunch {
                change,
                clear_interaction,
            }) => {
                submit_workspace_launch(
                    &launch_effects,
                    app,
                    workspace_id.clone(),
                    change,
                    clear_interaction,
                );
            }
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

    fn effects_for_test() -> (LaunchEffects, Sender<LaunchEffectResult>) {
        let (send, _jobs) = mpsc::channel();
        let (results, receive) = mpsc::channel();
        (
            LaunchEffects {
                send: Some(send),
                receive,
                worker: None,
                next_template_request: AtomicU64::new(1),
            },
            results,
        )
    }

    #[test]
    fn stopping_is_a_navigator_passthrough_action() {
        assert!(interaction_navigator_passthrough(&KeyCode::Char('S')));
        assert!(interaction_navigator_passthrough(&KeyCode::Char('i')));
        assert!(!interaction_navigator_passthrough(&KeyCode::Char('l')));
    }

    #[test]
    fn workspace_acknowledgement_updates_the_snapshot_and_clears_pending() {
        let (effects, results) = effects_for_test();
        let mut app =
            App::pending(styra_server::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap());
        app.workspace_launch_pending = 1;
        let policy = LaunchPolicy {
            templates: vec!["rust".into()],
            ..LaunchPolicy::default()
        };
        results
            .send(LaunchEffectResult::WorkspaceChanged {
                workspace_id: "workspace".into(),
                clear_interaction: false,
                result: Ok(policy.clone()),
            })
            .unwrap();

        effects.apply_ready(&mut app, "workspace");

        assert_eq!(app.workspace_launch_pending, 0);
        assert_eq!(app.launch.workspace, policy);
        assert!(app
            .notices
            .iter()
            .any(|notice| notice.text == "Workspace launch policy saved"));
    }

    #[test]
    fn workspace_edit_failure_is_visible_without_opening_the_log() {
        let (effects, results) = effects_for_test();
        let mut app =
            App::pending(styra_server::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap());
        app.workspace_launch_pending = 1;
        results
            .send(LaunchEffectResult::WorkspaceChanged {
                workspace_id: "workspace".into(),
                clear_interaction: false,
                result: Err("disk full".into()),
            })
            .unwrap();

        effects.apply_ready(&mut app, "workspace");

        assert_eq!(app.workspace_launch_pending, 0);
        assert!(app
            .notices
            .iter()
            .any(|notice| notice.text.contains("disk full")));
    }
}
