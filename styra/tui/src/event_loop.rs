use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::activity::Status;
use crate::app::{App, Focus, LaunchPolicy, Request};
use crate::audio::AudioInput;
use crate::config::Configuration;
use crate::input;
use crate::keybindings as keys;
use crate::launch::{self, LaunchScope};
use crate::launcher;
use crate::picker;
use crate::preferences;
use crate::presentation;
use crate::session::{self, Attachment};
use styra_protocol::{
    InteractionSummary, LogEntry, TemplateSummary, WorkspaceLaunchChange, WorkspaceSummary,
};
use styra_server::Client;
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
    Reset,
    NewSession,
}

const INTERACTIONS_REFRESH: Duration = Duration::from_millis(250);

/// How long one round of the loop waits for a key before coming back to poll
/// the server for updates, interactions and quota.
const KEY_POLL: Duration = Duration::from_millis(100);

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
                            app.show_action_message(message.clone());
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
        Err(error) => {
            app.show_action_message(format!("could not save interaction tags: {error:#}"))
        }
    }
}

pub struct RunContext<'a> {
    pub standing_launch: &'a LaunchPolicy,
    pub preferences_path: &'a Path,
    pub config: &'a dyn Configuration,
}

/// Hand one host path to the configured opener, reporting the outcome the way
/// [`terminal::open_shell`](crate::terminal::open_shell) does.
///
/// The one place a file is opened, so the Files view, a typed `files` answer,
/// and a selected Markdown link obey the same configuration and say the same
/// thing afterwards.
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
        || keys::GLOBAL_STOP.matches(key)
        || keys::GLOBAL_NEXT_LIVE.matches(key)
        || keys::GLOBAL_NEW_SESSION.matches(key)
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
    match (outcome, live) {
        (RunOutcome::Reset, Attachment::Attached { .. }) => true,
        _ => false,
    }
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
    let mut audio = AudioInput::new();
    let mut pending_fold = false;
    let mut interactions_refreshed = Instant::now();
    let mut quota_refreshed = Instant::now();
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
    loop {
        let workspace_id = app.workspace.id.clone().unwrap_or_default();
        dirty |= app.notices.expire();
        dirty |= launch_effects.apply_ready(app, &workspace_id);
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
            if let Ok(interactions) = client.list_interactions() {
                dirty |= app.interactions.refresh(interactions);
            }
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

        // A cursor that has come to rest loads the interaction under it. Until
        // then the navigator says that row is loading and the screen below is
        // still the interaction it was.
        if app.interactions.due(&app.session_id).is_some() {
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
                            app.show_action_message(format!(
                                "mounts not applied, still running on the old ones: {error:#}"
                            ));
                        }
                        Err(session::MountRestartError::Failed(error)) => {
                            app.push_log(LogEntry::error(format!(
                                "could not restart with the new mounts: {error:#}"
                            )));
                            app.show_action_message(format!(
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

        let waited = if audio.is_recording() {
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

        // `?` describes whichever window is showing, so every key-driven one
        // answers it — the launcher, the template chooser and the modal
        // overlays included. The prompts that take typed text are excluded:
        // there a `?` is a character of what is being typed.
        if keys::HELP.matches(key)
            && app.insert.is_none()
            && app.git_repository_prompt.is_none()
            && app.launch.prompt.is_none()
            && !app.search.typing()
            && !app
                .tag_picker
                .as_ref()
                .is_some_and(|picker| picker.new_tag.is_some())
            && (app.focus == Focus::List || app.launcher.is_some() || app.template_picker.is_some())
        {
            app.help.open();
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
        if app.branch_prompt.is_some() {
            input::handle_branch_prompt_key(app, client, key);
            // Confirming has already branched; open the result on this key
            // rather than leaving the switch queued behind the next one.
            if let Some(Request::OpenSession(id)) = app.take_open_session_request() {
                return Ok(RunOutcome::OpenSession(id));
            }
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
                k if keys::TAGS_DELETE_WORD.matches(k) => picker.delete_query_word(),
                k if keys::TAGS_TOGGLE.matches(k) => picker.toggle(),
                k if keys::TAGS_NEW.matches(k) => picker.start_new(),
                // Esc widens the list back out before it closes the picker.
                k if keys::TAGS_CANCEL.matches(k) => {
                    if picker.is_filtering() {
                        picker.clear_query();
                    } else {
                        app.tag_picker = None;
                    }
                }
                k if keys::TAGS_SAVE.matches(k) => {
                    let tags = picker.selected.clone();
                    let id = app.session_id.clone();
                    save_tags(app, client, id, tags);
                }
                _ => match key.code {
                    KeyCode::Backspace => picker.type_query(None),
                    KeyCode::Char(character)
                        if !character.is_control()
                            && !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        picker.type_query(Some(character));
                    }
                    _ => {}
                },
            }
            continue;
        }
        // The event list's `/` search is modal while it is being typed: every
        // printable key is part of the term, including the ones bound to
        // commands on the list it is marking.
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
            // arrows still walk what it leaves standing.
            if app.interactions.typing_filter() {
                let workspace_id = app.workspace.id.clone();
                match key.code {
                    KeyCode::Esc => app.interactions.clear_filter(),
                    KeyCode::Enter => app.interactions.finish_filter(),
                    KeyCode::Down => app
                        .interactions
                        .cursor_next(&session_id, workspace_id.as_deref()),
                    KeyCode::Up => app
                        .interactions
                        .cursor_previous(&session_id, workspace_id.as_deref()),
                    KeyCode::Backspace => {
                        app.interactions
                            .type_filter(None, &session_id, workspace_id.as_deref())
                    }
                    KeyCode::Char(character)
                        if !character.is_control()
                            && !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        app.interactions.type_filter(
                            Some(character),
                            &session_id,
                            workspace_id.as_deref(),
                        )
                    }
                    _ => {}
                }
                continue;
            }
            match key {
                k if keys::INTERACTIONS_FILTER.matches(k) => {
                    app.interactions.start_filter();
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
                        app.show_action_message("no other interaction is actively working");
                    }
                    continue;
                }
                k if keys::INTERACTIONS_PREV_WORKING.matches(k) => {
                    if app
                        .interactions
                        .cursor_to_previous_active(&session_id, app.workspace.id.as_deref())
                        .is_none()
                    {
                        app.show_action_message("no other interaction is actively working");
                    }
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
                _ => {}
            }
            // Every other key acts on the row under the cursor, so a move
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
                k if keys::INTERACTIONS_COMPLETE.matches(k) => {
                    let Some(interaction) = app.interactions.current(&app.session_id).cloned()
                    else {
                        continue;
                    };
                    // The cursor settle above already made this row the one
                    // on screen, so completing it is the same action — and the
                    // same local status update — as the `C` bound directly on
                    // the Events view.
                    if !session::complete_interaction(app, client, live) {
                        continue;
                    }
                    // Completion is a Session property the server owns, so the
                    // row is still listed and the server is what says so: take
                    // its word for the new state rather than writing one here.
                    if let Ok(interactions) = client.list_interactions() {
                        app.interactions.refresh(interactions);
                    }
                    if app.interactions.show_completed {
                        continue;
                    }
                    let workspace_id = app.workspace.id.clone();
                    let Some(next) = app
                        .interactions
                        .select_past_hidden(&interaction.id, workspace_id.as_deref())
                    else {
                        app.interactions.close();
                        return Ok(RunOutcome::Reset);
                    };
                    make_interaction_current(app, live, client, standing_launch, next);
                    continue;
                }
                k if keys::INTERACTIONS_DELETE.matches(k) => {
                    let Some(interaction) = app.interactions.current(&app.session_id).cloned()
                    else {
                        continue;
                    };
                    if interaction.activity.accepting() {
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
                        Err(error) => {
                            app.show_action_message(format!("could not list tags: {error:#}"))
                        }
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

        // A picker that the operator backs out of leaves the session as it was,
        // so those arms fall through to the next frame rather than returning.
        match app.take_request() {
            None => {}
            Some(Request::Quit) => return Ok(RunOutcome::Quit),
            Some(Request::Workspace) => {
                let mut workspaces = client.list_workspaces()?;
                let Some(choice) =
                    picker::run_workspace_picker(terminal, client, &mut workspaces, None)?
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
                // Starting fresh from this list enters the Workspace that was
                // just chosen with no Session loaded, which is the same place
                // entering a Workspace that has no history at all lands.
                if let Some(choice) = picker::run_session_picker(
                    terminal,
                    client,
                    &crate::workspace::display_name(&workspace),
                    &mut sessions,
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
            }
            Some(Request::OpenSession(id)) => return Ok(RunOutcome::OpenSession(id)),
            Some(Request::Sessions) => {
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
            // The interaction is loaded outright rather than cursored, and the
            // navigator is left as it was: open or closed. Newly idle work
            // takes priority; otherwise this walks every live interaction.
            Some(Request::NextLiveInteraction) => {
                // The client's snapshot is up to a refresh old, and an
                // interaction that has since stopped is not one to step onto.
                if let Ok(interactions) = client.list_interactions() {
                    app.interactions.refresh(interactions);
                    interactions_refreshed = Instant::now();
                }
                let next = app
                    .interactions
                    .next_attention_or_live(&app.session_id, app.workspace.id.as_deref());
                let Some(next) = next else {
                    app.show_action_message("no other interaction is running");
                    continue;
                };
                make_interaction_current(app, live, client, standing_launch, next);
            }
            Some(Request::NextWorkingInteraction) => {
                if let Ok(interactions) = client.list_interactions() {
                    app.interactions.refresh(interactions);
                    interactions_refreshed = Instant::now();
                }
                let Some(next) = app
                    .interactions
                    .next_active(&app.session_id, app.workspace.id.as_deref())
                else {
                    app.show_action_message("no other interaction is actively working");
                    continue;
                };
                make_interaction_current(app, live, client, standing_launch, next);
            }
            Some(Request::NewSession) => return Ok(RunOutcome::NewSession),
            // Naming the branch asks the model for a topic and the checkout
            // copies the repository out, which together take long enough to
            // look like a hang. `W` and `Ctrl-Enter` differ only in what the
            // checkout is made for: a Session already running, which has to be
            // restarted into it, or one that the first prompt launches there.
            Some(Request::CreateWorktree { first_prompt }) => {
                if let Some(message) = first_prompt {
                    // The launch branches and checks out before it sends, and
                    // reports its own failure — restoring the message box.
                    blocked_on(terminal, app, CREATING_WORKTREE, |app| {
                        session::submit_message(app, client, &workspace_id, live, message, true)
                    })?;
                    continue;
                }
                let session_id = app.session_id.clone();
                let created = blocked_on(terminal, app, CREATING_WORKTREE, |_| {
                    client.create_session_worktree(&session_id)
                })?;
                if let Err(error) = created {
                    app.show_action_message(format!(
                        "could not create a linked workspace: {error}"
                    ));
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
                    Err(error) => app.show_action_message(format!(
                        "created the linked workspace, but could not restart in it ({error}); it will be used when this Session next launches"
                    )),
                }
            }
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
            Some(Request::SetAutoCommit(enabled)) => {
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
            Some(Request::OpenUrl(url)) => open_url(app, config, &url),
            Some(Request::OpenDirectory) => {
                let Some(directory) = app.workspace.working_directory_or_current() else {
                    app.show_action_message("no directory to open a terminal in");
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
    fn global_actions_pass_through_the_navigator() {
        let press = |character| crossterm::event::KeyEvent::from(KeyCode::Char(character));
        assert!(interaction_navigator_passthrough(press('S')));
        assert!(interaction_navigator_passthrough(press('i')));
        assert!(interaction_navigator_passthrough(
            crossterm::event::KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL,)
        ));
        assert!(!interaction_navigator_passthrough(press('l')));
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
