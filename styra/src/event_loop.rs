use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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
use crate::config::Configuration;
use crate::keymap::HELP;
use crate::keys;
use crate::launch::{self, LaunchScope};
use crate::picker;
use crate::preferences;
use crate::session::{self, Attachment};
use crate::ui;
use styra_server::Client;
use styra_protocol::{
    InteractionSummary, LogEntry, TemplateSummary, WorkspaceLaunchChange, WorkspaceSummary,
};

/// What the interactive loop returned control to `main` for.
pub enum RunOutcome {
    Quit,
    OpenWorkspace {
        workspace: Box<WorkspaceSummary>,
        session_id: Option<String>,
        /// Land in the Workspace with the live Interactions navigator open,
        /// as entering one that has work in flight does.
        open_interactions: bool,
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
    pub config: &'a dyn Configuration,
}

/// Hand one host path to the configured opener, reporting the outcome the way
/// [`terminal::open_shell`](crate::terminal::open_shell) does.
///
/// The one place a file is opened, so every route to it — the Files view, a
/// typed `files` answer, a reference in a reply — obeys the same configuration
/// and says the same thing about it afterwards.
fn open_path(app: &mut App, config: &dyn Configuration, path: &Path) {
    let mut command = config.open_file(path);
    // Named in both messages, because what opens a file is configuration: an
    // operator who set it needs to see which command Styra actually ran.
    let described = crate::terminal::describe(&command);
    match crate::terminal::spawn_detached(&mut command) {
        Ok(()) => app.show_action_message(format!("opened {described}")),
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not open {}: {error:#}",
            path.display()
        ))),
    }
}

/// Global actions which operate on the current interaction without dismissing
/// its navigator. They fall through to the ordinary list-key handler below.
fn interaction_navigator_passthrough(code: &KeyCode) -> bool {
    matches!(code, KeyCode::Char('i') | KeyCode::Char('S'))
}

/// Load the interaction the navigator's cursor has moved onto, if it is not
/// the one already on screen. Called when the cursor settles, and ahead of any
/// key that acts on the row under it, so an outstanding move never leaves a
/// command addressing the interaction the operator has already moved off.
fn load_cursored_interaction(
    app: &mut App,
    live: &mut Attachment,
    client: &Client,
    standing_launch: &LaunchPolicy,
) {
    let Some(interaction) = app.interactions.pending(&app.session_id).cloned() else {
        return;
    };
    make_interaction_current(app, live, client, standing_launch, interaction);
}

/// Make `interaction` current. Its complete state is loaded on the event-loop
/// thread, then replaces the previous interaction's state in one step. There
/// is no separate loader-owned target: the cursor is the only pending state,
/// and it rests here whether the load lands or fails.
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
            app.interactions.rest();
        }
        Err(error) => {
            // The cursor comes home on a failed load too, so a server that
            // cannot answer for one interaction is reported once rather than
            // re-asked on every frame.
            app.interactions.rest();
            app.push_log(LogEntry::error(format!(
                "could not make interaction {id} current: {error:#}"
            )));
        }
    }
}

/// Open the live Interactions navigator over the main Interaction view, as `a`
/// does.
///
/// Best-effort: a server that cannot answer, or has nothing live left to list,
/// leaves the navigator closed rather than failing the transition that asked
/// for it. The root loop uses this when entering a Workspace it already knows
/// holds live work.
pub fn open_interaction_navigator(app: &mut App, client: &Client) {
    let Ok(interactions) = client.list_interactions() else {
        return;
    };
    if interactions.is_empty() {
        return;
    }
    let workspaces = client.list_workspaces().unwrap_or_default();
    app.view = crate::app::View::Events;
    app.focus = Focus::List;
    app.interactions.open(interactions, workspaces);
}

/// Fill the quota view from the server's log, which is where the readings live:
/// the server reads the figures off every interaction's wire, so one client
/// asking gets every session's readings rather than only this one's.
///
/// Called both when `Q` asks for the log and once at startup, so the footer's
/// warning is right from the first frame. A provider that is already nearly out
/// says so hours before it next volunteers a reading, and an operator who never
/// presses `Q` would otherwise not be told at all.
///
/// Best-effort: a server that cannot answer leaves the readings as they were
/// and logs why, rather than failing the startup that asked.
pub fn refresh_quota(app: &mut App, client: &Client) {
    match client.quota_log() {
        Ok(readings) => app.quota.replace(readings),
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not read the quota log: {error:#}"
        ))),
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
                    app.on_ended(styra_protocol::InteractionEnd {
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

        // Keep this snapshot fresh even with the navigator closed: it is what
        // lets the footer report a different interaction becoming idle while
        // the operator is reading or working in this one.
        if interactions_refreshed.elapsed() >= INTERACTIONS_REFRESH {
            interactions_refreshed = Instant::now();
            if let Ok(interactions) = client.list_interactions() {
                app.interactions.refresh(interactions);
            }
        }

        // A cursor that has come to rest loads the interaction under it. Until
        // then the navigator says that row is loading and the screen below is
        // still the interaction it was.
        if app.interactions.due(&app.session_id).is_some() {
            load_cursored_interaction(app, live, client, standing_launch);
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

        // The branching choice is modal and can replace the current Session
        // on confirmation, so it owns the key before every underlying view.
        if app.branch_prompt.is_some() {
            keys::handle_branch_prompt_key(app, client, key);
            // Confirming has already branched; open the result on this key
            // rather than leaving the switch queued behind the next one.
            if let Some(Request::OpenSession(id)) = app.take_open_session_request() {
                return Ok(RunOutcome::OpenSession(id));
            }
            continue;
        }

        // The list of files a reply cites is modal: while it is open nothing
        // underneath it can be acted on, `?` included.
        if app.references.is_some() {
            keys::handle_references_key(app, key);
            // Choosing a file closes the picker and asks for that file. It has
            // to open on this key rather than sit behind the next one.
            if let Some(Request::OpenPath(path)) = app.take_open_path_request() {
                open_path(app, config, &path);
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
        // The Git checkout is Workspace metadata. Its prompt is modal so a
        // path such as `git@host:group/project` is input rather than keys.
        if app.git_repository_prompt.is_some() {
            keys::handle_git_repository_prompt_key(app, client, key);
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
        // Moving its cursor makes that interaction current once the cursor
        // rests, so the list can be walked across faster than interactions can
        // be loaded. Enter only closes the navigator; there is no preview.
        if app.interactions.open && app.focus == Focus::List {
            let session_id = app.session_id.clone();
            match key.code {
                // In All scope the entries are grouped under Workspace
                // headings, and ctrl-j/ctrl-k skip whole groups: one press per
                // Workspace rather than one per interaction. They move the
                // cursor like j/k, so a skip across several groups costs no
                // more loads than a step across one row.
                // The jump to the next unseen-idle interaction moves the cursor
                // like the other skips do, so the row it lands on is the one
                // load it pays for — and the operator sees where it went.
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app
                        .interactions
                        .cursor_to_next_idle(&session_id, app.workspace.id.as_deref())
                        .is_none()
                    {
                        app.show_action_message("no interaction has gone idle unseen");
                    }
                    continue;
                }
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.interactions
                        .cursor_next_workspace(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.interactions
                        .cursor_previous_workspace(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    app.interactions
                        .cursor_next(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.interactions
                        .cursor_previous(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                _ => {}
            }
            // Every other key acts on the row under the cursor, so a move
            // still waiting out its settle is completed first rather than
            // abandoned.
            load_cursored_interaction(app, live, client, standing_launch);
            match key.code {
                KeyCode::Char('a') | KeyCode::Esc | KeyCode::Enter => {
                    app.interactions.close();
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
                        app.interactions.close();
                        return Ok(RunOutcome::Reset);
                    };
                    make_interaction_current(app, live, client, standing_launch, next);
                    continue;
                }
                code if interaction_navigator_passthrough(&code) => {}
                _ => app.interactions.close(),
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
                // A Workspace with work in flight is entered at that work: the
                // first live Interaction becomes current and its navigator
                // opens, so the operator sees the rest of the live list without
                // being asked which Session they meant.
                let live_interaction = client.list_interactions().ok().and_then(|interactions| {
                    crate::interactions::first_live_in_workspace(&interactions, &workspace.id)
                });
                if let Some(interaction) = live_interaction {
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace: Box::new(workspace),
                        session_id: Some(interaction.id),
                        open_interactions: true,
                    });
                }
                let mut sessions = client.list_sessions(&workspace.id)?;
                if sessions.is_empty() {
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace: Box::new(workspace),
                        session_id: None,
                        open_interactions: false,
                    });
                }
                if let Some(id) = picker::run_session_picker(terminal, client, &mut sessions, None)?
                {
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace: Box::new(workspace),
                        session_id: Some(id),
                        open_interactions: false,
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
                if let Some(id) = picker::run_session_picker(
                    terminal,
                    client,
                    &mut sessions,
                    Some(&app.session_id),
                )? {
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
            // Asked with the navigator closed, so the interaction is loaded
            // outright rather than through the cursor's settle, and the live
            // list opens around it: the operator asked to be taken to work
            // waiting elsewhere, and wants to see what else is waiting.
            Some(Request::NextIdleInteraction) => {
                // The footer's snapshot is up to a refresh old, and the
                // interaction it points at is one this client is not watching,
                // so ask before jumping rather than acting on a stale row.
                if let Ok(interactions) = client.list_interactions() {
                    app.interactions.refresh(interactions);
                    interactions_refreshed = Instant::now();
                }
                let Some(next) = app.interactions.next_idle_unseen(&app.session_id) else {
                    app.show_action_message("no interaction has gone idle unseen");
                    continue;
                };
                make_interaction_current(app, live, client, standing_launch, next);
                open_interaction_navigator(app, client);
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
            // Moving a stopped Session onto another agent. The server copies
            // the whole history into the new agent's native format as a
            // sibling Session, and that sibling — not this one — is where the
            // conversation carries on, so the view follows it. Both sides keep
            // a branch marker, so the Session it came from stays reachable.
            Some(Request::ConvertProvider(provider)) => {
                match client.branch_session(
                    &app.session_id,
                    None,
                    styra_protocol::BranchHistory::ThroughSelected,
                    Some(provider),
                ) {
                    Ok(converted) => {
                        app.push_log(LogEntry::info(format!(
                            "converted this Session's history to {}; continuing in Session {}",
                            provider.as_str(),
                            converted.name.as_deref().unwrap_or(&converted.id)
                        )));
                        return Ok(RunOutcome::OpenSession(converted.id));
                    }
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not convert this Session to {}: {error:#}",
                        provider.as_str()
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
            Some(Request::Quota) => refresh_quota(app, client),
            // The setting is the server's to keep — it is stored with the
            // Session and acted on long after this client has gone — so what
            // is shown is what the server accepted, not what was pressed.
            Some(Request::SetAutoRetry(enabled)) => {
                match client.set_interaction_auto_retry(&app.session_id, enabled) {
                    Ok(()) => {
                        app.auto_retry = enabled;
                        app.show_action_message(if enabled {
                            "rate-limit retry on — this session will be asked again after a reset"
                        } else {
                            "rate-limit retry off"
                        });
                    }
                    Err(error) => {
                        let message = format!("could not change the rate-limit retry: {error:#}");
                        app.show_action_message(message.clone());
                        app.push_log(LogEntry::error(message));
                    }
                }
            }
            Some(Request::EditFile) => {
                let Some(path) = app.selected_file_path() else {
                    continue;
                };
                open_path(app, config, &path);
            }
            Some(Request::OpenPath(path)) => open_path(app, config, &path),
            Some(Request::OpenShell) => {
                match crate::terminal::open_shell(client, &app.session_id, config) {
                    Ok(program) => app.show_action_message(format!("opened shell in {program}")),
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not open session shell: {error:#}"
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
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
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
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
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
