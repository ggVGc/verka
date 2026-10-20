use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::action_menu::{self, ActionMenu};
use crate::activity::Status;
use crate::app::{App, Focus, LaunchPolicy, Request, View};
use crate::audio::AudioInput;
use crate::config::Configuration;
use crate::edit_report::EditReports;
use crate::input;
use crate::keybindings as keys;
use crate::launch::{self, LaunchScope};
use crate::launcher;
use crate::picker;
use crate::preferences;
use crate::presentation;
use crate::session::{self, Attachment};
use crate::workspace_jump::{self, WorkspaceJump};
use styra_protocol::{
    CompletionState, InteractionQuery, InteractionSummary, LogEntry, TemplateSummary,
    WorkspaceLaunchChange, WorkspaceSummary,
};
use styra_server::Client;
use styra_ui::query::Edit;
use styra_ui::Ui;

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
    OpenBranch(String),
    /// Reopen the Session viewed before the last [`RunOutcome::OpenSession`].
    JumpBack,
    Reset,
    NewSession,
}

const INTERACTIONS_REFRESH: Duration = Duration::from_millis(250);

/// How long one round of the loop waits for a key before coming back to poll
/// the server for updates, interactions and quota.
const KEY_POLL: Duration = Duration::from_millis(100);

fn start_rebase(app: &mut App, client: &Client, target: Option<&str>) {
    let id = app.session_id.clone();
    let started = match target {
        Some(target) => client.retarget_session(&id, target),
        None => client.rebase_session(&id),
    };
    app.rebase_status = Some(styra_protocol::RebaseStatus {
        finished: started.is_err(),
        succeeded: false,
        output: String::new(),
        source_commit: String::new(),
        message: match &started {
            Ok(()) => "Starting rebase…".into(),
            Err(error) => format!("Could not rebase: {error:#}"),
        },
    });
    if started.is_ok() {
        app.pending_rebase = Some(id);
    }
}

/// The message box owns paste only when no inner prompt or chooser owns input.
fn message_paste_active(app: &App) -> bool {
    app.focus == Focus::Input
        && !app.help.is_open()
        && app.rebase_status.is_none()
        && app.launcher.is_none()
        && app.template_picker.is_none()
        && app.retarget_prompt.is_none()
        && app.integration_prompt.is_none()
        && app.branch_prompt.is_none()
        && app.link_menu.is_none()
        && app.action_menu.is_none()
        && app.workspace_jump.is_none()
        && app.insert.is_none()
        && app.git_repository_prompt.is_none()
        && app.launch.prompt.is_none()
        && app.launch.command_prompt.is_none()
        && app.tag_picker.is_none()
        && !app.diff_focused()
        && !app.search.typing()
}

/// Whether Space opens the action menu: on the list of one of the session's
/// windows, with nothing modal over it and nothing being typed. A highlighted
/// link keeps Space for its own menu.
fn action_menu_available(app: &App) -> bool {
    app.focus == Focus::List
        && app.launcher.is_none()
        && app.template_picker.is_none()
        && app.retarget_prompt.is_none()
        && app.integration_prompt.is_none()
        && app.branch_prompt.is_none()
        && app.link_menu.is_none()
        && app.link_highlight.is_none()
        && app.workspace_jump.is_none()
        && app.insert.is_none()
        && app.git_repository_prompt.is_none()
        && app.launch.prompt.is_none()
        && app.launch.command_prompt.is_none()
        && app.tag_picker.is_none()
        && !app.diff_focused()
        && !app.search.typing()
        && !app.interactions.typing_filter()
        && presentation::current_window(app).has_action_menu()
}

/// How long a level-meter frame waits for one instead.
///
/// The loop's own period is the server's, and a meter repainted on it moves in
/// ten steps a second — slower than speech, so the bar lands on syllables
/// rather than following them and reads as a stutter rather than as a level.
/// These frames cost nothing but a repaint: they ask the device for the level
/// it has already measured, and nothing else about the loop's round happens in
/// them.
const METER_FRAME: Duration = Duration::from_millis(25);

/// How often a frame whose text is read off the clock is repainted for that
/// reason alone — see [`presentation::ticking`](crate::presentation::ticking).
///
/// One second, because that is the resolution those figures are written at:
/// an elapsed count in whole seconds gains nothing from being repainted twice
/// within one, and loses a beat if it is repainted less often than that.
const REDRAW_TICK: Duration = Duration::from_secs(1);

/// Quota readings change on the scale of whole interactions, not keystrokes,
/// so this is slow enough to cost the server nothing and quick enough that the
/// footer's warning is never meaningfully behind the account.
const QUOTA_REFRESH: Duration = Duration::from_secs(5);
/// How often the open processes view lists the sandbox again.
const PROCESSES_REFRESH: Duration = Duration::from_secs(1);

/// How often the in-window diff is read again while the agent is working on
/// the checkout it shows.
const DIFF_REFRESH: Duration = Duration::from_secs(5);

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

    /// Reports whether it applied anything, so a round in which the worker
    /// answered nothing this client is still waiting for does not count as a
    /// change to the screen.
    fn apply_ready(&self, app: &mut App, workspace_id: &str) -> bool {
        let mut applied = false;
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
                    applied = true;
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
                    applied = true;
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
                            app.show_error(message.clone());
                            app.push_log(LogEntry::error(message));
                        }
                    }
                }
            }
        }
        applied
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

/// The interactions snapshot is polled off the terminal thread.
///
/// The server answers `list_interactions` by reading every live Session's
/// record and the Workspace directory, which takes on the order of 100ms. Asked
/// from the loop on every [`INTERACTIONS_REFRESH`], that was a tenth of a
/// second in every four during which no key was read, so typing landed in
/// stutters. Here the loop only asks; the answer is applied on whichever round
/// it arrives in.
struct InteractionsPoll {
    /// Each request carries its own query, since what the navigator shows —
    /// the Interaction on screen, whether finished rows are listed — changes
    /// between polls.
    send: Sender<InteractionQuery>,
    /// `None` for a poll the server could not answer: the snapshot on screen
    /// stays as it was, as it did when the loop polled for itself.
    receive: Receiver<Option<Vec<InteractionSummary>>>,
    /// A request has been made and not yet answered. One at a time: a server
    /// slower than the refresh period would otherwise have a queue of them.
    in_flight: bool,
}

impl InteractionsPoll {
    fn new(client: Client) -> Self {
        let (send, requests) = mpsc::channel::<InteractionQuery>();
        let (results, receive) = mpsc::channel();
        // Not joined: the loop leaving drops `send`, and the worker finishes
        // the request it is in, if any, and stops.
        thread::Builder::new()
            .name("styra-interactions-poll".into())
            .spawn(move || {
                while let Ok(query) = requests.recv() {
                    if results.send(client.list_interactions(&query).ok()).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning interactions poller");
        Self {
            send,
            receive,
            in_flight: false,
        }
    }

    /// Ask for a fresh snapshot, unless one is already on its way.
    fn request(&mut self, query: InteractionQuery) {
        if !self.in_flight && self.send.send(query).is_ok() {
            self.in_flight = true;
        }
    }

    /// The answer to the outstanding request, if it has arrived and the server
    /// could give one.
    fn ready(&mut self) -> Option<Vec<InteractionSummary>> {
        let answer = self.receive.try_recv().ok()?;
        self.in_flight = false;
        answer
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

/// Store a completed tag edit and immediately refresh the row the navigator
/// renders. A new tag is a completed edit in its own right: requiring a
/// second Enter after the text field made it look as though adding failed.
fn save_tags(app: &mut App, client: &Client, id: String, tags: Vec<String>) {
    app.tag_picker = None;
    match client.set_session_tags(&id, tags) {
        Ok(summary) => {
            if let Some(item) = app
                .interactions
                .items
                .iter_mut()
                .find(|item| item.id == summary.id)
            {
                item.tags = summary.tags;
            }
            app.show_action_message("interaction tags saved");
        }
        Err(error) => app.show_error(format!("could not save interaction tags: {error:#}")),
    }
}

fn apply_tag_picker_filter(app: &mut App) {
    let tags = {
        let Some(picker) = app.tag_picker.as_ref() else {
            return;
        };
        (picker.purpose == crate::tag_picker::TagPurpose::Filter).then(|| picker.selected.clone())
    };
    let Some(tags) = tags else {
        return;
    };
    let id = app.session_id.clone();
    let workspace_id = app.workspace.id.clone();
    app.interactions
        .set_tag_filter(tags, &id, workspace_id.as_deref());
}

/// Toggle the highlighted tag and, when the picker is choosing a navigator
/// filter, apply the new selection immediately. The filter picker is drawn on
/// top of the navigator, so updating its state here lets the rows behind it
/// react to every Space press instead of waiting for Enter.
fn toggle_tag_picker_selection(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.toggle();
    apply_tag_picker_filter(app);
}

fn toggle_tag_picker_exclusion(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.toggle_excluded();
    apply_tag_picker_filter(app);
}

fn clear_tag_picker_selection(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.selected.clear();
    apply_tag_picker_filter(app);
}

fn restore_tag_picker_selection(app: &mut App) {
    let Some(picker) = app.tag_picker.as_mut() else {
        return;
    };
    picker.restore_selection();
    apply_tag_picker_filter(app);
}

pub struct RunContext<'a> {
    pub standing_launch: &'a LaunchPolicy,
    pub preferences_path: &'a Path,
    pub config: &'a dyn Configuration,
}

/// Use the interaction's checkout for project commands, including stored Sessions.
fn project_action_directory(app: &App, client: &Client) -> Result<std::path::PathBuf> {
    if let Some(root) = app.workspace.root() {
        return Ok(root.to_path_buf());
    }
    // Stored Sessions have no live sandbox root, but can still own a checkout.
    if let Some(workspace_id) = app.workspace.id.as_deref() {
        for worktree in client.list_worktrees(Some(workspace_id))? {
            if worktree.exists
                && worktree
                    .sessions
                    .iter()
                    .any(|session| session.id == app.session_id)
            {
                if let Some(path) = worktree.worktree {
                    return Ok(path);
                }
            }
        }
    }
    app.workspace
        .host_path
        .clone()
        .or_else(|| app.workspace.root_or_current_directory())
        .ok_or_else(|| anyhow::anyhow!("could not determine the Workspace directory"))
}

/// Hand one host path to the configured opener, at `line` when one is known,
/// reporting the outcome the way
/// [`terminal::open_shell`](crate::terminal::open_shell) does.
///
/// The one place a file is opened, so the Files view, a typed `files` answer,
/// a selected Markdown link, and the diff view obey the same configuration
/// and say the same thing afterwards.
fn open_path(app: &mut App, config: &dyn Configuration, path: &Path, line: Option<u32>) {
    let mut command = match line {
        Some(line) => config.open_file_at(path, line),
        None => config.open_file(path),
    };
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

/// Hand a web address to the configured browser, reporting the outcome as
/// [`open_path`] does.
fn open_url(app: &mut App, config: &dyn Configuration, url: &str) {
    let mut command = config.open_url(url);
    let described = crate::terminal::describe(&command);
    match crate::terminal::spawn_detached(&mut command) {
        Ok(()) => app.show_action_message(format!("opened {described}")),
        Err(error) => app.push_log(LogEntry::error(format!("could not open {url}: {error:#}"))),
    }
}

/// Global actions which operate on the current interaction without dismissing
/// its navigator. They fall through to the ordinary list-key handler below.
/// `n` and Ctrl-N are among them: stepping keeps the list open on the
/// interaction it lands on, while Ctrl-N starts a new interaction.
fn interaction_navigator_passthrough(key: crossterm::event::KeyEvent) -> bool {
    keys::GLOBAL_FOCUS_MESSAGE.matches(key)
        || keys::GLOBAL_REBASE.matches(key)
        || keys::GLOBAL_STOP.matches(key)
        || keys::GLOBAL_NEXT_LIVE.matches(key)
        || keys::GLOBAL_NEW_SESSION.matches(key)
        // The jump narrows the navigator to the Workspaces it offers, so the
        // navigator stays open under it.
        || keys::GLOBAL_WORKSPACE_JUMP.matches(key)
        || keys::GLOBAL_WORKSPACE_JUMP_LISTED.matches(key)
}

/// Narrow an open navigator to the Workspaces the jump's query leaves, or
/// lift that narrowing once the jump has closed.
fn narrow_navigator_to_jump(app: &mut App) {
    app.interactions.jump_workspaces = app
        .workspace_jump
        .as_ref()
        .filter(|_| app.interactions.open)
        .map(WorkspaceJump::matching);
}

/// Open the live interactions with the cursor on the first of `target`'s, the
/// way `v` lands. Run where the Workspace is chosen rather than asked for: a
/// request is only taken once a key reaches the dispatch, so the cursor would
/// not settle and load until the operator pressed another.
fn open_interactions_in_workspace(app: &mut App, client: &Client, target: &str) -> Result<()> {
    let workspaces = client.list_workspaces()?;
    app.set_view(crate::app::View::Events);
    app.focus = Focus::List;
    app.interactions.show(workspaces);
    let session_id = app.session_id.clone();
    if !app
        .interactions
        .cursor_to_workspace(target, &session_id, app.workspace.id.as_deref())
    {
        app.show_warning("the list's filters hide that Workspace's interactions");
    }
    Ok(())
}

/// Over the grid, show the Workspace highlighted in the jump by moving the
/// grid's cursor onto its first tile. Only the grid's own cursor moves, so
/// nothing loads until the choice is made, and Esc has nothing to undo but
/// the cursor and scope the jump remembered.
fn preview_workspace_jump(app: &mut App) {
    let Some(target) = app
        .workspace_jump
        .as_ref()
        .filter(|jump| jump.grid.is_some())
        .and_then(|jump| jump.selected())
        .map(str::to_owned)
    else {
        return;
    };
    select_workspace_in_grid(app, &target);
}

/// Narrow the grid to `target` and move its cursor to `target`'s first tile.
/// The grid shows `target` in place of the Workspace on screen until a tile of
/// it has loaded.
fn select_workspace_in_grid(app: &mut App, target: &str) -> bool {
    app.interactions.only_current_workspace = true;
    app.overview.workspace =
        (app.workspace.id.as_deref() != Some(target)).then(|| target.to_owned());
    app.overview
        .select_workspace(&app.interactions, target, app.workspace.id.as_deref())
}

/// Load the interaction the list or grid cursor has moved onto, if it is not
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
    let overview = (app.view == crate::app::View::Overview).then(|| app.overview.clone());
    make_interaction_current(app, live, client, standing_launch, interaction);
    if let Some(overview) = overview {
        app.overview = overview;
        app.set_view(crate::app::View::Overview);
        // The Workspace the jump chose is now the one on screen.
        if app.overview.workspace.as_deref() == app.workspace.id.as_deref() {
            app.overview.workspace = None;
        }
    }
}

fn cursored_interaction_is_loadable(app: &App) -> bool {
    app.tag_picker.is_none()
        && app.diff_branch_picker.is_none()
        && app.interactions.due(&app.session_id).is_some()
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
    session::save_composer(app, client, true);
    match session::attach_live_interaction(client, &id, &app.quota_noticed) {
        Ok((mut next, next_live)) => {
            next.adopt(app.take_operator_state());
            next.launch.interaction = standing_launch.clone();
            // The Workspace list is only filled when the navigator is opened,
            // so a switch made with it closed — `n` — would otherwise land on
            // a screen that cannot name the Workspace it is in, and the title
            // would lose its opening name. Ask the server for the list it is
            // missing, and keep the answer for the switches after this one.
            if !next
                .interactions
                .workspaces
                .iter()
                .any(|workspace| Some(workspace.id.as_str()) == next.workspace.id.as_deref())
            {
                if let Ok(workspaces) = client.list_workspaces() {
                    next.interactions.workspaces = workspaces;
                }
            }
            if let Some(workspace) = next
                .interactions
                .workspaces
                .iter()
                .find(|workspace| Some(workspace.id.as_str()) == next.workspace.id.as_deref())
                .cloned()
            {
                next.show_workspace(&workspace);
            }
            next.set_view(crate::app::View::Events);
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
/// Best-effort: nothing live to list leaves the navigator closed rather than
/// failing the transition that asked for it. The root loop uses this when
/// entering a Workspace it already knows holds live work, with the listing it
/// found that out from.
pub fn open_interaction_navigator(
    app: &mut App,
    client: &Client,
    interactions: Vec<InteractionSummary>,
) {
    if interactions.is_empty() {
        return;
    }
    let workspaces = client.list_workspaces().unwrap_or_default();
    app.set_view(crate::app::View::Events);
    app.focus = Focus::List;
    app.interactions.open(interactions, workspaces);
}

/// Make sure the App holds an interactions listing, asking the server only
/// when it has none yet — the startup listing and the loop's poll both having
/// failed. Every request that reads the fleet goes through this rather than
/// asking for itself: the poll keeps the listing at most a refresh old, and
/// the server takes a long time to answer it.
fn ensure_listed(app: &mut App, client: &Client) -> Result<()> {
    if !app.interactions.listed() {
        let query = app.interactions_query();
        app.interactions.refresh(client.list_interactions(&query)?);
    }
    Ok(())
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

/// List the processes in the current interaction's sandbox, for the
/// processes view. Reports whether the listing on screen changed.
///
/// A blank screen has no interaction to ask about; a failure is shown in the
/// view itself rather than logged, since the view asks every second.
pub fn refresh_processes(app: &mut App, client: &Client) -> bool {
    let answer = if app.session_id.is_empty() {
        Ok(None)
    } else {
        client
            .interaction_processes(&app.session_id)
            .map_err(|error| format!("{error:#}"))
    };
    let session = app.session_id.clone();
    app.processes.replace(&session, answer)
}

/// Take a fresh reading of the quota log on the loop's timer.
///
/// The attached session's own readings arrive on its update stream, but
/// readings belong to the account: another session burning the plan down shows
/// up here or not at all. Doing it on the timer rather than only on `Q` is what
/// keeps the footer's warning true for an operator who never opens the view —
/// and keeps the view itself moving while it is open.
///
/// Silent on failure, unlike [`refresh_quota`]: a server that cannot answer
/// would otherwise log the same line every few seconds.
///
/// Reports whether the readings actually moved. They change on the scale of
/// whole interactions, so most of these polls bring back exactly what the
/// footer is already showing.
fn poll_quota(app: &mut App, client: &Client) -> bool {
    match client.quota_log() {
        Ok(readings) => app.quota.restock(readings),
        Err(_) => false,
    }
}

/// Wait out one round's [`KEY_POLL`] in short frames, repainting the level
/// meter in each — or return as soon as a key arrives, which is the loop's
/// real business.
///
/// Only called while a capture is running, and a capture is only ever running
/// with the message box up: no other window can be on screen to repaint the
/// wrong thing over.
fn meter_frames(
    terminal: &mut dyn Ui,
    app: &mut App,
    audio: &mut AudioInput,
) -> Result<Option<Event>> {
    let until = Instant::now() + KEY_POLL;
    loop {
        if let Some(event) = terminal.poll_event(METER_FRAME)? {
            return Ok(Some(event));
        }
        if Instant::now() >= until {
            return Ok(None);
        }
        audio.note_level(app);
        let feedback = presentation::draw_application(terminal, app)?;
        presentation::apply_feedback(app, &feedback);
    }
}

/// What the screen says while a Git branch and linked workspace are made,
/// whichever key asked for them.
const CREATING_WORKTREE: &str = "creating a Git branch and workspace…";

/// Run a synchronous call the loop has to wait out, behind [`App::busy`].
///
/// Nothing reads the keyboard until `work` returns, so the notice takes the
/// message box's place and is painted before the call is made. Keys pressed
/// under it are thrown away afterwards rather than answered late — least of
/// all by the event list, where a stray `q` quits.
fn blocked_on<T>(
    terminal: &mut dyn Ui,
    app: &mut App,
    notice: &str,
    work: impl FnOnce(&mut App) -> T,
) -> Result<T> {
    app.busy = Some(notice.to_owned());
    presentation::draw_application(terminal, app)?;
    let done = work(app);
    app.busy = None;
    while terminal.poll_event(Duration::ZERO)?.is_some() {}
    Ok(done)
}

/// Return the running interaction an in-client transition explicitly stops.
pub fn stops_current_interaction(outcome: &RunOutcome, live: &Attachment) -> bool {
    matches!(
        (outcome, live),
        (RunOutcome::Reset, Attachment::Attached { .. })
    )
}

/// The event loop: apply pending session updates, render, and handle input
/// until the operator quits or asks to switch sessions.
pub fn run(
    terminal: &mut dyn Ui,
    app: &mut App,
    client: &Client,
    live: &mut Attachment,
    context: RunContext<'_>,
) -> Result<RunOutcome> {
    let outcome = run_rounds(terminal, app, client, live, context);
    // Every way out of the loop is a screen about to be replaced or a client
    // about to close, so what was typed on this one is stored first.
    session::save_composer(app, client, true);
    outcome
}

fn run_rounds(
    terminal: &mut dyn Ui,
    app: &mut App,
    client: &Client,
    live: &mut Attachment,
    context: RunContext<'_>,
) -> Result<RunOutcome> {
    tracing::debug!(
        target: "styra_tui::event_loop",
        pid = std::process::id(),
        session_id = %app.session_id,
        "entered event loop"
    );
    let RunContext {
        standing_launch,
        preferences_path,
        config,
    } = context;
    // The model column's ordering is remembered across runs, so pick it up
    // before the picker can be opened.
    app.recent_models = preferences::load_recent_models(preferences_path);
    let launch_effects = LaunchEffects::new(client.clone());
    let mut interactions_poll = InteractionsPoll::new(client.clone());
    let mut edit_reports = EditReports::new();
    let mut audio = AudioInput::new();
    let mut pending_fold = false;
    // Keys the action menu chose, still to be handled as if they were typed.
    let mut replay: VecDeque<crossterm::event::KeyEvent> = VecDeque::new();
    let mut interactions_refreshed = Instant::now();
    let mut quota_refreshed = Instant::now();
    let mut processes_refreshed = Instant::now();
    let mut diff_refreshed = Instant::now();
    let mut ticked = Instant::now();
    // What the clock read on the frame currently on screen, so that a reading
    // which has not changed since cannot ask for another one.
    let mut clock = presentation::clock_reading(app);
    // Whether anything has happened since the last frame was painted. The
    // round is a poll, not an event: it comes back every `KEY_POLL` whether or
    // not the server, the terminal or the operator had anything to say, and
    // almost always they did not. Every step below that can change what is on
    // screen reports whether it did, and the frame is painted only if one of
    // them did — so a Styra nobody is using stops drawing rather than
    // repainting the same screen ten times a second.
    //
    // It starts set: the first round has a blank terminal to fill.
    let mut dirty = true;
    terminal.set_bracketed_paste(false)?;
    let mut bracketed_paste = false;
    loop {
        session::save_composer(app, client, false);
        let workspace_id = app.workspace.id.clone().unwrap_or_default();
        // Every request the last round queued is carried out here, at the
        // top of the next, before anything is drawn. A key handler that
        // `continue`s — and most of them do — still restarts the loop, so
        // nothing it asked for can be left sitting in the queue behind
        // later keys, as the diff's keys once were. A picker that the
        // operator backs out of leaves the session as it was, so those arms
        // go on to the next request rather than returning.
        while let Some(request) = app.take_request() {
            dirty = true;
            match request {
                Request::Rebase => start_rebase(app, client, None),
                Request::Retarget(target) => start_rebase(app, client, Some(&target)),
                Request::Quit => return Ok(RunOutcome::Quit),
                Request::Integrate(_) | Request::GenerateIntegrationMessage => {}
                Request::Workspace => {
                    let mut workspaces = client.list_workspaces()?;
                    // The picker keeps the listing it is handed fresh while it is
                    // open, during which this loop is not polling, so what it
                    // ends on is the newest listing there is.
                    ensure_listed(app, client)?;
                    let mut interactions = app.interactions.snapshot();
                    let query = app.interactions_query();
                    let choice = picker::run_workspace_picker(
                        terminal,
                        client,
                        &mut workspaces,
                        &mut interactions,
                        &query,
                        None,
                    );
                    app.interactions.refresh(interactions);
                    // Backing out never leaves Styra, even with no Workspace (or a
                    // deleted one) behind the list: it lands on the interaction
                    // view, where `q` is still the way out.
                    let Some(choice) = choice? else {
                        app.set_view(crate::app::View::Events);
                        app.focus = Focus::List;
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
                        // A fresh interaction is asked for outright, so neither
                        // live work nor the Session list stands in the way.
                        picker::WorkspaceChoice::New(workspace) => {
                            return Ok(RunOutcome::OpenWorkspace {
                                workspace: Box::new(
                                    client.workspace(&workspace.id).unwrap_or(workspace),
                                ),
                                session_id: None,
                                open_interactions: false,
                            });
                        }
                        picker::WorkspaceChoice::CreateCurrentDirectory => {
                            let host_path = session::resolve_workspace(None)?;
                            session::create_workspace(client, host_path, None)?
                        }
                    };
                    // A Workspace with work in flight is entered at that work, the
                    // way `v` enters it: the live list opens with the cursor on the
                    // Workspace's first Interaction, so the operator sees the rest
                    // of the live list without being asked which Session they meant.
                    if workspace_jump::has_live_work(&app.interactions.items, &workspace.id) {
                        open_interactions_in_workspace(app, client, &workspace.id)?;
                        continue;
                    }
                    let mut sessions = client.list_sessions(&workspace.id)?;
                    if sessions.is_empty() {
                        return Ok(RunOutcome::OpenWorkspace {
                            workspace: Box::new(workspace),
                            session_id: None,
                            open_interactions: false,
                        });
                    }
                    // Starting fresh from this list enters the Workspace that was
                    // just chosen with no Session loaded, which is the same place
                    // entering a Workspace that has no history at all lands.
                    if let Some(choice) = picker::run_session_picker(
                        terminal,
                        client,
                        &crate::workspace::display_name(&workspace),
                        &mut sessions,
                        &app.interactions.items,
                        None,
                        true,
                    )? {
                        return Ok(RunOutcome::OpenWorkspace {
                            workspace: Box::new(workspace),
                            session_id: match choice {
                                picker::SessionChoice::Open(id) => Some(id),
                                picker::SessionChoice::New => None,
                            },
                            open_interactions: false,
                        });
                    }
                    if !client
                        .list_workspaces()?
                        .iter()
                        .any(|workspace| workspace.id == workspace_id)
                    {
                        return Ok(RunOutcome::Quit);
                    }
                }
                Request::OpenSession(id) => return Ok(RunOutcome::OpenSession(id)),
                Request::OpenBranch(id) => return Ok(RunOutcome::OpenBranch(id)),
                Request::JumpBack => return Ok(RunOutcome::JumpBack),
                Request::Sessions => {
                    let mut sessions = client.list_sessions(&workspace_id)?;
                    if sessions.is_empty() {
                        app.push_log(LogEntry::warn("no sessions found in the current Workspace"));
                        continue;
                    }
                    match picker::run_session_picker(
                        terminal,
                        client,
                        app.workspace.name.as_deref().unwrap_or("styra"),
                        &mut sessions,
                        &app.interactions.items,
                        Some(&app.session_id),
                        true,
                    )? {
                        Some(picker::SessionChoice::Open(id)) => {
                            return Ok(RunOutcome::OpenSession(id))
                        }
                        // The same new interaction `n` starts from the main view:
                        // it inherits the context being viewed, in this Workspace.
                        Some(picker::SessionChoice::New) => return Ok(RunOutcome::NewSession),
                        None => {}
                    }
                }
                // Listed here rather than inside the picker so that a server that
                // cannot answer — one older than the request — is a logged line
                // and not the end of the client.
                Request::Worktrees => {
                    let worktrees = match client.list_worktrees(Some(&workspace_id)) {
                        Ok(worktrees) => worktrees,
                        Err(error) => {
                            app.push_log(LogEntry::error(format!(
                                "could not list worktrees: {error:#}"
                            )));
                            continue;
                        }
                    };
                    let Some(choice) = crate::worktree_picker::run_worktree_picker(
                        terminal,
                        client,
                        worktrees,
                        &workspace_id,
                        &app.session_id,
                    )?
                    else {
                        continue;
                    };
                    // With every Workspace listed the Session may be in another
                    // one, which has to be entered for the view to follow it.
                    if choice.workspace_id != workspace_id {
                        if let Some(workspace) = client
                            .list_workspaces()?
                            .into_iter()
                            .find(|workspace| workspace.id == choice.workspace_id)
                        {
                            return Ok(RunOutcome::OpenWorkspace {
                                workspace: Box::new(workspace),
                                session_id: Some(choice.session_id),
                                open_interactions: false,
                            });
                        }
                    }
                    return Ok(RunOutcome::OpenSession(choice.session_id));
                }
                Request::Interactions => {
                    ensure_listed(app, client)?;
                    if app.interactions.items.is_empty() {
                        app.push_log(LogEntry::warn("no live interactions on the server"));
                        continue;
                    }
                    let workspaces = client.list_workspaces()?;
                    app.set_view(crate::app::View::Events);
                    app.focus = Focus::List;
                    app.interactions.show(workspaces);
                }
                // The listing the loop keeps, as `a` takes, so the choice offered
                // is of the work running at the last refresh.
                // `V` offers what the navigator would list rather than only live
                // work, so it opens on the list itself: the grid tiles live work
                // alone and could not show most of what it offers.
                Request::WorkspaceJump { listed } => {
                    ensure_listed(app, client)?;
                    let workspaces = client.list_workspaces()?;
                    let interactions = &app.interactions.items;
                    let mut jump = if listed {
                        WorkspaceJump::listed(&workspaces, interactions, |interaction| {
                            app.interactions.lists(interaction)
                        })
                    } else {
                        WorkspaceJump::new(&workspaces, interactions)
                    };
                    if jump.is_empty() {
                        app.show_warning(if listed {
                            "no Workspace has interactions the list shows"
                        } else {
                            "no Workspace has live interactions"
                        });
                        continue;
                    }
                    if !listed && app.view == crate::app::View::Overview {
                        jump.grid = Some((
                            app.overview.clone(),
                            app.interactions.only_current_workspace,
                        ));
                    }
                    app.interactions.workspaces = workspaces;
                    app.workspace_jump = Some(jump);
                    preview_workspace_jump(app);
                    narrow_navigator_to_jump(app);
                }
                // The overview reads the same snapshot the loop refreshes on its
                // timer, but the tiles name their Workspaces, and that list is
                // only fetched when asked for. It is best-effort: a grid a moment
                // stale is still the grid.
                Request::Overview => {
                    if let Ok(workspaces) = client.list_workspaces() {
                        app.interactions.workspaces = workspaces;
                    }
                    app.interactions.close();
                    app.overview.open();
                    app.set_view(crate::app::View::Overview);
                    app.focus = Focus::List;
                }
                // Loaded outright, as `n` does. A load that fails leaves the
                // overview up, and says so where the operator is looking rather
                // than only in the log.
                Request::ShowInteraction(id) => {
                    if id == app.session_id {
                        app.set_view(crate::app::View::Events);
                        continue;
                    }
                    let Some(interaction) = app.interactions.current(&id).cloned() else {
                        app.show_warning("that interaction is no longer running");
                        continue;
                    };
                    make_interaction_current(app, live, client, standing_launch, interaction);
                    if app.session_id != id {
                        app.show_error(
                            "could not open that interaction; the log (ctrl-l) says why",
                        );
                    }
                }
                // As above, but the grid stays up behind the message box, so the
                // operator can go on to the next tile once this one is answered.
                Request::MessageInteraction(id) => {
                    let Some(interaction) = app.interactions.current(&id).cloned() else {
                        app.show_warning("that interaction is no longer running");
                        continue;
                    };
                    make_interaction_current(app, live, client, standing_launch, interaction);
                    app.set_view(crate::app::View::Overview);
                    if app.session_id == id {
                        app.enter_input();
                    } else {
                        app.show_error(
                            "could not open that interaction; the log (ctrl-l) says why",
                        );
                    }
                }
                // The interaction is loaded outright rather than cursored, and the
                // navigator is left as it was: open or closed. Newly idle work
                // takes priority; otherwise this walks every live interaction.
                Request::NextLiveInteraction => {
                    // The client's snapshot is at most a refresh old, which is
                    // fresh enough to step by: one stopped since then still loads.
                    let overview =
                        (app.view == crate::app::View::Overview).then(|| app.overview.clone());
                    let next = if let Some(overview) = &overview {
                        overview.next_attention_or_live(
                            &app.interactions,
                            &app.session_id,
                            app.workspace.id.as_deref(),
                        )
                    } else {
                        app.interactions
                            .next_attention_or_live(&app.session_id, app.workspace.id.as_deref())
                    };
                    let Some(next) = next else {
                        app.show_warning("no other interaction is running");
                        continue;
                    };
                    let target_id = next.id.clone();
                    make_interaction_current(app, live, client, standing_launch, next);
                    if let Some(mut overview) = overview {
                        if app.session_id == target_id {
                            if overview.running_only
                                && overview
                                    .indices(&app.interactions, app.workspace.id.as_deref())
                                    .iter()
                                    .all(|index| {
                                        app.interactions.items[*index].id != app.session_id
                                    })
                            {
                                overview.toggle_running_only();
                            }
                            overview.open();
                        }
                        app.overview = overview;
                        app.set_view(crate::app::View::Overview);
                    }
                }
                Request::NextWorkingInteraction => {
                    let Some(next) = app
                        .interactions
                        .next_active(&app.session_id, app.workspace.id.as_deref())
                    else {
                        app.show_warning("no other interaction is actively working");
                        continue;
                    };
                    make_interaction_current(app, live, client, standing_launch, next);
                }
                Request::NewSession => return Ok(RunOutcome::NewSession),
                // Naming the branch asks the model for a topic and the checkout
                // copies the repository out, which together take long enough to
                // look like a hang. `Ctrl+W` and `Ctrl-Enter` differ only in what the
                // checkout is made for: a Session already running, which has to be
                // restarted into it — carrying the message, if `Ctrl-Enter` sent
                // one — or one that the first prompt launches there.
                Request::CreateWorktree { message } => {
                    let message = match message {
                        Some(message) if app.session_id.is_empty() => {
                            // The launch branches and checks out before it sends,
                            // and reports its own failure — restoring the message
                            // box.
                            blocked_on(terminal, app, CREATING_WORKTREE, |app| {
                                session::submit_message(
                                    app,
                                    client,
                                    &workspace_id,
                                    live,
                                    message,
                                    true,
                                )
                            })?;
                            continue;
                        }
                        message => message,
                    };
                    let session_id = app.session_id.clone();
                    let created = blocked_on(terminal, app, CREATING_WORKTREE, |_| {
                        client.create_session_worktree(&session_id)
                    })?;
                    if let Err(error) = created {
                        app.show_error(format!("could not create a linked workspace: {error}"));
                        // Nothing was sent: the message waits in the box for a
                        // plain Enter, or for another try.
                        if let Some(message) = message {
                            app.set_input(message);
                            app.enter_input();
                        }
                        continue;
                    }
                    if let Some(message) = message {
                        let sent = blocked_on(
                            terminal,
                            app,
                            "restarting the interaction in the linked workspace…",
                            |app| session::restart_and_send(app, client, live, message),
                        )?;
                        if sent {
                            return Ok(RunOutcome::OpenSession(session_id));
                        }
                        app.show_error(
                            "created the linked workspace, but could not restart in it; the message is back in the box",
                        );
                        app.enter_input();
                        continue;
                    }
                    // The checkout is read when the agent launches, so the
                    // interaction is restarted into it rather than left running in
                    // the directory it started in. Reopening the Session afterwards
                    // is what puts the checkout on screen: the view then reads its
                    // directory from the revived interaction.
                    let restarted = blocked_on(
                        terminal,
                        app,
                        "restarting the interaction in the linked workspace…",
                        |app| session::restart(app, client, live),
                    )?;
                    match restarted {
                        Ok(_) => return Ok(RunOutcome::OpenSession(session_id)),
                        Err(error) => app.show_error(format!(
                            "created the linked workspace, but could not restart in it ({error}); it will be used when this Session next launches"
                        )),
                    }
                }
                Request::ApplySelection => {
                    let Attachment::Attached { .. } = live else {
                        continue;
                    };
                    let selection = app.selection.clone();
                    match client.set_session_selection(&app.session_id, &selection) {
                        Ok(()) => {
                            app.show_action_message(format!("model set to {}", selection.model))
                        }
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not switch to {}: {error:#}",
                            selection.model
                        ))),
                    }
                }
                Request::ConvertProvider(selection) => {
                    let previous = app.selection.clone();
                    app.set_selection(selection.clone());
                    let restarted = blocked_on(
                        terminal,
                        app,
                        "switching provider and restarting the interaction…",
                        |app| session::restart(app, client, live),
                    )?;
                    match restarted {
                        Ok(_) => return Ok(RunOutcome::OpenSession(app.session_id.clone())),
                        Err(error) => {
                            app.set_selection(previous);
                            app.show_error(format!("could not switch provider: {error:#}"));
                        }
                    }
                }
                Request::Templates => {
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
                Request::ChangeWorkspaceLaunch {
                    change,
                    clear_interaction,
                } => {
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
                Request::Answer { contract } => {
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
                Request::Quota => refresh_quota(app, client),
                Request::Processes => {
                    refresh_processes(app, client);
                }
                // The setting is the server's to keep — it is stored with the
                // Session and acted on long after this client has gone — so what
                // is shown is what the server accepted, not what was pressed.
                Request::SetAutoRetry(enabled) => {
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
                            let message =
                                format!("could not change the rate-limit retry: {error:#}");
                            app.show_error(message.clone());
                            app.push_log(LogEntry::error(message));
                        }
                    }
                }
                Request::SetAutoCommit(enabled) => {
                    match client.set_interaction_auto_commit(&app.session_id, enabled) {
                        Ok(()) => {
                            app.auto_commit = enabled;
                            app.show_action_message(if enabled {
                                "auto-commit on — each turn's work is committed when it goes idle"
                            } else {
                                "auto-commit off"
                            });
                        }
                        Err(error) => {
                            let message = format!("could not change auto-commit: {error:#}");
                            app.show_error(message.clone());
                            app.push_log(LogEntry::error(message));
                        }
                    }
                }
                Request::EditFile => {
                    let Some(path) = app.selected_file_path() else {
                        continue;
                    };
                    open_path(app, config, &path, None);
                }
                Request::OpenPath(path) => open_path(app, config, &path, None),
                Request::OpenPathAt(path, line) => open_path(app, config, &path, line),
                Request::OpenUrl(url) => open_url(app, config, &url),
                Request::OpenDirectory => {
                    let Some(directory) = app.workspace.working_directory_or_current() else {
                        app.show_warning("no directory to open a terminal in");
                        continue;
                    };
                    match crate::terminal::open_directory(&directory, config) {
                        Ok(program) => app.show_action_message(format!(
                            "opened {program} in {}",
                            directory.display()
                        )),
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not open a terminal in {}: {error:#}",
                            directory.display()
                        ))),
                    }
                }
                Request::ShowDiffAt {
                    id,
                    path,
                    line,
                    fullscreen,
                } => {
                    let Some(interaction) = app.interactions.items.iter().find(|row| row.id == id)
                    else {
                        app.show_error("no interaction to diff");
                        continue;
                    };
                    let target = match crate::checkout_diff::target(client, interaction) {
                        Ok(target) => target,
                        Err(error) => {
                            app.show_error(format!("could not open snapshot diff: {error:#}"));
                            continue;
                        }
                    };
                    let mut diff = styra_diff::CheckoutDiff::read(target);
                    if let Err(reason) = diff.jump_to(&path, line) {
                        app.show_warning(reason);
                        continue;
                    }
                    app.checkout_diff = Some(diff);
                    edit_reports.load(app, client);
                    diff_refreshed = Instant::now();
                    app.diff_preview = !fullscreen;
                    app.diff_interaction_focus = false;
                    app.diff_turn = false;
                    if fullscreen {
                        app.set_view(View::CheckoutDiff);
                    } else if !matches!(app.view, View::Events | View::Files) {
                        app.set_view(View::Events);
                    }
                }
                Request::ShowDiff(id) => {
                    let Some(interaction) = app.interactions.items.iter().find(|row| row.id == id)
                    else {
                        app.show_error("no interaction to diff");
                        continue;
                    };
                    let target = match crate::checkout_diff::target(client, interaction) {
                        Ok(target) => target,
                        Err(error) => {
                            app.show_error(format!("could not open snapshot diff: {error:#}"));
                            continue;
                        }
                    };
                    app.checkout_diff = Some(crate::checkout_diff::reopen(
                        app.checkout_diff.take(),
                        target,
                    ));
                    edit_reports.load(app, client);
                    diff_refreshed = Instant::now();
                    app.diff_preview = true;
                    app.diff_interaction_focus = false;
                    app.diff_turn = false;
                    if !matches!(app.view, View::Events | View::Files) {
                        app.set_view(View::Events);
                    }
                }
                Request::ShowDiffFullscreen(id) => {
                    let Some(interaction) = app.interactions.items.iter().find(|row| row.id == id)
                    else {
                        app.show_error("no interaction to diff");
                        continue;
                    };
                    let target = match crate::checkout_diff::target(client, interaction) {
                        Ok(target) => target,
                        Err(error) => {
                            app.show_error(format!("could not open snapshot diff: {error:#}"));
                            continue;
                        }
                    };
                    app.checkout_diff = Some(crate::checkout_diff::reopen(
                        app.checkout_diff.take(),
                        target,
                    ));
                    edit_reports.load(app, client);
                    diff_refreshed = Instant::now();
                    app.diff_preview = false;
                    app.diff_interaction_focus = false;
                    app.diff_turn = false;
                    app.set_view(View::CheckoutDiff);
                }
                Request::ProjectAction(action) => {
                    let directory = match project_action_directory(app, client) {
                        Ok(directory) => directory,
                        Err(error) => {
                            app.show_error(format!("could not locate the project: {error:#}"));
                            continue;
                        }
                    };
                    let result = crate::terminal::project_command(
                        &directory,
                        &app.launch.effective(),
                        action,
                    )
                    .and_then(|command| {
                        let program =
                            crate::terminal::open_project_action(&directory, &command, config)?;
                        Ok((program, command))
                    });
                    match result {
                        Ok((program, command)) => app.show_action_message(format!(
                            "{}: {command} — opened in {program}",
                            action.name()
                        )),
                        Err(error) => app.show_error(format!(
                            "could not {} the project: {error:#}",
                            action.name()
                        )),
                    }
                }
                Request::GenerateEditReport => edit_reports.start(app, client),
                Request::OpenShell => {
                    match crate::terminal::open_shell(client, &app.session_id, config) {
                        Ok(program) => {
                            app.show_action_message(format!("opened shell in {program}"))
                        }
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not open session shell: {error:#}"
                        ))),
                    }
                }
            }
        }
        dirty |= app.notices.expire();
        dirty |= app.interactions.expire_flashes();
        dirty |= launch_effects.apply_ready(app, &workspace_id);
        dirty |= edit_reports.apply_ready(app);
        dirty |= audio.apply_ready(app, client);
        // Workspace launch policy is a server-owned read model. Refresh it
        // independently of input so edits from another Styra client flow into
        // this Driva view and invalidate its planned options.
        if let Ok(policy) = client.workspace_launch(&workspace_id) {
            if policy != app.launch.workspace {
                app.launch.sync_workspace(policy);
                dirty = true;
            }
        }
        dirty |= session::ensure_driva_plan(app, client, &workspace_id);
        let mut disconnected = false;
        if let Attachment::Attached { cursor } = live {
            match client.updates(&app.session_id, *cursor) {
                Ok(batch) => {
                    *cursor = batch.next;
                    dirty |= !batch.updates.is_empty();
                    for sequenced in batch.updates {
                        session::apply_update(app, sequenced.update);
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        target: "styra_tui::rpc",
                        pid = std::process::id(),
                        session_id = %app.session_id,
                        error = %error,
                        "update poll failed"
                    );
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
            dirty = true;
        }

        // Keep this snapshot fresh even with the navigator closed: it is what
        // lets the footer report a different interaction becoming idle while
        // the operator is reading or working in this one.
        if interactions_refreshed.elapsed() >= INTERACTIONS_REFRESH {
            interactions_refreshed = Instant::now();
            if let Some(id) = app.pending_rebase.clone() {
                match client.session_rebase_status(&id) {
                    Ok(status) => {
                        if status.finished {
                            app.pending_rebase = None;
                        }
                        app.rebase_status = Some(status);
                    }
                    Err(error) => {
                        app.pending_rebase = None;
                        if let Some(status) = app.rebase_status.as_mut() {
                            status.finished = true;
                            status.succeeded = false;
                            status.message = format!("Could not read rebase progress: {error:#}");
                        }
                    }
                }
                dirty = true;
            }
            if let Some(id) = app.pending_integration.clone() {
                match client.session_integration_result(&id) {
                    Ok(Some(result)) => {
                        app.pending_integration = None;
                        if result.completed {
                            app.show_action_message(result.message);
                        } else {
                            app.show_error(result.message);
                        }
                        dirty = true;
                    }
                    Err(error) => {
                        app.pending_integration = None;
                        app.show_error(format!("could not read integration result: {error:#}"));
                        dirty = true;
                    }
                    Ok(None) => {}
                }
            }
            interactions_poll.request(app.interactions_query());
        }
        if let Some(interactions) = interactions_poll.ready() {
            dirty |= app.interactions.refresh(interactions);
            dirty |= app.adopt_integration();
        }
        // An editor started work in the Workspace on screen and asked for it
        // to be watched. A blank screen is left alone: it is the operator
        // composing work of their own, not watching any.
        if let Some(claimed) = app.interactions.take_focus_claim() {
            if !app.session_id.is_empty()
                && app.workspace.id.as_deref() == Some(claimed.workspace_id.as_str())
            {
                make_interaction_current(app, live, client, standing_launch, claimed);
                dirty = true;
            }
        }

        if quota_refreshed.elapsed() >= QUOTA_REFRESH {
            quota_refreshed = Instant::now();
            dirty |= poll_quota(app, client);
        }

        if app.processes_visible() && processes_refreshed.elapsed() >= PROCESSES_REFRESH {
            processes_refreshed = Instant::now();
            dirty |= refresh_processes(app, client);
        }

        // The diff on screen is a reading of a checkout the agent may still be
        // writing to, so it is read again while the agent works. An idle or
        // stopped agent leaves the checkout alone: reopening refreshes it then.
        if app.diff_visible()
            && matches!(app.activity.status, Status::Running | Status::Background)
            && diff_refreshed.elapsed() >= DIFF_REFRESH
        {
            diff_refreshed = Instant::now();
            if let Some(diff) = app.checkout_diff.as_mut() {
                dirty |= diff.refresh();
            }
        }

        // A cursor that has come to rest loads the interaction under it. Until
        // then the navigator says that row is loading and the screen below is
        // still the interaction it was.
        // A live tag-filter edit can move the navigator cursor when the row
        // under it stops matching. Do not load that row while the picker is
        // still open: loading replaces the whole App and would make Space
        // appear to close the modal. The settled cursor is loaded on the
        // first round after the picker closes instead.
        if cursored_interaction_is_loadable(app) {
            load_cursored_interaction(app, live, client, standing_launch);
            dirty = true;
        }

        // A mount changed while the agent sat idle. Restarted here rather than
        // where the key was pressed, so a Workspace mount is in the server's
        // policy before the resume merges it, and ahead of the queued send
        // below, so a waiting message is answered under the new mounts.
        if app.restart_for_mounts && app.workspace_launch_pending == 0 {
            app.restart_for_mounts = false;
            dirty = true;
            if let Attachment::Attached { .. } = live {
                if app.activity.status.is_idle() {
                    let restarted = blocked_on(
                        terminal,
                        app,
                        "restarting the interaction with the new mounts…",
                        |app| session::restart_for_mounts(app, client, live),
                    )?;
                    match restarted {
                        Ok(()) => app.show_action_message("restarted with the new mounts"),
                        // Logged as well as shown: the notice is one line, cut
                        // to the terminal's width, and the cause is the part
                        // at the end of it.
                        Err(session::MountRestartError::Rejected(error)) => {
                            app.push_log(LogEntry::error(format!(
                                "the new mounts cannot be launched, so the interaction was not restarted: {error:#}"
                            )));
                            app.show_error(format!(
                                "mounts not applied, still running on the old ones: {error:#}"
                            ));
                        }
                        Err(session::MountRestartError::Failed(error)) => {
                            app.push_log(LogEntry::error(format!(
                                "could not restart with the new mounts: {error:#}"
                            )));
                            app.show_error(format!(
                                "could not restart with the new mounts ({error:#}); they apply when this Session next launches"
                            ));
                        }
                    }
                } else {
                    // A turn started before the restart could: leave it be.
                    app.show_action_message(
                        "the agent is working — the new mounts apply when this Session next launches",
                    );
                }
            }
        }

        if let Attachment::Attached { .. } = live {
            if app.activity.status.is_idle() && app.outbox.queued_count() > 0 {
                dirty = true;
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

        dirty |= crate::branch_log::sync(app, client);
        dirty |= crate::turn_snapshot::sync(app, client);
        dirty |= app.sync_turn_diff();
        dirty |= app.activity.note_progress();

        // The parts of a frame that are read off the clock have nothing to
        // announce them, so they are read on their own second rather than on
        // every round — and the frame is repainted only where that reading
        // differs from the one already on screen. A clock that has not moved
        // since the last frame is not a reason to paint another one, however
        // often it is consulted: see `presentation::clock_reading`.
        if ticked.elapsed() >= REDRAW_TICK {
            ticked = Instant::now();
            let reading = presentation::clock_reading(app);
            if reading != clock {
                // A frame repainted for nothing but the clock is the one kind
                // the trace cannot otherwise account for, and "why is it still
                // rendering" is exactly what this log gets read to answer. The
                // reading itself is logged, so consecutive lines show what
                // moved.
                tracing::debug!(
                    target: "styra_tui::render",
                    pid = std::process::id(),
                    session_id = %app.session_id,
                    moving = ?reading,
                    "repainting for the clock"
                );
                clock = reading;
                dirty = true;
            }
        }

        // A capture paints its own frames at the meter's rate, so the round's
        // own frame would be one more painting of what is already there.
        if dirty && !audio.is_recording() {
            if app.help.is_open() {
                // The reference is for the window underneath it, which cannot
                // change while it is open, so it is read from the app rather than
                // remembered when `?` was pressed.
                let window = presentation::current_window(app);
                let rows = presentation::help_rows(window);
                let feedback = terminal.render_help(
                    window.name(),
                    &rows,
                    &crate::keybindings::CLOSE_REFERENCE.label(),
                    app.help.offset(),
                )?;
                if let Some(scroll) = feedback
                    .scroll
                    .iter()
                    .find(|scroll| scroll.panel == styra_ui::PanelId::Help)
                {
                    app.help
                        .apply_feedback(scroll.limit, scroll.effective_offset);
                }
            } else if let Some(picker) = &app.template_picker {
                match &picker.templates {
                    Some(templates) => {
                        terminal.render_template_picker(
                            templates,
                            &picker.chosen,
                            picker.cursor,
                        )?;
                    }
                    None => {
                        terminal.render_template_picker_loading()?;
                    }
                }
            } else {
                let feedback = presentation::draw_application(terminal, app)?;
                presentation::apply_feedback(app, &feedback);
            }
            dirty = false;
        }

        let paste_active = message_paste_active(app) && !audio.is_recording();
        if bracketed_paste != paste_active {
            terminal.set_bracketed_paste(paste_active)?;
            bracketed_paste = paste_active;
        }
        let waited = if let Some(key) = replay.pop_front() {
            Some(Event::Key(key))
        } else if audio.is_recording() {
            meter_frames(terminal, app, &mut audio)?
        } else {
            terminal.poll_event(KEY_POLL)?
        };
        // A terminal that changed size has to be repainted, and nothing about
        // the application changed to say so. This used to be carried by the
        // unconditional frame at the top of every round, which is exactly what
        // is no longer there.
        if let Some(Event::Resize(..)) = &waited {
            dirty = true;
        }
        if let Some(Event::Paste(text)) = &waited {
            if paste_active {
                app.composer.paste(text);
                dirty = true;
            }
            continue;
        }
        let Some(Event::Key(key)) = waited else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        // Every key is answered on screen — by what it does, or by the notice
        // saying why it did nothing — and the handlers below are spread across
        // a dozen modal windows. Marking the frame here, once, is what keeps
        // that from being a promise each of them has to remember to keep.
        dirty = true;

        // While the reference is open it is modal, so none of the commands
        // described by it can accidentally act on the window underneath. It
        // comes before every other modal because it can be opened over them.
        if app.help.is_open() {
            match key {
                k if keys::CLOSE_REFERENCE.matches(k) => app.help.close(),
                // The reference is taller than a short terminal, so the
                // sections at the end have to be reachable.
                k if keys::REFERENCE_DOWN.matches(k) => app.help.line_down(),
                k if keys::REFERENCE_UP.matches(k) => app.help.line_up(),
                k if keys::REFERENCE_PAGE_DOWN.matches(k) => app.help.page_down(),
                k if keys::REFERENCE_PAGE_UP.matches(k) => app.help.page_up(),
                k if keys::REFERENCE_TOP.matches(k) => app.help.scroll_to_top(),
                _ => {}
            }
            continue;
        }

        if let Some(status) = &app.rebase_status {
            if status.finished && matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                app.rebase_status = None;
            }
            continue;
        }

        // `?` describes whichever window is showing, so every key-driven one
        // answers it — the launcher, the template chooser and the modal
        // overlays included. The prompts that take typed text are excluded:
        // there a `?` is a character of what is being typed.
        if keys::HELP.matches(key)
            && app.insert.is_none()
            && app.git_repository_prompt.is_none()
            && app.retarget_prompt.is_none()
            && app.integration_prompt.is_none()
            && app.launch.prompt.is_none()
            && app.launch.command_prompt.is_none()
            && !app.search.typing()
            // The diff owns the keyboard; `?` is not one of its keys.
            && !app.diff_focused()
            && app
                .tag_picker
                .as_ref()
                .is_none_or(|picker| picker.new_tag.is_none())
            && (app.focus == Focus::List || app.launcher.is_some() || app.template_picker.is_some())
        {
            app.help.open();
            continue;
        }

        // The comparison picker owns typed text and navigation while open.
        if let Some(picker) = app.diff_branch_picker.as_mut() {
            match picker.handle_key(key) {
                crate::checkout_diff::BranchChoice::Stay => {}
                crate::checkout_diff::BranchChoice::Cancel => app.diff_branch_picker = None,
                crate::checkout_diff::BranchChoice::Select(branch) => {
                    let id = picker.id.clone();
                    let result = client.set_diff_branch(&id, branch).and_then(|()| {
                        let interaction = app
                            .interactions
                            .items
                            .iter()
                            .find(|row| row.id == id)
                            .ok_or_else(|| anyhow::anyhow!("no interaction to diff"))?;
                        crate::checkout_diff::target(client, interaction)
                    });
                    match result {
                        Ok(target) => {
                            app.checkout_diff = Some(styra_diff::CheckoutDiff::read(target));
                            app.diff_turn = false;
                            app.diff_branch_picker = None;
                            edit_reports.load(app, client);
                        }
                        Err(error) => {
                            app.show_error(format!("could not change comparison branch: {error:#}"))
                        }
                    }
                }
            }
            continue;
        }
        // The action menu is typed at, so while it is open it owns every key
        // but the `?` above. What it runs is pressed back through this loop,
        // so a command chosen from it takes the same path as its own key.
        if let Some(menu) = app.action_menu.as_mut() {
            match menu.handle_key(key) {
                action_menu::Outcome::Stay => {}
                action_menu::Outcome::Cancel => app.action_menu = None,
                action_menu::Outcome::Run(presses) => {
                    app.action_menu = None;
                    replay.extend(presses);
                }
            }
            continue;
        }
        // Typed at too, so it owns the keys the same way.
        if let Some(jump) = app.workspace_jump.as_mut() {
            let outcome = jump.handle_key(key);
            let grid = match outcome {
                workspace_jump::Outcome::Stay => {
                    preview_workspace_jump(app);
                    narrow_navigator_to_jump(app);
                    continue;
                }
                _ => app.workspace_jump.take().and_then(|jump| jump.grid),
            };
            narrow_navigator_to_jump(app);
            match (outcome, grid) {
                (workspace_jump::Outcome::Cancel, Some((overview, only_current_workspace))) => {
                    app.overview = overview;
                    app.interactions.only_current_workspace = only_current_workspace;
                }
                // Over the grid the choice stays on the grid: its cursor is
                // already on the Workspace, and loads the tile as a step would.
                (workspace_jump::Outcome::Open(id), Some(_)) => {
                    if !select_workspace_in_grid(app, &id) {
                        app.show_warning("the grid's filters hide that Workspace's interactions");
                    }
                    input::follow_overview_cursor(app);
                }
                (workspace_jump::Outcome::Open(id), None) => {
                    open_interactions_in_workspace(app, client, &id)?;
                }
                // A fresh interaction is asked for outright, as the Workspace
                // picker's Ctrl-N asks for one. The summary the jump was built
                // from stands in if the server cannot answer.
                (workspace_jump::Outcome::New(id), _) => {
                    let workspace = client.workspace(&id).or_else(|error| {
                        app.interactions
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.id == id)
                            .cloned()
                            .ok_or(error)
                    })?;
                    return Ok(RunOutcome::OpenWorkspace {
                        workspace: Box::new(workspace),
                        session_id: None,
                        open_interactions: false,
                    });
                }
                _ => {}
            }
            continue;
        }
        if keys::GLOBAL_ACTION_MENU.matches(key) && action_menu_available(app) {
            // A `z` pressed before Space would otherwise swallow the first key
            // the menu runs.
            pending_fold = false;
            app.action_menu = Some(ActionMenu::new(presentation::current_window(app)));
            continue;
        }

        // Template discovery and choice are part of this root loop. Even the
        // loading state is cancellable, and no nested event loop can defer the
        // edit produced by Enter until some unrelated future keypress.
        if let Some(template_picker) = app.template_picker.as_mut() {
            let action = template_picker.handle_key(key);
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
        if app.retarget_prompt.is_some() {
            input::handle_retarget_prompt_key(app, client, key);
            continue;
        }
        if app.integration_prompt.is_some() {
            input::handle_integration_prompt_key(app, key);
            if let Some(request) = app.take_request() {
                let id = app.session_id.clone();
                match request {
                    Request::GenerateIntegrationMessage => {
                        let generated =
                            blocked_on(terminal, app, "generating commit message…", |_| {
                                client.generate_integration_message(&id)
                            })?;
                        match generated {
                            Ok(message) => app.integration_prompt = Some(message),
                            Err(error) => app.show_error(format!(
                                "could not generate commit message: {error:#}"
                            )),
                        }
                    }
                    Request::Integrate(message) => match client.integrate_session(&id, &message) {
                        Ok(()) => {
                            app.close_integration_prompt();
                            app.pending_integration = Some(id.clone());
                            app.show_action_message(
                                "integration started — progress appears in the interaction log",
                            );
                        }
                        Err(error) => app.show_error(format!("could not integrate: {error:#}")),
                    },
                    _ => {}
                }
            }
            continue;
        }
        if app.branch_prompt.is_some() {
            input::handle_branch_prompt_key(app, client, key);
            // Confirming has already branched; open the result on this key
            // rather than leaving the switch queued behind the next one.
            match app.take_open_session_request() {
                Some(Request::OpenSession(id)) => return Ok(RunOutcome::OpenSession(id)),
                Some(Request::OpenBranch(id)) => return Ok(RunOutcome::OpenBranch(id)),
                _ => {}
            }
            continue;
        }

        // The link menu is modal over link navigation, so it owns the keys
        // that would otherwise move the highlight or open the link directly.
        if app.link_menu.is_some() {
            input::handle_link_menu_key(app, key);
            continue;
        }

        // The message editor's path prompt is modal, and its second question is
        // answered by a bare letter that means something else everywhere else,
        // so it is handled ahead of the reference.
        if app.insert.is_some() {
            input::handle_insert_key(app, key);
            continue;
        }
        // The Git checkout is Workspace metadata. Its prompt is modal so a
        // path such as `git@host:group/project` is input rather than keys.
        if app.git_repository_prompt.is_some() {
            input::handle_git_repository_prompt_key(app, client, key);
            continue;
        }
        // The project command prompt owns typed characters too.
        if app.launch.command_prompt.is_some() {
            input::handle_command_prompt_key(app, key);
            continue;
        }
        // So is the Driva view's mount prompt: what is typed into it is part
        // of a path, including the characters that are shortcuts elsewhere.
        if app.launch.prompt.is_some() {
            input::handle_mount_prompt_key(app, key);
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
        // Tags are edited as one modal operation, so typing a new tag cannot
        // trigger the navigator or any global shortcut underneath it.
        if let Some(picker) = app.tag_picker.as_mut() {
            if picker.new_tag.is_some() {
                match key.code {
                    KeyCode::Esc => picker.new_tag = None,
                    KeyCode::Enter => {
                        picker.add_new();
                        let tags = picker.selected.clone();
                        let id = app.session_id.clone();
                        save_tags(app, client, id, tags);
                    }
                    KeyCode::Backspace => {
                        picker.new_tag.as_mut().expect("new tag is present").pop();
                    }
                    KeyCode::Char(ch) if !ch.is_control() => picker
                        .new_tag
                        .as_mut()
                        .expect("new tag is present")
                        .push(ch),
                    _ => {}
                }
                continue;
            }
            match key {
                k if keys::TAGS_NEXT.matches(k) => picker.next(),
                k if keys::TAGS_PREV.matches(k) => picker.previous(),
                k if keys::TAGS_PAGE_DOWN.matches(k) => picker.page_down(),
                k if keys::TAGS_PAGE_UP.matches(k) => picker.page_up(),
                k if keys::TAGS_TOGGLE.matches(k) => toggle_tag_picker_selection(app),
                k if keys::TAGS_EXCLUDE.matches(k)
                    && picker.purpose == crate::tag_picker::TagPurpose::Filter =>
                {
                    toggle_tag_picker_exclusion(app)
                }
                k if keys::TAGS_CLEAR.matches(k) => clear_tag_picker_selection(app),
                k if keys::TAGS_NEW.matches(k) => picker.start_new(),
                k if keys::TAGS_CANCEL.matches(k) => {
                    restore_tag_picker_selection(app);
                    app.tag_picker = None;
                }
                k if keys::TAGS_SAVE.matches(k) => {
                    let tags = picker.selected.clone();
                    let id = app.session_id.clone();
                    match picker.purpose {
                        crate::tag_picker::TagPurpose::Edit => save_tags(app, client, id, tags),
                        crate::tag_picker::TagPurpose::Filter => {
                            app.tag_picker = None;
                        }
                    }
                }
                _ => {
                    if let Some(edit) = Edit::from_key(key) {
                        picker.type_query(edit);
                    }
                }
            }
            continue;
        }
        // The diff owns reading keys in either layout, including its modal
        // file filter, before the event list and navigator handle them.
        if input::handle_diff_focus_key(app, key) {
            continue;
        }
        if app.diff_focused() {
            if keys::DIFF_BRANCH.matches(key) && !app.shown_diff().is_some_and(|diff| diff.typing())
            {
                match client.diff_branches(&app.session_id) {
                    Ok(branches) => {
                        app.diff_branch_picker = Some(crate::checkout_diff::BranchPicker::new(
                            app.session_id.clone(),
                            branches,
                        ))
                    }
                    Err(error) => {
                        app.show_error(format!("could not list comparison branches: {error:#}"))
                    }
                }
                continue;
            }
            input::handle_diff_key(app, key);
            continue;
        }
        if app.search.typing() {
            input::handle_search_key(app, key);
            continue;
        }
        // The embedded interaction list owns navigation while it is open.
        // Moving its cursor makes that interaction current once the cursor
        // rests, so the list can be walked across faster than interactions can
        // be loaded. Enter only closes the navigator; there is no preview.
        if app.interactions.open && app.focus == Focus::List {
            let session_id = app.session_id.clone();
            // The `/` filter is modal while it is being typed, as the event
            // list's search is: every printable key is part of the term. The
            // arrows still walk what it leaves standing. Enter ends the typing
            // and goes on to open the row under the cursor, as Enter on the
            // list does, rather than needing a second press.
            if app.interactions.typing_filter() && key.code == KeyCode::Enter {
                app.interactions.finish_filter();
            } else if app.interactions.typing_filter() {
                let workspace_id = app.workspace.id.clone();
                match key.code {
                    KeyCode::Esc => app.interactions.clear_filter(),
                    KeyCode::Down => app
                        .interactions
                        .cursor_next(&session_id, workspace_id.as_deref()),
                    KeyCode::Up => app
                        .interactions
                        .cursor_previous(&session_id, workspace_id.as_deref()),
                    KeyCode::PageDown => {
                        app.interactions
                            .cursor_page(&session_id, workspace_id.as_deref(), true)
                    }
                    KeyCode::PageUp => {
                        app.interactions
                            .cursor_page(&session_id, workspace_id.as_deref(), false)
                    }
                    _ => {
                        if let Some(edit) = Edit::from_key(key) {
                            app.interactions
                                .type_filter(edit, &session_id, workspace_id.as_deref())
                        }
                    }
                }
                continue;
            }
            match key {
                k if keys::INTERACTIONS_FILTER.matches(k) => {
                    app.interactions.start_filter();
                    continue;
                }
                // Chosen from tags carried by the current live snapshot, with
                // the filter in force already marked.
                k if keys::INTERACTIONS_TAG_FILTER.matches(k) => {
                    app.tag_picker = Some(crate::tag_picker::TagPicker::filter(
                        app.interactions.live_tags(),
                        app.interactions.tag_filter().to_vec(),
                    ));
                    continue;
                }
                // Esc widens a filtered list back out before it closes it.
                k if k.code == KeyCode::Esc && app.interactions.filter().is_some() => {
                    app.interactions.clear_filter();
                    continue;
                }
                // In All scope the entries are grouped under Workspace
                // headings, and J/K skip whole groups: one press per
                // Workspace rather than one per interaction. They move the
                // cursor like j/k, so a skip across several groups costs no
                // more loads than a step across one row.
                // This skips idle work, so it only ever lands on an
                // interaction currently doing work.
                k if keys::INTERACTIONS_NEXT_WORKING.matches(k) => {
                    if app
                        .interactions
                        .cursor_to_next_active(&session_id, app.workspace.id.as_deref())
                        .is_none()
                    {
                        app.show_warning("no other interaction is actively working");
                    }
                    continue;
                }
                k if keys::INTERACTIONS_PREV_WORKING.matches(k) => {
                    if app
                        .interactions
                        .cursor_to_previous_active(&session_id, app.workspace.id.as_deref())
                        .is_none()
                    {
                        app.show_warning("no other interaction is actively working");
                    }
                    continue;
                }
                // Only how the rows are drawn changes, so the cursor's
                // pending move need not be loaded first.
                k if keys::INTERACTIONS_RESPONSES.matches(k) => {
                    app.interactions.toggle_responses();
                    continue;
                }
                k if keys::INTERACTIONS_NEXT_WORKSPACE.matches(k) => {
                    app.interactions
                        .cursor_next_workspace(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                k if keys::INTERACTIONS_PREV_WORKSPACE.matches(k) => {
                    app.interactions
                        .cursor_previous_workspace(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                k if keys::INTERACTIONS_NEXT.matches(k) => {
                    app.interactions
                        .cursor_next(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                k if keys::INTERACTIONS_PREV.matches(k) => {
                    app.interactions
                        .cursor_previous(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                k if keys::INTERACTIONS_FIRST.matches(k) => {
                    app.interactions
                        .cursor_first(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                k if keys::INTERACTIONS_PAGE_DOWN.matches(k) => {
                    app.interactions
                        .cursor_page(&session_id, app.workspace.id.as_deref(), true);
                    continue;
                }
                k if keys::INTERACTIONS_PAGE_UP.matches(k) => {
                    app.interactions
                        .cursor_page(&session_id, app.workspace.id.as_deref(), false);
                    continue;
                }
                k if keys::INTERACTIONS_LAST.matches(k) => {
                    app.interactions
                        .cursor_last(&session_id, app.workspace.id.as_deref());
                    continue;
                }
                _ => {}
            }
            // Unbound keys leave the navigator and its pending cursor alone.
            // Events and global shortcuts still act on the selected row.
            let mapped = [keys::Window::Interactions, keys::Window::Events]
                .into_iter()
                .flat_map(keys::Window::actions)
                .any(|binding| binding.matches(key))
                || keys::EVENTS_FOLD_PREFIX.matches(key);
            if !mapped {
                continue;
            }
            // Every other mapped key acts on the row under the cursor, so a move
            // still waiting out its settle is completed first rather than
            // abandoned.
            load_cursored_interaction(app, live, client, standing_launch);
            match key {
                k if keys::INTERACTIONS_CLOSE.matches(k) => {
                    app.interactions.close();
                    continue;
                }
                k if keys::INTERACTIONS_SCOPE.matches(k) => {
                    app.interactions.toggle_workspace_scope();
                    continue;
                }
                k if keys::INTERACTIONS_COMPLETED.matches(k) => {
                    app.interactions.toggle_completed();
                    continue;
                }
                k if keys::INTERACTIONS_STOPPED.matches(k) => {
                    app.interactions.toggle_stopped();
                    continue;
                }
                k if keys::INTERACTIONS_COMPLETE.matches(k)
                    || keys::INTERACTIONS_ABANDON.matches(k) =>
                {
                    let Some(interaction) = app.interactions.current(&app.session_id).cloned()
                    else {
                        continue;
                    };
                    let finished = if keys::INTERACTIONS_ABANDON.matches(k) {
                        CompletionState::Abandoned
                    } else {
                        CompletionState::Completed
                    };
                    // The cursor settle above already made this row the one
                    // on screen, so finishing it is the same action — and the
                    // same local status update — as the `C` and `Z` bound
                    // directly on the Events view.
                    if !session::finish_interaction(app, client, live, finished) {
                        continue;
                    }
                    // Completion is a Session property the server owns, so the
                    // row is still listed and the server is what says so: take
                    // its word for the new state rather than writing one here.
                    if let Ok(interactions) = client.list_interactions(&app.interactions_query()) {
                        app.interactions.refresh(interactions);
                    }
                    let workspace_id = app.workspace.id.clone();
                    let Some(next) = app
                        .interactions
                        .select_past_hidden(&interaction.id, workspace_id.as_deref())
                    else {
                        app.interactions.close();
                        return Ok(RunOutcome::Reset);
                    };
                    if next.id == interaction.id {
                        continue;
                    }
                    make_interaction_current(app, live, client, standing_launch, next);
                    continue;
                }
                k if keys::INTERACTIONS_TAGS.matches(k) => {
                    let Some(interaction) = app.interactions.current(&app.session_id) else {
                        continue;
                    };
                    match client.list_tags() {
                        Ok(tags) => {
                            app.tag_picker = Some(crate::tag_picker::TagPicker::new(
                                tags,
                                interaction.tags.clone(),
                            ))
                        }
                        Err(error) => app.show_error(format!("could not list tags: {error:#}")),
                    }
                    continue;
                }
                k if interaction_navigator_passthrough(k) => {}
                _ => app.interactions.close(),
            }
        }

        // The picker raises requests of its own (applying a model change to the
        // live session), so it falls through to the request match below rather
        // than skipping straight to the next frame.
        if app.launcher.is_some() {
            launcher::handle_key(app, key, preferences_path);
        } else {
            if keys::GLOBAL_LAUNCHER.matches(key) || keys::EDITOR_LAUNCHER.matches(key) {
                app.recent_models = preferences::load_recent_models(preferences_path);
            }
            // Grid navigation shares the navigator's pending cursor. Finish
            // its move before a command can act on the previous interaction.
            if app.focus == Focus::List
                && app.view == crate::app::View::Overview
                && !input::overview_navigation_key(key)
            {
                load_cursored_interaction(app, live, client, standing_launch);
            }
            match app.focus {
                Focus::List => input::handle_list_key(
                    app,
                    client,
                    live,
                    key,
                    &mut pending_fold,
                    preferences_path,
                ),
                Focus::Input => {
                    // A running capture owns the message box and its keys:
                    // there is no text being typed for them to mean anything
                    // else to, and the operator is speaking rather than
                    // looking for a modifier.
                    if audio.is_recording() {
                        match key {
                            k if keys::EDITOR_RECORD_FINISH.matches(k)
                                || keys::EDITOR_RECORD.matches(k) =>
                            {
                                audio.finish(app, client.clone())
                            }
                            k if keys::EDITOR_RECORD_CANCEL.matches(k) => {
                                audio.cancel(app, client.clone())
                            }
                            k if keys::EDITOR_RECORD_LOUDER.matches(k) => audio.boost(app),
                            k if keys::EDITOR_RECORD_QUIETER.matches(k) => audio.quieten(app),
                            _ => {}
                        }
                        continue;
                    }
                    if keys::EDITOR_RECORD.matches(key) {
                        audio.toggle(app, client.clone());
                        continue;
                    }
                    input::handle_input_key(app, client, &workspace_id, live, key)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

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
    fn message_paste_follows_editor_focus_and_modal_ownership() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.focus = Focus::List;
        assert!(!message_paste_active(&app));
        app.focus = Focus::Input;
        assert!(message_paste_active(&app));
        app.retarget_prompt = Some((String::new(), Vec::new()));
        assert!(!message_paste_active(&app));
        app.retarget_prompt = None;
        app.integration_prompt = Some(String::new());
        assert!(!message_paste_active(&app));
        app.integration_prompt = None;
        app.insert = Some(crate::insert::Prompt::new(None, None, false));
        assert!(!message_paste_active(&app));
        app.insert = None;
        app.help.open();
        assert!(!message_paste_active(&app));
        app.help.close();
        app.composer.add_part();
        app.composer.start_choosing();
        assert!(message_paste_active(&app));
        app.diff_preview = true;
        assert!(app.diff_visible());
        assert!(message_paste_active(&app));
        app.enter_list();
        assert!(!message_paste_active(&app));
    }

    /// Space is a letter wherever something is being typed and a link's own
    /// menu while one is highlighted, so the action menu answers it only on a
    /// window's list.
    #[test]
    fn the_action_menu_opens_only_from_a_windows_list() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.focus = Focus::List;
        assert!(action_menu_available(&app));
        app.focus = Focus::Input;
        assert!(!action_menu_available(&app));
        app.focus = Focus::List;
        app.search.open();
        assert!(!action_menu_available(&app));
        app.search.cancel();
        app.view = View::CheckoutDiff;
        assert!(!action_menu_available(&app), "the diff owns the keyboard");
        app.view = View::Overview;
        assert!(action_menu_available(&app));
        app.integration_prompt = Some(String::new());
        assert!(!action_menu_available(&app));
    }

    #[test]
    fn project_command_prompts_own_spaces_and_question_marks() {
        for action in styra_protocol::ProjectAction::ALL {
            let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
            app.view = View::Driva;
            app.focus = Focus::List;
            crate::launch::open_command_prompt(&mut app, action);
            assert!(!action_menu_available(&app));
            for character in "cargo run -- --help ?".chars() {
                input::handle_command_prompt_key(
                    &mut app,
                    crossterm::event::KeyEvent::from(KeyCode::Char(character)),
                );
            }
            assert_eq!(
                app.launch.command_prompt.as_ref().unwrap().1,
                "cargo run -- --help ?"
            );
            app.focus = Focus::Input;
            assert!(!message_paste_active(&app));
        }
    }

    #[test]
    fn global_actions_pass_through_the_navigator() {
        let press = |character| crossterm::event::KeyEvent::from(KeyCode::Char(character));
        assert!(interaction_navigator_passthrough(press('S')));
        assert!(interaction_navigator_passthrough(press('i')));
        assert!(interaction_navigator_passthrough(
            crossterm::event::KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL,)
        ));
        assert!(!interaction_navigator_passthrough(press('l')));
    }

    /// Opening the jump over the navigator keeps it open, listing only the
    /// Workspaces the jump's query leaves; closing the jump lists them all.
    #[test]
    fn the_workspace_jump_narrows_the_open_navigator() {
        use styra_protocol::{InteractionActivity, WorkspaceSummary};
        let press = |character| crossterm::event::KeyEvent::from(KeyCode::Char(character));
        assert!(interaction_navigator_passthrough(press('v')));
        assert!(interaction_navigator_passthrough(press('V')));

        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "one",
        );
        let mut other =
            crate::interactions::tests::interaction("two", InteractionActivity::Running);
        other.workspace_id = "other".into();
        let interactions = vec![
            crate::interactions::tests::interaction("one", InteractionActivity::Running),
            other,
        ];
        let workspaces: Vec<WorkspaceSummary> = ["workspace", "other"]
            .into_iter()
            .map(|id| WorkspaceSummary {
                id: id.into(),
                name: Some(id.into()),
                host_path: id.into(),
                git_repository: None,
                path: id.into(),
                session_count: 0,
                age: String::new(),
                created_at_ms: 0,
                last_accessed_at_ms: 0,
                launch: Default::default(),
            })
            .collect();
        let jump = WorkspaceJump::new(&workspaces, &interactions);
        app.interactions.open(interactions, workspaces);
        app.workspace_jump = Some(jump);
        narrow_navigator_to_jump(&mut app);
        assert_eq!(app.interactions.visible_indices(None), vec![0, 1]);

        for character in "oth".chars() {
            app.workspace_jump
                .as_mut()
                .unwrap()
                .handle_key(press(character));
        }
        narrow_navigator_to_jump(&mut app);
        assert_eq!(app.interactions.visible_indices(None), vec![1]);

        app.workspace_jump = None;
        narrow_navigator_to_jump(&mut app);
        assert!(app.interactions.open);
        assert_eq!(app.interactions.visible_indices(None), vec![0, 1]);
    }

    /// Over the grid, the jump narrows the grid to its highlighted Workspace
    /// and moves only the grid's cursor; nothing is scheduled to load.
    #[test]
    fn the_workspace_jump_previews_its_highlight_on_the_grid() {
        use styra_protocol::{InteractionActivity, WorkspaceSummary};
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "one",
        );
        app.workspace.id = Some("workspace".into());
        let mut other =
            crate::interactions::tests::interaction("two", InteractionActivity::Running);
        other.workspace_id = "other".into();
        let interactions = vec![
            crate::interactions::tests::interaction("one", InteractionActivity::Running),
            other,
        ];
        let workspaces: Vec<WorkspaceSummary> = ["workspace", "other"]
            .into_iter()
            .map(|id| WorkspaceSummary {
                id: id.into(),
                name: Some(id.into()),
                host_path: id.into(),
                git_repository: None,
                path: id.into(),
                session_count: 0,
                age: String::new(),
                created_at_ms: 0,
                last_accessed_at_ms: 0,
                launch: Default::default(),
            })
            .collect();
        let mut jump = WorkspaceJump::new(&workspaces, &interactions);
        app.interactions.open(interactions, workspaces);
        app.view = View::Overview;
        jump.grid = Some((app.overview.clone(), false));
        app.workspace_jump = Some(jump);

        app.workspace_jump
            .as_mut()
            .unwrap()
            .handle_key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Down,
            ));
        preview_workspace_jump(&mut app);

        assert!(app.interactions.only_current_workspace);
        assert_eq!(
            app.overview
                .indices(&app.interactions, Some("workspace"))
                .len(),
            1,
            "only the highlighted Workspace's tiles are shown"
        );
        assert_eq!(
            app.overview
                .selected_id(&app.interactions, "one", Some("workspace")),
            Some("two")
        );
        assert!(app.interactions.pending("one").is_none());
    }

    #[test]
    fn a_failed_grid_selection_load_rests_the_shared_cursor_and_keeps_the_grid() {
        let root = tempfile::tempdir().unwrap();
        let client = Client::new(root.path().join("missing.sock"));
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "one",
        );
        app.interactions.refresh(
            ["one", "two"]
                .into_iter()
                .map(|id| {
                    crate::interactions::tests::interaction(
                        id,
                        styra_protocol::InteractionActivity::Running,
                    )
                })
                .collect(),
        );
        app.view = View::Overview;
        app.overview.running_only = true;
        app.overview.note_rows(&[2]);
        app.overview.right(&app.interactions, "one", None);
        app.interactions.move_cursor_to("two".into(), "one");
        let mut live = Attachment::Detached;

        load_cursored_interaction(&mut app, &mut live, &client, &LaunchPolicy::default());

        assert_eq!(app.session_id, "one");
        assert_eq!(app.view, View::Overview);
        assert!(app.overview.running_only);
        assert_eq!(
            app.overview.selected_id(&app.interactions, "one", None),
            Some("two")
        );
        assert!(app.interactions.pending("one").is_none());
        assert!(!cursored_interaction_is_loadable(&app));
    }

    #[test]
    fn toggling_a_tag_filter_applies_it_immediately() {
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
        app.session_id = "one".into();
        let current = crate::interactions::tests::interaction(
            "one",
            styra_protocol::InteractionActivity::Pending,
        );
        let mut matching = crate::interactions::tests::interaction(
            "two",
            styra_protocol::InteractionActivity::Pending,
        );
        matching.tags = vec!["bug".into()];
        app.interactions.open(vec![current, matching], Vec::new());
        app.tag_picker = Some(crate::tag_picker::TagPicker::filter(
            vec!["bug".into()],
            Vec::new(),
        ));

        toggle_tag_picker_selection(&mut app);
        assert_eq!(app.interactions.tag_filter(), ["bug"]);
        assert!(app.tag_picker.is_some());
        assert!(app.interactions.pending("one").is_some());
        std::thread::sleep(Duration::from_millis(140));
        assert!(!cursored_interaction_is_loadable(&app));

        toggle_tag_picker_selection(&mut app);
        assert!(app.interactions.tag_filter().is_empty());
        assert!(app.tag_picker.is_some());

        toggle_tag_picker_selection(&mut app);
        clear_tag_picker_selection(&mut app);
        assert!(app.interactions.tag_filter().is_empty());
        assert!(app
            .tag_picker
            .as_ref()
            .is_some_and(|picker| picker.selected.is_empty()));

        app.tag_picker = Some(crate::tag_picker::TagPicker::filter(
            vec!["bug".into()],
            vec!["bug".into()],
        ));
        clear_tag_picker_selection(&mut app);
        restore_tag_picker_selection(&mut app);
        assert_eq!(app.interactions.tag_filter(), ["bug"]);
        assert_eq!(app.tag_picker.as_ref().unwrap().selected, ["bug"]);

        app.tag_picker = None;
        assert!(cursored_interaction_is_loadable(&app));
    }

    #[test]
    fn excluding_a_tag_filter_applies_it_immediately() {
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
        app.session_id = "one".into();
        let current = crate::interactions::tests::interaction(
            "one",
            styra_protocol::InteractionActivity::Pending,
        );
        let mut excluded = crate::interactions::tests::interaction(
            "two",
            styra_protocol::InteractionActivity::Pending,
        );
        excluded.tags = vec!["bug".into()];
        app.interactions.open(vec![current, excluded], Vec::new());
        app.tag_picker = Some(crate::tag_picker::TagPicker::filter(
            vec!["bug".into()],
            Vec::new(),
        ));

        toggle_tag_picker_exclusion(&mut app);
        assert_eq!(app.interactions.tag_filter(), ["-bug"]);
        assert_eq!(app.interactions.visible_indices(None), vec![0]);

        toggle_tag_picker_exclusion(&mut app);
        assert!(app.interactions.tag_filter().is_empty());
        assert_eq!(app.interactions.visible_indices(None), vec![0, 1]);
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

    /// A server that answers nothing, and says what it was asked.
    #[derive(Default)]
    struct Unreachable {
        asked: std::sync::Mutex<Vec<styra_protocol::Request>>,
    }

    impl styra_server::InProcessServer for Unreachable {
        fn handle(
            &self,
            request: styra_protocol::Request,
        ) -> anyhow::Result<styra_protocol::Response> {
            self.asked.lock().unwrap().push(request);
            anyhow::bail!("this test's server answers nothing")
        }

        fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn only_mapped_keys_dismiss_the_interaction_navigator() {
        for (code, stays_open) in [
            (KeyCode::Char('b'), true),
            (KeyCode::F(12), true),
            (KeyCode::Left, true),
            (KeyCode::Enter, false),
            (KeyCode::Esc, false),
            (KeyCode::Char('a'), false),
        ] {
            let mut app = App::new(
                styra_protocol::agent::Selection::parse("codex").unwrap(),
                "s1",
            );
            app.enter_list();
            app.interactions.show(Vec::new());
            let mut terminal = styra_ui::TestUi::new(100, 30).unwrap();
            terminal.script([Event::Key(crossterm::event::KeyEvent::from(code))]);
            let result = run(
                &mut terminal,
                &mut app,
                &Client::in_process(std::sync::Arc::new(Unreachable::default())),
                &mut Attachment::Detached,
                RunContext {
                    standing_launch: &LaunchPolicy::default(),
                    preferences_path: &std::env::temp_dir()
                        .join("styra-event-loop-no-preferences.toml"),
                    config: &crate::config::Defaults,
                },
            );
            assert!(result.is_err(), "the input script should be exhausted");
            assert_eq!(app.interactions.open, stays_open, "{code:?}");
        }
    }

    /// Run the loop over a fullscreen diff of two files, pressing `keys`.
    /// The loop ends with what the keys asked for or, once the script runs
    /// out, with an error.
    fn run_over_diff(keys: &str, server: std::sync::Arc<Unreachable>) -> Result<RunOutcome> {
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_list();
        app.view = View::CheckoutDiff;
        let mut diff = styra_diff::CheckoutDiff::read(styra_diff::DiffTarget {
            worktree: std::env::temp_dir().join("styra-event-loop-no-such-checkout"),
            branch: None,
            base_branch: None,
            base: "4bf5c35d".into(),
            branch_point: true,
        });
        diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n".into());
        app.checkout_diff = Some(diff);
        assert!(app.diff_focused());
        let mut terminal = styra_ui::TestUi::new(100, 30).unwrap();
        terminal.script(
            keys.chars()
                .map(|key| Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char(key)))),
        );
        let preferences = std::env::temp_dir().join("styra-event-loop-no-preferences.toml");
        run(
            &mut terminal,
            &mut app,
            &Client::in_process(server),
            &mut Attachment::Detached,
            RunContext {
                standing_launch: &LaunchPolicy::default(),
                preferences_path: &preferences,
                config: &crate::config::Defaults,
            },
        )
    }

    /// The diff owns the keyboard, and its key handler never falls through
    /// to the rest of the round; what its keys ask for is still done on the
    /// key, not left queued behind keys that never come.
    #[test]
    fn what_a_diff_key_asks_for_is_done_on_that_key() {
        let quit = run_over_diff("q", Default::default());
        assert!(
            matches!(quit, Ok(RunOutcome::Quit)),
            "q quits from the diff"
        );

        let server = std::sync::Arc::new(Unreachable::default());
        assert!(matches!(
            run_over_diff("Eq", server.clone()),
            Ok(RunOutcome::Quit)
        ));
        // The report is asked for off the loop's thread.
        let asked = || {
            server.asked.lock().unwrap().iter().any(|request| {
                matches!(
                    request,
                    styra_protocol::Request::GenerateEditReport { id, files, .. }
                        if id == "s1" && files.len() == 1
                )
            })
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !asked() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(asked(), "E asked the server for an edit report");
    }
}
