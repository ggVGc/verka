//! Application state: session status, which view is up, the raw/log/files
//! panels, and the pending request for the event loop.
//!
//! Anything with enough state of its own to be worth a module has one, and is
//! carried here as a field rather than spread across this struct: the event
//! list is a [`Timeline`], the sandbox policy a [`Launch`], and likewise
//! [`Launcher`], [`Composer`] and [`Notes`]. What stays here is what belongs to
//! no single one of them, and the few methods that have to join two — a
//! selection move also resets the preview scroll, an edit to the launch policy
//! is refused while an interaction is running.
//!
//! This module is pure state and transitions — no terminal, no threads, no IO —
//! so the whole interaction model is unit-testable. [`crate::ui`] renders it and
//! `main` feeds it input and session updates.

use std::cell::Cell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use styra_server::agent::SandboxLayout;
use styra_server::agent::{Provider, Selection};
use styra_server::event::PresentationMode;
use styra_server::event::{AgentEvent, DetailBlock, TokenUsage};
use styra_server::{Answer, AnswerValue, Contract, FileLocation, QueuedMessage};
use styra_server::{InteractionEnd, LogEntry, RawLine};

use crate::composer::Composer;
use crate::ingest;
use crate::launch::{self, Launch};
use crate::launcher::Launcher;
use crate::notes::Notes;
use crate::timeline::{Entry, Step, Timeline};

/// Which region receives keys, like vim's normal/insert split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Navigate and fold the event list.
    List,
    /// Type into the message box.
    Input,
}

/// What the main region shows: the decoded event list, the raw wire stream,
/// the diagnostic log, the rendered transcript, the session's Driva policy,
/// or the selected entry's full-screen preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Events,
    Raw,
    Log,
    Transcript,
    Driva,
    Files,
    /// The last turn's typed answer, rendered as the shape it was asked for.
    Answer,
    Preview,
}

/// Which entry the preview panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewTarget {
    /// The entry the list selection is on.
    #[default]
    Selection,
    /// The newest shell command and its result, regardless of where the
    /// selection currently is — so the preview keeps showing what the agent
    /// is running while the operator reads elsewhere in the list.
    Command,
}

/// The session's lifecycle as the operator sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No agent process has been launched yet; it starts on the operator's
    /// first submitted message (see `App::pending`).
    Pending,
    /// The agent is working.
    Running,
    /// A turn completed; the agent is idle, awaiting input.
    Idle,
    /// The agent is idle, but a Claude background task is still running.
    Background,
    /// The operator stopped the session; the process may still be winding down.
    Stopped,
    /// The agent process ended.
    Ended {
        exit_code: Option<i32>,
        error: Option<String>,
    },
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Pending => "not started".into(),
            Status::Running => "running".into(),
            Status::Idle => "idle".into(),
            Status::Background => "idle · background work running".into(),
            Status::Stopped => "stopped".into(),
            Status::Ended { error: Some(_), .. } => "failed".into(),
            Status::Ended {
                exit_code: Some(code),
                ..
            } => format!("ended ({code})"),
            Status::Ended { .. } => "ended".into(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Status::Pending | Status::Running | Status::Idle | Status::Background | Status::Stopped
        )
    }
}

impl From<styra_server::InteractionActivity> for Status {
    fn from(activity: styra_server::InteractionActivity) -> Self {
        match activity {
            styra_server::InteractionActivity::Pending => Self::Idle,
            styra_server::InteractionActivity::Running => Self::Running,
            styra_server::InteractionActivity::Background => Self::Background,
        }
    }
}

/// What a launch is asked for beyond the agent selection: the sandbox policy
/// inputs the server resolves into a concrete Driva request.
///
/// Defined on the wire rather than here ([`styra_server::LaunchPolicy`]),
/// because both layers of it are the server's to resolve: the Workspace's
/// standing policy and this interaction's own are merged there, once, for every
/// launch path. The client holds the two apart (in [`Launch`]) only so the driva
/// view can say where each grant comes from and edit one without the other.
///
/// Re-exported because the modules that persist and send a policy
/// ([`crate::preferences`], [`crate::session`]) want the type without wanting
/// the state machine around it.
pub use styra_server::LaunchPolicy;

/// How long the session has been in its current state, and how long since
/// anything last arrived from the agent.
///
/// A turn can spend minutes inside one tool call, during which nothing on the
/// screen changes; without these two numbers a working session and a hung one
/// look identical.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    /// Time since the status last changed — for a running turn, how long it
    /// has been running; for an idle one, how long it has been waiting.
    pub in_status: Duration,
    /// Time since the last event was received, or `None` if none has been.
    pub since_event: Option<Duration>,
    /// How many events have arrived from the agent. The spinner steps with
    /// this rather than with the clock, so its motion means "something came
    /// back" instead of "the frame was redrawn".
    pub events: usize,
}

/// A short-lived notice about something Styra did on the operator's behalf.
pub struct ActionMessage {
    pub text: String,
    shown_at: Instant,
}

const ACTION_MESSAGE_LIFETIME: Duration = Duration::from_secs(5);

/// How many recently selected models the picker remembers to order its model
/// column by.
const RECENT_MODELS: usize = 16;

/// The agent, model, and reasoning effort a session is on, as the status line
/// names them.
///
/// `model` and `effort` are what the agent *reported* once it started, falling
/// back to what the launch asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchLabel {
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Whether `model` came from the agent itself rather than from the launch
    /// request, so the display can distinguish what *is* running from what was
    /// asked for.
    pub model_reported: bool,
    /// The same for `effort`. It can differ from `model_reported`: Claude Code
    /// names the model it resolved but never an effort, so a Claude session's
    /// effort is only ever what the launch asked for.
    pub effort_reported: bool,
}

/// How far a preview panel is scrolled, and the last maximum offset its
/// renderer calculated.
///
/// The limit has to come back from rendering because wrapping depends on the
/// terminal width, which only the renderer knows. Holding onto it stops
/// repeated PageDown presses at the bottom from accumulating an invisible
/// offset that PageUp would later have to unwind.
#[derive(Default)]
pub struct Scroll {
    /// Lines scrolled down from the top. Only ever shown through
    /// [`Scroll::clamped`], so it may sit past what currently fits.
    pub offset: u16,
    limit: Cell<u16>,
}

/// Lines moved by one PageUp/PageDown press.
const SCROLL_PAGE: u16 = 10;

impl Scroll {
    /// The offset to render at: what was asked for, held to what actually fits.
    pub fn clamped(&self) -> u16 {
        self.offset.min(self.limit.get())
    }

    /// Record the furthest the renderer can actually scroll at this width.
    pub fn note_limit(&self, limit: u16) {
        self.limit.set(limit);
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }

    pub fn page_down(&mut self) {
        self.offset = self
            .clamped()
            .saturating_add(SCROLL_PAGE)
            .min(self.limit.get());
    }

    pub fn page_up(&mut self) {
        self.offset = self.clamped().saturating_sub(SCROLL_PAGE);
    }

    pub fn line_down(&mut self) {
        self.offset = self.clamped().saturating_add(1).min(self.limit.get());
    }

    pub fn line_up(&mut self) {
        self.offset = self.clamped().saturating_sub(1);
    }
}

/// The complete UI state.
pub struct App {
    /// The event list and the operator's place in it; see [`Timeline`].
    pub timeline: Timeline,
    pub focus: Focus,
    pub view: View,
    /// Whether the full-screen keyboard shortcut reference is open.
    pub show_keybinds: bool,
    /// Lines the keybind reference is scrolled down by. It is longer than a
    /// short terminal, so without this the sections at the end are simply
    /// unreachable rather than merely below the fold.
    pub keybinds_scroll: u16,
    /// The message being typed and the ones already sent; see [`Composer`].
    pub composer: Composer,
    /// Messages submitted while the current turn is running. They remain
    /// visible until sent or the interaction is stopped.
    queued_messages: VecDeque<QueuedMessage>,
    pub status: Status,
    /// Recent actions Styra performed without a direct operator command.
    /// Each is displayed for five seconds in the message panel.
    pub action_messages: VecDeque<ActionMessage>,
    /// When true, a side panel shows the full expanded content of the
    /// selected entry, independent of whether it is folded in the list.
    pub show_preview: bool,
    /// How far the selected entry's preview is scrolled.
    pub preview: Scroll,
    pub preview_mode: PresentationMode,
    /// Whether the preview follows the selection or the newest command.
    pub preview_target: PreviewTarget,
    /// What the next session launches with: agent, model, reasoning effort.
    /// This is the choice for the current workspace, edited through [`Launcher`]
    /// while nothing is running. The terminal client only persists it as the
    /// standing default when the operator explicitly asks.
    pub selection: Selection,
    /// The open launch picker, while the operator is choosing.
    pub launcher: Option<Launcher>,
    /// Models the operator has confirmed in the launch picker, most recent
    /// first. Loaded from the saved preferences when the loop starts and
    /// written back as the picker is used, so the ordering of the model
    /// column outlives the client.
    pub recent_models: Vec<String>,
    /// The model and reasoning effort the agent itself reported when it started
    /// the session, which is what is actually running — a launch pins a model,
    /// but only the agent can confirm what it resolved to. `None` until the
    /// agent's session-start line arrives (and for agents that report neither).
    pub reported_model: Option<(String, Option<String>)>,
    /// Durable Workspace containing the current Session, when known.
    pub workspace_id: Option<String>,
    /// Operator-facing name of the active Workspace. This is resolved from
    /// Workspace metadata (with the host directory name as its fallback) by
    /// the client, since Sessions only carry the durable Workspace id.
    pub workspace_name: Option<String>,
    pub session_id: String,
    /// Optional operator-facing name of the current durable Session.
    pub session_name: Option<String>,
    /// The host directory backing the agent's sandboxed workspace, when
    /// known (a live session; a replayed journal has no live workspace).
    /// Lets the preview panel read a changed file's current content.
    pub workspace_root: Option<PathBuf>,
    /// The current directory within `workspace_root` used by a live interaction.
    pub working_directory: Option<PathBuf>,
    /// The sandbox policy: both of its layers, the sandbox they resolve to, and
    /// which layer the driva view's keys are editing. See [`Launch`].
    pub launch: Launch,
    pub latest_usage: Option<TokenUsage>,
    /// When the status last changed, and the status that was current then.
    /// Status is written from several places (an applied update, a queued send,
    /// a key handler), so the moment it changed is noticed in one place —
    /// [`App::note_progress`] — rather than at each assignment.
    status_since: Instant,
    noted_status: Status,
    /// When the last event arrived from the agent. `None` until one has.
    last_event_at: Option<Instant>,
    /// How many events have arrived from the agent, for the spinner's phase.
    events_received: usize,
    background_work: bool,
    /// Set once the provider has reported its background-task set. From then
    /// on that count is the only thing that moves `background_work`; the
    /// tool-call heuristics are a fallback for providers that stay silent.
    background_count_known: bool,
    /// The verbatim wire interaction, in occurrence order.
    pub raw: Vec<RawLine>,
    /// Which wire line the raw view has selected.
    pub raw_selected: usize,
    /// When true, `raw_selected` tracks the newest line as it arrives.
    pub raw_follow: bool,
    /// How far the selected wire line's pretty-printed preview is scrolled.
    pub raw_preview: Scroll,
    /// Diagnostic log entries, in occurrence order.
    pub log: Vec<LogEntry>,
    /// Lines scrolled back from the bottom of the log view; 0 tracks the tail.
    pub log_scroll_back: u16,
    /// Lines scrolled down from the top of the rendered transcript view; 0
    /// shows its start. Unlike the raw/log views, the transcript reads as a
    /// document from the beginning rather than anchoring to the tail.
    pub transcript_scroll: u16,
    /// Selected file in the Files view and whether it aggregates the session.
    pub file_selected: usize,
    pub file_show_all: bool,
    /// The shape the next message asks its reply to come back in, or `None` for
    /// an ordinary turn. Set by the operator before sending and cleared once
    /// the message it applies to has gone, so a contract never silently
    /// outlives the question it was chosen for.
    pub contract: Option<Contract>,
    /// The typed answer last fetched for this session, and the selection
    /// within it. `None` before anything has been asked for; an `Answer` whose
    /// value is absent is a reply that missed its contract, which is shown
    /// rather than discarded.
    pub answer: Option<Answer>,
    /// Why the last answer could not be fetched at all — no typed turn, no
    /// reply yet, no session. Distinct from an answer that arrived and failed
    /// to parse, which is an `Answer`.
    pub answer_error: Option<String>,
    pub answer_selected: usize,
    /// Set when the operator asks for something only the event loop can do;
    /// it takes the request and acts on it.
    pub request: Option<Request>,
    /// This Session's and Workspace's notes; see [`crate::notes`].
    pub notes: Notes,
}

/// Something the operator asked for that [`App`] cannot carry out itself,
/// because it means leaving this screen or this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Quit,
    /// Choose a Workspace, then one of its Sessions.
    Workspace,
    /// Choose another Session in the current Workspace. This only changes the
    /// client view; it stops neither Session.
    Sessions,
    /// Switch the client view directly to an already-known Session id,
    /// skipping the picker — e.g. right after branching one, to look at the
    /// result rather than having to find it again in the list.
    OpenSession(String),
    /// List the server's live interactions and, if the operator picks one,
    /// attach to it. The current interaction is left running, not stopped:
    /// attaching only changes what this client views.
    Interactions,
    /// Stop the current interaction and return to the blank start screen.
    Reset,
    /// Return to the blank start screen without stopping the current interaction.
    NewSession,
    /// Open the selected entry in the Files view in the configured editor.
    EditFile,
    /// Choose which Driva templates the next interaction launches with. The
    /// list of them lives on the server, so the event loop fetches it and runs
    /// the picker.
    Templates,
    /// Send the Workspace's standing policy, as edited here, to the server that
    /// owns it — so every launch here, from this client or any other, starts
    /// from it. Raised by each edit to that layer; `announce` is set only for
    /// the explicit `W`, since an automatic store has nothing to report that
    /// the edit's own message did not already say.
    StoreWorkspaceLaunch {
        announce: bool,
    },
    /// Move this interaction's own settings up into the Workspace's standing
    /// policy: the merge is stored as the Workspace's and the overlay emptied,
    /// so what the next launch runs under does not change.
    PromoteLaunchToWorkspace,
    /// Tell the server the live interaction has been switched onto
    /// [`App::selection`], so the change lands now and outlives this client.
    ApplySelection,
    /// Fetch the last turn's typed answer from the server, which parses it.
    /// `contract` names a shape to read the reply under instead of the one the
    /// turn was sent with, which is how a mis-shaped answer is recovered
    /// without asking the agent again.
    Answer {
        contract: Option<Contract>,
    },
}

impl App {
    pub fn new(selection: Selection, session_id: impl Into<String>) -> Self {
        Self {
            timeline: Timeline::default(),
            focus: Focus::List,
            view: View::Events,
            show_keybinds: false,
            keybinds_scroll: 0,
            composer: Composer::default(),
            queued_messages: VecDeque::new(),
            status: Status::Running,
            action_messages: VecDeque::new(),
            show_preview: false,
            preview: Scroll::default(),
            preview_mode: PresentationMode::Pretty,
            preview_target: PreviewTarget::Selection,
            selection,
            launcher: None,
            recent_models: Vec::new(),
            reported_model: None,
            workspace_id: None,
            workspace_name: None,
            session_id: session_id.into(),
            session_name: None,
            workspace_root: None,
            working_directory: None,
            launch: Launch::default(),
            latest_usage: None,
            status_since: Instant::now(),
            noted_status: Status::Running,
            last_event_at: None,
            events_received: 0,
            background_work: false,
            background_count_known: false,
            raw: Vec::new(),
            raw_selected: 0,
            raw_follow: true,
            raw_preview: Scroll::default(),
            log: Vec::new(),
            log_scroll_back: 0,
            transcript_scroll: 0,
            file_selected: 0,
            file_show_all: false,
            contract: None,
            answer: None,
            answer_error: None,
            answer_selected: 0,
            request: None,
            notes: Notes::default(),
        }
    }

    /// A fresh App with no agent process launched yet: no journal or session
    /// id exists until the operator submits a first message, at which point
    /// the event loop spawns the session and fills those in.
    /// Opens in list focus, not input focus, so landing on this screen never
    /// drops the operator straight into typing; `i` still reaches it.
    ///
    /// `selection` is what the session will launch with; it is also the only
    /// state in this screen the operator can still change (see
    /// [`App::open_launcher`]), since nothing has been launched to be stuck
    /// with yet.
    pub fn pending(selection: Selection) -> Self {
        let mut app = Self::new(selection, String::new());
        app.status = Status::Pending;
        app
    }

    /// Whether the picker is reachable. Before launch all providers are
    /// configurable; idle Codex and Claude threads also accept a model change
    /// before their next turn (Codex additionally accepts an effort change).
    pub fn can_configure_launch(&self) -> bool {
        self.status == Status::Pending
            || (self.status == Status::Idle
                && matches!(self.selection.provider, Provider::Codex | Provider::Claude))
    }

    /// Open the launch picker on the current selection, if anything can still
    /// be chosen.
    pub fn open_launcher(&mut self) {
        if self.can_configure_launch() {
            self.launcher = Some(Launcher::from_selection(
                &self.selection,
                &self.recent_models,
            ));
        }
    }

    /// Adopt what the picker describes, and close it.
    ///
    /// Before launch nothing is sent: the operator's first message still starts
    /// the agent. On a live session the change is asked of the server right
    /// away ([`Request::ApplySelection`]) rather than riding on the next
    /// message, so the model the status line names is the model that is loaded.
    pub fn confirm_launcher(&mut self) {
        if let Some(launcher) = self.launcher.take() {
            let selection = launcher.selection();
            if self.status != Status::Pending && selection.provider != self.selection.provider {
                self.show_action_message("changing agent requires a new session");
            } else {
                let mut selection = selection;
                if self.status != Status::Pending
                    && selection.provider == Provider::Claude
                    && selection.effort != self.selection.effort
                {
                    selection.effort = self.selection.effort;
                    self.show_action_message(
                        "Claude Code can change model between turns; effort remains session-wide",
                    );
                }
                let live = self.status != Status::Pending && selection != self.selection;
                self.note_recent_model(&selection.model);
                self.set_selection(selection);
                self.reported_model = None;
                if live {
                    self.ask(Request::ApplySelection);
                }
            }
        }
    }

    /// Record a model as the most recently selected one, so the picker lists
    /// it first the next time it opens. The list is capped: past a certain
    /// depth it stops being a shortlist and starts being the catalog again.
    fn note_recent_model(&mut self, model: &str) {
        self.recent_models.retain(|recent| recent != model);
        self.recent_models.insert(0, model.to_owned());
        self.recent_models.truncate(RECENT_MODELS);
    }

    /// Close the picker, leaving the selection as it was.
    pub fn cancel_launcher(&mut self) {
        self.launcher = None;
    }

    /// What the status line names: the agent, and the model and reasoning effort
    /// in use.
    ///
    /// The agent's own report wins where it made one, since that is what is
    /// actually running; otherwise the requested selection answers for it.
    pub fn launch_label(&self) -> LaunchLabel {
        let agent = self.selection.provider.as_str().to_owned();
        let requested_model = Some(self.selection.model.clone());
        let requested_effort = Some(self.selection.effort.as_str().to_owned());
        match &self.reported_model {
            Some((model, effort)) => LaunchLabel {
                agent,
                model: Some(model.clone()),
                effort_reported: effort.is_some(),
                effort: effort.clone().or(requested_effort),
                model_reported: true,
            },
            None => LaunchLabel {
                agent,
                model: requested_model,
                effort: requested_effort,
                model_reported: false,
                effort_reported: false,
            },
        }
    }

    /// Record the launch choice so the status line names what an `Enter` would start.
    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = selection;
    }

    /// Tell the operator about an action Styra took on their behalf.
    pub fn show_action_message(&mut self, message: impl Into<String>) {
        self.action_messages.push_back(ActionMessage {
            text: message.into(),
            shown_at: Instant::now(),
        });
    }

    /// Remove notices whose independent five-second display window has elapsed.
    /// Notice a status change made since the last frame, so [`Self::progress`]
    /// can report how long the session has been in its current state. Called
    /// once per event-loop iteration, just before rendering.
    pub fn note_progress(&mut self) {
        if self.noted_status != self.status {
            self.noted_status = self.status.clone();
            self.status_since = Instant::now();
        }
    }

    pub fn progress(&self) -> Progress {
        Progress {
            in_status: self.status_since.elapsed(),
            since_event: self.last_event_at.map(|at| at.elapsed()),
            events: self.events_received,
        }
    }

    pub fn expire_action_messages(&mut self) {
        let now = Instant::now();
        while self
            .action_messages
            .front()
            .is_some_and(|message| now.duration_since(message.shown_at) >= ACTION_MESSAGE_LIFETIME)
        {
            self.action_messages.pop_front();
        }
    }

    /// Replace the message box's contents outright, used to restore a message
    /// that failed to launch so it isn't lost.
    pub fn set_input(&mut self, text: String) {
        self.composer.set(text);
    }

    /// Append a diagnostic log entry, keeping the tail in view unless the
    /// operator has scrolled up (mirrors [`push_raw`](Self::push_raw)).
    pub fn push_log(&mut self, entry: LogEntry) {
        self.log.push(entry);
        if self.log_scroll_back > 0 {
            self.log_scroll_back = self.log_scroll_back.saturating_add(1);
        }
    }

    pub fn log_scroll_up(&mut self) {
        let max = self.log.len().saturating_sub(1) as u16;
        self.log_scroll_back = self.log_scroll_back.saturating_add(1).min(max);
    }

    pub fn log_scroll_down(&mut self) {
        self.log_scroll_back = self.log_scroll_back.saturating_sub(1);
    }

    pub fn log_to_top(&mut self) {
        self.log_scroll_back = self.log.len().saturating_sub(1) as u16;
    }

    pub fn log_to_bottom(&mut self) {
        self.log_scroll_back = 0;
    }

    /// Append a verbatim wire line. When the operator has selected a line
    /// explicitly, the view stays pinned to it; otherwise the selection
    /// tracks the new tail.
    pub fn push_raw(&mut self, line: RawLine) {
        self.raw.push(line);
        if self.raw_follow {
            self.raw_selected = self.raw.len() - 1;
            self.raw_preview.reset();
        }
    }

    /// Toggle the raw wire view on, or back to the event list. Entering it
    /// focuses the wire line behind the currently selected entry (or the
    /// tail, while the list is following it, or if no line is known for the
    /// selection), so switching views keeps the same point in the session
    /// in view rather than resetting to wherever the raw view was last left.
    pub fn toggle_raw(&mut self) {
        if self.view == View::Raw {
            self.view = View::Events;
            return;
        }
        self.view = View::Raw;
        self.raw_preview.reset();
        if !self.timeline.follow {
            if let Some(idx) = self
                .timeline
                .selected_entry()
                .and_then(|entry| entry.raw_index)
            {
                self.raw_selected = idx.min(self.raw.len().saturating_sub(1));
                self.raw_follow = false;
                return;
            }
        }
        self.raw_selected = self.raw.len().saturating_sub(1);
        self.raw_follow = true;
    }

    /// Show `view`, or return to the event list if it is already showing, so
    /// one key both opens and closes each alternate view. [`Self::toggle_raw`]
    /// stays separate because entering the raw view also has to line its
    /// selection up with the event list's.
    pub fn toggle_view(&mut self, view: View) {
        self.view = if self.view == view {
            View::Events
        } else {
            view
        };
    }

    /// Move the raw view's selection to the next wire line.
    pub fn raw_select_next(&mut self) {
        if self.raw_selected + 1 < self.raw.len() {
            self.raw_selected += 1;
            self.raw_preview.reset();
        }
        // Re-enable follow only when the selection reaches the tail.
        self.raw_follow = !self.raw.is_empty() && self.raw_selected + 1 >= self.raw.len();
    }

    /// Move the raw view's selection to the previous wire line.
    pub fn raw_select_prev(&mut self) {
        if self.raw_selected > 0 {
            self.raw_selected -= 1;
            self.raw_preview.reset();
        }
        // Moving off the tail pins the view.
        self.raw_follow = false;
    }

    pub fn raw_select_first(&mut self) {
        if self.raw.is_empty() {
            return;
        }
        self.raw_selected = 0;
        self.raw_preview.reset();
        self.raw_follow = false;
    }

    pub fn raw_select_last(&mut self) {
        if self.raw.is_empty() {
            return;
        }
        self.raw_selected = self.raw.len() - 1;
        self.raw_preview.reset();
        self.raw_follow = true;
    }

    /// Scroll the transcript view forward (towards its end).
    pub fn transcript_scroll_down(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(1);
    }

    /// Scroll the transcript view backward (towards its start).
    pub fn transcript_scroll_up(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(1);
    }

    pub fn transcript_to_top(&mut self) {
        self.transcript_scroll = 0;
    }

    /// Jump past the transcript's true end; rendering clamps this back to
    /// the last page, so the exact rendered line count need not be known here.
    pub fn transcript_to_bottom(&mut self) {
        self.transcript_scroll = u16::MAX;
    }

    /// True when the operator can still send messages.
    pub fn can_send(&self) -> bool {
        self.status.is_active()
    }

    pub fn queued_messages(&self) -> impl Iterator<Item = &QueuedMessage> {
        self.queued_messages.iter()
    }

    pub fn queued_message_count(&self) -> usize {
        self.queued_messages.len()
    }

    pub fn queue_message(&mut self, message: QueuedMessage) {
        self.queued_messages.push_back(message);
    }

    pub fn take_queued_message(&mut self) -> Option<QueuedMessage> {
        self.queued_messages.pop_front()
    }

    pub fn clear_queued_messages(&mut self) -> usize {
        let count = self.queued_messages.len();
        self.queued_messages.clear();
        count
    }

    // --- Ingesting session updates -----------------------------------------
    //
    // How an event changes the list is [`crate::ingest`]'s; these are the parts
    // of that which are about the session rather than the list, kept here
    // because the fields they move are this struct's own.

    /// Append a decoded event; see [`ingest::push_event`].
    pub fn push_event(&mut self, event: AgentEvent) {
        ingest::push_event(self, event);
    }

    /// Record that the session ended; see [`ingest::on_ended`].
    pub fn on_ended(&mut self, end: InteractionEnd) {
        ingest::on_ended(self, end);
    }

    /// Note that something arrived from the agent, whatever it was: the spinner
    /// steps with the count and the "nothing for a while" figure with the time.
    pub(crate) fn note_event_received(&mut self) {
        self.last_event_at = Some(Instant::now());
        self.events_received += 1;
    }

    /// Where a completed turn leaves the session: still working on something in
    /// the background, or genuinely waiting for the operator.
    pub(crate) fn idle_or_background(&self) -> Status {
        if self.background_work {
            Status::Background
        } else {
            Status::Idle
        }
    }

    /// The provider's own count of what it is running in the background. Once
    /// it has reported one, that count is the only thing that moves the flag.
    pub(crate) fn note_background_count(&mut self, running: usize) {
        self.background_count_known = true;
        self.background_work = running > 0;
        if !self.background_work && self.status == Status::Background {
            self.status = Status::Idle;
        }
    }

    /// A tool call that looks like it started background work, for providers
    /// that never report a count.
    pub(crate) fn note_background_started(&mut self) {
        self.background_work = true;
    }

    /// The same heuristic in reverse, and ignored once the provider has spoken
    /// for itself.
    pub(crate) fn note_background_finished(&mut self) {
        if !self.background_count_known {
            self.background_work = false;
            self.status = Status::Idle;
        }
    }

    /// Put the selection on the last entry, where following leaves it, and
    /// start its preview from the top.
    pub(crate) fn select_tail(&mut self) {
        self.timeline.select_tail();
        self.preview.reset();
    }

    // --- List navigation ----------------------------------------------------
    //
    // Where the selection lands is [`Timeline`]'s; the preview scroll it
    // invalidates is this struct's, so each of these is one of its moves plus
    // that. See [`Timeline::select_forward`] for what "moved" means.

    fn moved(&mut self, moved: bool) {
        if moved {
            self.preview.reset();
        }
    }

    /// Move to the next entry with an arrow (something beyond its bare
    /// summary), skipping over ones with nothing else to show. See
    /// [`Self::select_next_line`] to instead step one entry at a time.
    pub fn select_next(&mut self) {
        let moved = self.timeline.select_forward(Step::WithDetail);
        self.moved(moved);
    }

    /// Move to the previous entry with an arrow; see [`Self::select_next`].
    pub fn select_prev(&mut self) {
        let moved = self.timeline.select_backward(Step::WithDetail);
        self.moved(moved);
    }

    /// Move to the next visible entry regardless of whether it has anything
    /// beyond its summary — a finer-grained step than [`Self::select_next`],
    /// which skips entries with no arrow.
    pub fn select_next_line(&mut self) {
        let moved = self.timeline.select_forward(Step::Line);
        self.moved(moved);
    }

    /// Move to the previous visible entry; see [`Self::select_next_line`].
    pub fn select_prev_line(&mut self) {
        let moved = self.timeline.select_backward(Step::Line);
        self.moved(moved);
    }

    pub fn select_first(&mut self) {
        let moved = self.timeline.select_first();
        self.moved(moved);
    }

    pub fn select_last(&mut self) {
        let moved = self.timeline.select_last();
        self.moved(moved);
    }

    /// Toggle whether minor lifecycle events (thread/turn/usage) are shown.
    pub fn toggle_minor(&mut self) {
        let moved = self.timeline.toggle_minor();
        self.moved(moved);
    }

    /// Toggle whether the main event list shows only operator/agent messages.
    pub fn toggle_conversation_only(&mut self) {
        let moved = self.timeline.toggle_conversation_only();
        self.moved(moved);
    }

    /// Toggle the side panel that previews the selected entry's full content.
    pub fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
    }

    /// Open the combined interaction/files layout with its entry preview
    /// visible, or return to the ordinary event list when already open.
    pub fn toggle_files(&mut self) {
        if self.view == View::Files {
            self.view = View::Events;
        } else {
            self.view = View::Files;
            self.show_preview = true;
        }
    }

    /// Move the keybind reference by `delta` lines, never above its start.
    /// The lower bound is the renderer's, since only it knows the height it
    /// has to fill.
    pub fn scroll_keybinds(&mut self, delta: i16) {
        self.keybinds_scroll = self
            .keybinds_scroll
            .saturating_add_signed(delta)
            .min(u16::MAX);
    }

    // --- Typed turn answers ---------------------------------------------------

    /// Contracts in the order `Ctrl-T` walks them, taken from the server's own
    /// list so a contract added there is offered here without a second edit.
    /// The cycle ends back at an untyped turn, so the operator can always get
    /// out of one without leaving the message box.
    pub const CONTRACTS: [Contract; 4] = styra_server::contract::CONTRACTS;

    /// Step to the next return contract for the message being typed.
    pub fn cycle_contract(&mut self) {
        let next = match self.contract {
            None => Some(Self::CONTRACTS[0]),
            Some(current) => {
                let index = Self::CONTRACTS.iter().position(|c| *c == current);
                index
                    .and_then(|index| Self::CONTRACTS.get(index + 1))
                    .copied()
            }
        };
        self.contract = next;
    }

    /// Take the contract the message about to be sent asks for, clearing it.
    ///
    /// Taken rather than read: a contract belongs to the question it was chosen
    /// for, and leaving it set would silently type the next message too.
    pub fn take_contract(&mut self) -> Option<Contract> {
        self.contract.take()
    }

    /// Show the answer view, asking the event loop to fetch under the
    /// session's recorded contract; or leave it when already there.
    pub fn toggle_answer(&mut self) {
        if self.view == View::Answer {
            self.view = View::Events;
        } else {
            self.view = View::Answer;
            self.ask(Request::Answer { contract: None });
        }
    }

    /// Re-read the same reply under another shape.
    pub fn reread_answer(&mut self, contract: Contract) {
        self.ask(Request::Answer {
            contract: Some(contract),
        });
    }

    /// Record a fetched answer, or why there was none to fetch.
    pub fn set_answer(&mut self, answer: Result<Answer, String>) {
        self.answer_selected = 0;
        match answer {
            Ok(answer) => {
                self.answer = Some(answer);
                self.answer_error = None;
            }
            Err(error) => {
                self.answer = None;
                self.answer_error = Some(error);
            }
        }
    }

    /// How many selectable rows the current answer has; 0 for shapes that are
    /// read rather than navigated.
    pub fn answer_rows(&self) -> usize {
        match self
            .answer
            .as_ref()
            .and_then(|answer| answer.value.as_ref())
        {
            Some(AnswerValue::Lines(lines)) => lines.len(),
            Some(AnswerValue::Files(files)) => files.len(),
            _ => 0,
        }
    }

    pub fn answer_select_next(&mut self) {
        let last = self.answer_rows().saturating_sub(1);
        self.answer_selected = self.answer_selected.saturating_add(1).min(last);
    }

    pub fn answer_select_prev(&mut self) {
        self.answer_selected = self.answer_selected.saturating_sub(1);
    }

    /// The file location the answer view's selection is on, if it is showing
    /// files at all. What `e` opens, and what the footer names.
    pub fn selected_answer_file(&self) -> Option<&FileLocation> {
        match self
            .answer
            .as_ref()
            .and_then(|answer| answer.value.as_ref())
        {
            Some(AnswerValue::Files(files)) => files.get(self.answer_selected),
            _ => None,
        }
    }

    pub fn file_select_next(&mut self) {
        let last = self.file_paths().len().saturating_sub(1);
        self.file_selected = self.file_selected.saturating_add(1).min(last);
    }

    pub fn file_select_prev(&mut self) {
        self.file_selected = self.file_selected.saturating_sub(1);
    }

    pub fn toggle_file_scope(&mut self) {
        self.file_show_all = !self.file_show_all;
        self.file_selected = 0;
    }

    /// Files explicitly touched by an event, plus path-like text mentions that
    /// currently resolve to files. Paths retain their reported spelling so the
    /// Files renderer can distinguish workspace-relative and external roots.
    pub fn file_paths(&self) -> Vec<String> {
        let entries: Box<dyn Iterator<Item = &Entry> + '_> = if self.file_show_all {
            Box::new(self.timeline.entries.iter())
        } else {
            Box::new(self.timeline.selected_entry().into_iter())
        };
        let mut paths = Vec::new();
        for entry in entries {
            if let AgentEvent::FileChanged { paths: changed, .. } = &entry.event {
                paths.extend(changed.iter().cloned());
            }
            let mut text = entry.event.summary();
            for block in entry.event.detail() {
                text.push('\n');
                match block {
                    DetailBlock::Text(part) | DetailBlock::Code { text: part, .. } => {
                        text.push_str(&part)
                    }
                }
            }
            for token in text.split_whitespace() {
                let candidate = token.trim_matches(|ch: char| {
                    matches!(
                        ch,
                        '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':' | ';'
                    )
                });
                if candidate.is_empty() || (!candidate.contains('/') && !candidate.contains('.')) {
                    continue;
                }
                let path = PathBuf::from(candidate);
                let resolved = if path.is_absolute() {
                    match path.strip_prefix(&SandboxLayout::default().workspace) {
                        Ok(relative) => self
                            .workspace_root
                            .as_ref()
                            .map(|root| root.join(relative))
                            .unwrap_or(path),
                        Err(_) => path,
                    }
                } else if let Some(root) = &self.workspace_root {
                    root.join(path)
                } else {
                    continue;
                };
                if resolved.is_file() {
                    paths.push(candidate.to_owned());
                }
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Resolve the selected Files-view entry to the corresponding host path.
    ///
    /// In the answer view it is the selected location of a `files` answer
    /// instead, so `e` opens what the agent named without a second mechanism
    /// for doing the same thing.
    pub fn selected_file_path(&self) -> Option<PathBuf> {
        if self.view == View::Answer {
            let file = self.selected_answer_file()?;
            let root = self
                .workspace_root
                .clone()
                .or_else(|| std::env::current_dir().ok())?;
            return Some(if file.path.is_absolute() {
                file.path.clone()
            } else {
                root.join(&file.path)
            });
        }
        let root = self
            .workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok())?;
        let sandbox_workspace = SandboxLayout::default().workspace;
        let mut paths = self
            .file_paths()
            .into_iter()
            .map(|reported| {
                let path = PathBuf::from(reported);
                let resolved = if path.is_absolute() {
                    path.strip_prefix(&sandbox_workspace)
                        .map(|relative| root.join(relative))
                        .unwrap_or(path)
                } else {
                    root.join(path)
                };
                let (display_root, relative) = match resolved.strip_prefix(&root) {
                    Ok(relative) => (root.clone(), relative.to_path_buf()),
                    Err(_) => {
                        let display_root =
                            resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
                        let relative = resolved
                            .strip_prefix(&display_root)
                            .unwrap_or(&resolved)
                            .to_path_buf();
                        (display_root, relative)
                    }
                };
                ((display_root, relative), resolved)
            })
            .collect::<Vec<_>>();
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        paths
            .get(self.file_selected)
            .map(|(_, resolved)| resolved.clone())
    }

    pub fn toggle_preview_mode(&mut self) {
        self.preview_mode = match self.preview_mode {
            PresentationMode::Pretty => PresentationMode::Raw,
            PresentationMode::Raw => PresentationMode::Pretty,
        };
        self.preview.reset();
    }

    pub fn toggle_preview_target(&mut self) {
        self.preview_target = match self.preview_target {
            PreviewTarget::Selection => PreviewTarget::Command,
            PreviewTarget::Command => PreviewTarget::Selection,
        };
        self.preview.reset();
    }

    /// The entry the preview panel and the `y` shortcut act on: the selected
    /// one, or — in [`PreviewTarget::Command`] — the newest shell entry. That
    /// entry holds both the command and its result, since a completion
    /// replaces its start row in place (see [`Self::push_event`]). Falls back
    /// to the selection while no command has run yet, so the panel is never
    /// blank just because the mode is on.
    pub fn preview_entry(&self) -> Option<&Entry> {
        if self.preview_target == PreviewTarget::Command {
            if let Some(entry) = self.timeline.newest_command() {
                return Some(entry);
            }
        }
        self.timeline.selected_entry()
    }

    /// Record the host directory backing the agent's workspace, so the
    /// preview panel can resolve a changed file's path to its current
    /// content on disk.
    pub fn set_workspace_root(&mut self, path: PathBuf) {
        self.working_directory = Some(path.clone());
        self.workspace_root = Some(path);
    }

    pub fn set_working_directory(&mut self, path: PathBuf) {
        self.working_directory = Some(path);
    }

    /// Whether the launch policy can still be edited; see [`launch::editable`].
    pub fn can_edit_launch(&self) -> bool {
        launch::editable(&self.status)
    }

    /// Refuse an edit to the launch policy when there is nothing to edit,
    /// saying why. Returns whether the caller may proceed. Public because the
    /// keys for it live in [`crate::launch`], and because one of them leaves
    /// this process to do its work (writing the defaults file) and must refuse
    /// with the same words as the rest.
    pub fn allow_launch_edit(&mut self) -> bool {
        if self.can_edit_launch() {
            return true;
        }
        self.show_action_message("the launch policy is fixed while an interaction is running");
        false
    }

    /// Plain text for whatever the current view treats as "the selected
    /// entry", for the `y` shortcut to send to the clipboard. `None` where the
    /// view has no single selected thing to copy (the log and transcript read
    /// as a continuous stream rather than discrete entries, and Driva shows
    /// static session-wide fields).
    pub fn copy_text(&self) -> Option<String> {
        match self.view {
            // The same uncapped, presented detail the preview panel shows —
            // its own doc comment already calls out being copy-friendly, so
            // `y` here just automates the selection an operator would
            // otherwise make by hand.
            View::Events | View::Preview => {
                let entry = self.preview_entry()?;
                let protocol = self.selection.provider.protocol();
                let mut text = String::new();
                for block in protocol.presented_detail(&entry.event, self.preview_mode) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    match block {
                        DetailBlock::Text(part) | DetailBlock::Code { text: part, .. } => {
                            text.push_str(&part)
                        }
                    }
                }
                if text.is_empty() {
                    text = protocol.presented_summary(&entry.event, self.preview_mode);
                }
                Some(text)
            }
            View::Raw => self
                .raw
                .get(self.raw_selected)
                .map(|line| line.text.clone()),
            View::Files => self
                .selected_file_path()
                .map(|path| path.display().to_string()),
            // A navigable answer copies the selected row; one that is read
            // rather than navigated copies the whole value, which is what an
            // operator reaching for `y` on a JSON or prose answer wants.
            View::Answer => match self.answer.as_ref()?.value.as_ref() {
                Some(AnswerValue::Lines(lines)) => lines.get(self.answer_selected).cloned(),
                Some(AnswerValue::Files(_)) => {
                    self.selected_answer_file().map(FileLocation::located)
                }
                Some(AnswerValue::Text(text)) => Some(text.clone()),
                Some(AnswerValue::Json(json)) => serde_json::to_string_pretty(json).ok(),
                // Nothing parsed, so the reply itself is the only thing there
                // is to copy — and the thing worth looking at.
                None => Some(self.answer.as_ref()?.source.clone()),
            },
            View::Log | View::Transcript | View::Driva => None,
        }
    }

    // --- Focus ---------------------------------------------------------------

    pub fn enter_input(&mut self) {
        self.focus = Focus::Input;
    }

    pub fn enter_list(&mut self) {
        self.focus = Focus::List;
    }

    // --- Message editing -----------------------------------------------------
    //
    // The buffer and its history are [`Composer`]'s; what is left here is the
    // one part of sending that is not — a sent message leaves the box, and the
    // rest of the client wants it.

    /// Take the trimmed message for sending, clearing the buffer. Returns
    /// `None` when the buffer holds only whitespace.
    pub fn take_message(&mut self) -> Option<String> {
        self.composer.take()
    }

    /// Ask the event loop for something this screen cannot do itself; see
    /// [`Request`].
    pub fn ask(&mut self, request: Request) {
        self.request = Some(request);
    }

    /// Take the operator's pending request, if any, for the event loop to act on.
    pub fn take_request(&mut self) -> Option<Request> {
        self.request.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::LaunchColumn;
    use styra_server::agent::Effort;

    fn app() -> App {
        App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "session-1",
        )
    }

    #[test]
    fn following_tracks_the_newest_entry() {
        let mut app = app();
        app.push_event(AgentEvent::TurnStarted);
        app.push_event(AgentEvent::AgentMessage { text: "hi".into() });
        assert!(app.timeline.follow);
        assert_eq!(app.timeline.selected, 1);
    }

    #[test]
    fn following_ignores_hidden_minor_events() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage { text: "hi".into() });
        app.timeline.entries[0].expanded = true;
        app.preview.offset = 3;

        app.push_event(AgentEvent::TurnStarted);

        assert!(app.timeline.follow);
        assert_eq!(app.timeline.selected, 0);
        assert_eq!(app.preview.offset, 3);
        assert!(app.timeline.is_visible(app.timeline.selected));
        assert!(app.timeline.entries[0].expanded);
        assert!(!app.timeline.entries[1].expanded);
    }

    #[test]
    fn following_transfers_expansion_to_a_new_visible_entry() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage {
            text: "first".into(),
        });
        app.timeline.entries[0].expanded = true;

        app.push_event(AgentEvent::AgentMessage {
            text: "second".into(),
        });

        assert_eq!(app.timeline.selected, 1);
        assert!(!app.timeline.entries[0].expanded);
        assert!(app.timeline.entries[1].expanded);
    }

    #[test]
    fn moving_up_pins_the_view_and_reaching_the_tail_resumes_follow() {
        let mut app = app();
        // Multi-line so every entry has detail and so is reachable by the
        // has-detail-only select_next/select_prev this test exercises.
        for _ in 0..3 {
            app.push_event(AgentEvent::AgentMessage {
                text: "x\nmore x".into(),
            });
        }
        app.select_prev();
        assert!(!app.timeline.follow);
        assert_eq!(app.timeline.selected, 1);

        // New events no longer move the selection while pinned.
        app.push_event(AgentEvent::AgentMessage {
            text: "x\nmore x".into(),
        });
        assert_eq!(app.timeline.selected, 1);

        // Walking back down to the tail re-enables follow.
        app.select_next();
        app.select_next();
        app.select_next();
        assert!(app.timeline.follow);
        assert_eq!(app.timeline.selected, app.timeline.entries.len() - 1);
    }

    #[test]
    fn moving_up_by_line_pins_the_view_and_reaching_the_tail_resumes_follow() {
        // Same follow/pin contract as select_next/select_prev, but for
        // select_next_line/select_prev_line (j/k), which move one visible
        // entry at a time regardless of whether it has detail.
        let mut app = app();
        for _ in 0..3 {
            app.push_event(AgentEvent::AgentMessage { text: "x".into() });
        }
        app.select_prev_line();
        assert!(!app.timeline.follow);
        assert_eq!(app.timeline.selected, 1);

        app.push_event(AgentEvent::AgentMessage { text: "x".into() });
        assert_eq!(app.timeline.selected, 1);

        app.select_next_line();
        app.select_next_line();
        app.select_next_line();
        assert!(app.timeline.follow);
        assert_eq!(app.timeline.selected, app.timeline.entries.len() - 1);
    }

    #[test]
    fn thinking_updates_fold_into_one_line_that_shows_the_latest_count() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::Thinking {
            text: "weigh the options".into(),
            tokens: None,
        });
        app.push_event(AgentEvent::Thinking {
            text: String::new(),
            tokens: Some(64),
        });
        app.push_event(AgentEvent::Thinking {
            text: String::new(),
            tokens: Some(512),
        });

        assert_eq!(app.timeline.entries.len(), 2);
        assert_eq!(
            app.timeline.entries[1].event,
            AgentEvent::Thinking {
                text: "weigh the options".into(),
                tokens: Some(512),
            }
        );

        // A new run of thinking after other work starts its own line.
        app.push_event(AgentEvent::AgentMessage { text: "b".into() });
        app.push_event(AgentEvent::Thinking {
            text: String::new(),
            tokens: Some(8),
        });
        assert_eq!(app.timeline.entries.len(), 4);
    }

    #[test]
    fn expansion_is_per_entry_and_bulk_toggles_work() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::AgentMessage { text: "b".into() });

        app.select_first();
        app.timeline.toggle_expand();
        assert!(app.timeline.entries[0].expanded);
        assert!(!app.timeline.entries[1].expanded);

        app.timeline.expand_all();
        assert!(app.timeline.entries.iter().all(|entry| entry.expanded));
        app.timeline.collapse_all();
        assert!(app.timeline.entries.iter().all(|entry| !entry.expanded));
    }

    #[test]
    fn expanding_only_selected_collapses_every_other_entry() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::AgentMessage { text: "b".into() });
        app.push_event(AgentEvent::AgentMessage { text: "c".into() });
        app.timeline.expand_all();

        app.select_first();
        app.select_next_line();
        app.timeline.expand_only_selected();

        assert!(!app.timeline.entries[0].expanded);
        assert!(app.timeline.entries[1].expanded);
        assert!(!app.timeline.entries[2].expanded);
    }

    #[test]
    fn status_follows_turn_lifecycle_and_captures_usage() {
        let mut app = app();
        assert_eq!(app.status, Status::Running);
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage {
                input_tokens: 7,
                ..Default::default()
            },
        });
        assert_eq!(app.status, Status::Idle);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 7);

        app.push_event(AgentEvent::UserMessage {
            text: "more".into(),
        });
        assert_eq!(app.status, Status::Running);
    }

    /// A typed turn arrives framed, since the server appends the contract's
    /// instructions before sending. The list shows the operator's own message
    /// and records what it asked for, rather than a screen of boilerplate.
    #[test]
    fn a_framed_operator_message_is_shown_as_it_was_written() {
        let mut app = app();
        app.push_event(AgentEvent::UserMessage {
            text: styra_server::contract::frame("which files handle auth?", Contract::Files),
        });
        let entry = app.timeline.entries.last().expect("the message was pushed");
        assert_eq!(
            entry.event,
            AgentEvent::UserMessage {
                text: "which files handle auth?".into()
            }
        );
        assert_eq!(entry.contract, Some(Contract::Files));
    }

    /// An ordinary message is not framed and must be left exactly as it is,
    /// including one that happens to talk about answer blocks.
    #[test]
    fn an_unframed_operator_message_is_untouched() {
        let mut app = app();
        let text = "explain the <styra:answer> convention";
        app.push_event(AgentEvent::UserMessage { text: text.into() });
        let entry = app.timeline.entries.last().expect("the message was pushed");
        assert_eq!(entry.event, AgentEvent::UserMessage { text: text.into() });
        assert_eq!(entry.contract, None);
    }

    #[test]
    fn background_task_keeps_idle_status_explicitly_active() {
        let mut app = app();
        app.push_event(AgentEvent::ToolStarted {
            id: "bash-1".into(),
            name: "Bash".into(),
            detail: r#"{"command":"cargo test","run_in_background":true}"#.into(),
        });
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert_eq!(app.status, Status::Background);
        assert_eq!(app.status.label(), "idle · background work running");

        app.push_event(AgentEvent::ToolStarted {
            id: "poll-1".into(),
            name: "TaskOutput".into(),
            detail: r#"{"task_id":"bash-1"}"#.into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "poll-1".into(),
            name: "poll-1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "Task completed successfully".into(),
        });
        assert_eq!(app.status, Status::Idle);
    }

    #[test]
    fn reported_empty_background_set_clears_the_status_without_a_poll() {
        let mut app = app();
        app.push_event(AgentEvent::ToolStarted {
            id: "bash-1".into(),
            name: "Bash".into(),
            detail: r#"{"command":"cargo test","run_in_background":true}"#.into(),
        });
        app.push_event(AgentEvent::BackgroundTasks { running: 1 });
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert_eq!(app.status, Status::Background);

        // The agent never polls the task; it reads the output file directly
        // and the provider reports the set is empty. That must clear.
        app.push_event(AgentEvent::BackgroundTasks { running: 0 });
        assert_eq!(app.status, Status::Idle);
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert_eq!(app.status, Status::Idle);
    }

    #[test]
    fn a_poll_of_one_task_does_not_clear_a_reported_second_one() {
        let mut app = app();
        app.push_event(AgentEvent::BackgroundTasks { running: 2 });
        app.push_event(AgentEvent::ToolStarted {
            id: "poll-1".into(),
            name: "TaskOutput".into(),
            detail: r#"{"task_id":"one"}"#.into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "poll-1".into(),
            name: "poll-1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "Task completed successfully".into(),
        });
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert_eq!(app.status, Status::Background);
    }

    #[test]
    fn queued_messages_are_fifo_and_can_be_cleared() {
        let mut app = app();
        app.queue_message(QueuedMessage::new("first"));
        app.queue_message(QueuedMessage::new("second"));

        assert_eq!(app.queued_message_count(), 2);
        assert_eq!(app.take_queued_message(), Some(QueuedMessage::new("first")));
        assert_eq!(
            app.take_queued_message(),
            Some(QueuedMessage::new("second"))
        );
        assert_eq!(app.take_queued_message(), None);

        app.queue_message(QueuedMessage::new("keep until Esc"));
        assert_eq!(app.clear_queued_messages(), 1);
        assert_eq!(app.queued_message_count(), 0);
    }

    /// A message queued while the agent was busy keeps the shape it was
    /// composed with, so it still asks for one when the queue drains.
    #[test]
    fn a_queued_message_keeps_its_contract() {
        let mut app = app();
        app.queue_message(QueuedMessage::new("which files?").asking_for(Some(Contract::Files)));
        let queued = app.take_queued_message().expect("the message was queued");
        assert_eq!(queued.text, "which files?");
        assert_eq!(queued.contract, Some(Contract::Files));
    }

    #[test]
    fn automatic_action_messages_accumulate() {
        let mut app = app();
        assert!(app.action_messages.is_empty());

        app.show_action_message("first action");
        app.show_action_message("second action");
        assert_eq!(app.action_messages.len(), 2);
        assert_eq!(app.action_messages[0].text, "first action");
        assert_eq!(app.action_messages[1].text, "second action");
    }

    #[test]
    fn automatic_action_messages_expire_after_five_seconds() {
        let mut app = app();
        app.show_action_message("old action");
        app.action_messages[0].shown_at = Instant::now() - Duration::from_secs(5);

        app.expire_action_messages();

        assert!(app.action_messages.is_empty());
    }

    #[test]
    fn usage_updates_mid_turn_refresh_the_display_without_ending_the_turn() {
        // The app-server protocol reports a token-usage snapshot after every
        // step within a turn (each tool call, each model round), not just the
        // last one. Only a real `TurnCompleted` should flip the status to
        // idle; `UsageUpdated` must not, or the indicator falsely reads idle
        // while the agent is still actively working.
        let mut app = app();
        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test".into(),
        });
        assert_eq!(app.status, Status::Running);

        app.push_event(AgentEvent::UsageUpdated {
            usage: TokenUsage {
                input_tokens: 10,
                ..Default::default()
            },
        });
        assert_eq!(
            app.status,
            Status::Running,
            "a usage ping mid-turn must not end it"
        );
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 10);

        app.push_event(AgentEvent::CommandStarted {
            command: "cargo build".into(),
        });
        app.push_event(AgentEvent::UsageUpdated {
            usage: TokenUsage {
                input_tokens: 20,
                ..Default::default()
            },
        });
        assert_eq!(app.status, Status::Running);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 20);

        // The app-server's real end-of-turn signal carries no usage of its
        // own; the last reported usage must survive it, not reset to zero.
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert_eq!(app.status, Status::Idle);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 20);
    }

    #[test]
    fn a_tool_completion_replaces_its_started_row_and_recovers_the_tool_name() {
        // Claude's `tool_result` only ever repeats the `tool_use_id`, never
        // the tool's name, so the merge must pull the name back from the
        // matching `ToolStarted` row rather than leaving the placeholder id
        // on screen, and it must not append a second line.
        let mut app = app();
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "ok".into(),
        });

        assert_eq!(app.timeline.entries.len(), 1);
        assert_eq!(
            app.timeline.entries[0].event,
            AgentEvent::ToolCompleted {
                id: "toolu_1".into(),
                name: "Bash".into(),
                detail: "{\"command\":\"cargo test\"}".into(),
                status: "completed".into(),
                output: "ok".into(),
            }
        );
    }

    #[test]
    fn a_clean_edit_completion_is_dropped_since_the_file_changed_row_already_shows_it() {
        // Edit/Write/MultiEdit surface as `FileChanged` at start, not
        // `ToolStarted`, so their `ToolCompleted` used to find no match and
        // fall through to a second, id-only line. A clean result adds
        // nothing the `FileChanged` row didn't already show.
        let mut app = app();
        app.push_event(AgentEvent::FileChanged {
            id: "toolu_2".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ edit @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_2".into(),
            name: "toolu_2".into(),
            detail: String::new(),
            status: "completed".into(),
            output: String::new(),
        });

        assert_eq!(app.timeline.entries.len(), 1);
        assert!(matches!(
            app.timeline.entries[0].event,
            AgentEvent::FileChanged { .. }
        ));
    }

    #[test]
    fn a_failed_edit_completion_replaces_the_file_changed_row_with_a_visible_error() {
        let mut app = app();
        app.push_event(AgentEvent::FileChanged {
            id: "toolu_3".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ edit @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_3".into(),
            name: "toolu_3".into(),
            detail: String::new(),
            status: "error".into(),
            output: "old_string not found".into(),
        });

        assert_eq!(app.timeline.entries.len(), 1);
        assert_eq!(
            app.timeline.entries[0].event,
            AgentEvent::Error {
                message: "src/lib.rs: old_string not found".into(),
            }
        );
    }

    #[test]
    fn ending_is_terminal_for_events_but_not_can_send() {
        let mut app = app();
        app.on_ended(InteractionEnd {
            exit_code: Some(0),
            error: None,
        });
        assert_eq!(
            app.status,
            Status::Ended {
                exit_code: Some(0),
                error: None
            }
        );
        // `can_send` only flags the input box's title; the event loop still
        // lets a new message resume an ended Session through its provider.
        assert!(!app.can_send());
        // A late event does not revive an ended session.
        app.push_event(AgentEvent::AgentMessage {
            text: "late".into(),
        });
        assert!(matches!(app.status, Status::Ended { .. }));
    }

    #[test]
    fn pending_opens_in_list_focus_with_no_session_yet_and_allows_sending() {
        let app = App::pending(Selection::new(Provider::Codex));
        assert_eq!(app.status, Status::Pending);
        assert_eq!(app.focus, Focus::List);
        assert!(app.session_id.is_empty());
        assert!(app.composer.text.is_empty());
        // The message box must not read "session ended" before anything ran.
        assert!(app.can_send());
    }

    #[test]
    fn set_input_prefills_the_message_box_for_the_operator_to_edit_or_send() {
        let mut app = App::pending(Selection::new(Provider::Codex));
        app.set_input("earlier session's transcript".into());
        assert_eq!(app.composer.text, "earlier session's transcript");
        assert_eq!(
            app.take_message(),
            Some("earlier session's transcript".into())
        );
    }

    /// The whole point of the start screen: all three of agent, model, and
    /// effort are still open, and picking them only records what the first
    /// message will launch.
    #[test]
    fn the_launcher_records_a_provider_model_and_effort_without_launching() {
        let mut app = App::pending(Selection::new(Provider::Codex));
        assert!(app.can_configure_launch());
        app.open_launcher();
        let launcher = app.launcher.as_mut().expect("the picker is reachable");

        // Provider column: move to Claude Code.
        assert_eq!(launcher.column, LaunchColumn::Provider);
        launcher.next();
        launcher.next();
        assert_eq!(launcher.provider(), Provider::Claude);

        // Model column: every row is a model, and it opens on the agent's declared
        // default, so one step lands on the entry after that.
        launcher.next_column();
        assert_eq!(launcher.selection().model, Provider::Claude.default_model());
        launcher.next();
        let models = Provider::Claude.models();
        let default = models
            .iter()
            .position(|model| *model == Provider::Claude.default_model())
            .unwrap_or(0);
        let model = models[default + 1].to_owned();
        assert_eq!(launcher.selection().model, model);

        // Effort column, likewise — the ladder itself, no extra row.
        launcher.next_column();
        assert_eq!(launcher.column, LaunchColumn::Effort);
        let effort = Provider::Claude.efforts()[0];
        while launcher.selection().effort != effort {
            launcher.prev();
        }
        assert_eq!(launcher.selection().effort, effort);

        app.confirm_launcher();
        assert!(app.launcher.is_none());
        assert_eq!(app.selection.provider, Provider::Claude);
        assert_eq!(app.selection.model, model);
        assert_eq!(app.selection.effort, effort);
        // Nothing was launched or sent by the picking itself.
        assert_eq!(app.status, Status::Pending);
        assert!(app.session_id.is_empty());
        assert!(app.timeline.entries.is_empty());
    }

    #[test]
    fn the_launcher_opens_on_the_current_selection_and_cancelling_keeps_it() {
        let selection = Selection::parse("claude:claude-sonnet-5/xhigh").unwrap();
        let mut app = App::pending(selection.clone());
        app.open_launcher();
        let launcher = app.launcher.as_ref().unwrap();
        assert_eq!(launcher.selection(), selection, "opens on what is chosen");

        app.launcher.as_mut().unwrap().next();
        app.cancel_launcher();
        assert!(app.launcher.is_none());
        assert_eq!(app.selection, selection, "cancelling changes nothing");
    }

    /// A model the catalog no longer lists can still arrive from stored state,
    /// and confirming must not silently replace it. The picker carries it as
    /// its own row; it cannot author one.
    #[test]
    fn a_model_outside_the_catalog_is_carried_as_its_own_row() {
        let selection = Selection::parse("claude:claude-opus-4-1-20250805").unwrap();
        let mut app = App::pending(selection.clone());
        app.open_launcher();
        let launcher = app.launcher.as_ref().unwrap();
        assert_eq!(
            launcher.carried_model.as_deref(),
            Some("claude-opus-4-1-20250805")
        );
        assert_eq!(
            launcher.model,
            Provider::Claude.models().len(),
            "carried last, after the catalog"
        );
        assert_eq!(launcher.selection().model, selection.model);

        // Confirming keeps the model rather than falling back to a catalogued
        // one — but it does pin an effort, since the picker has no row for
        // leaving that to the agent. The launch asked for none, so it lands on
        // the opening row.
        app.confirm_launcher();
        assert_eq!(app.selection.model, selection.model);
        assert_eq!(app.selection.effort, Effort::High);

        // Having just been confirmed it is now the most recently selected
        // model, so reopening lists it first — ahead of the catalog rather
        // than after it. It is an ordinary row either way, just not one the
        // picker could write.
        app.open_launcher();
        let launcher = app.launcher.as_mut().unwrap();
        launcher.next_column();
        assert_eq!(launcher.model, 0, "carried first, ahead of the catalog");
        assert_eq!(launcher.selection().model, selection.model);
        launcher.next();
        assert_eq!(
            launcher.selection().model,
            *Provider::Claude.models().first().unwrap()
        );
        launcher.prev();
        assert_eq!(launcher.selection().model, selection.model);
    }

    /// Confirming a model moves it to the front of the ordering, and the list
    /// never grows a duplicate of a model chosen twice.
    #[test]
    fn confirming_records_the_model_as_the_most_recent_one() {
        let mut app = App::pending(Selection::parse("claude:claude-sonnet-5").unwrap());
        app.recent_models = vec!["claude-opus-5".into(), "claude-sonnet-5".into()];

        app.open_launcher();
        app.confirm_launcher();

        assert_eq!(
            app.recent_models,
            vec!["claude-sonnet-5".to_owned(), "claude-opus-5".to_owned()]
        );
    }

    /// Once a session is up, its agent and model are facts about a running
    /// process; the picker is a property of the *next* one.
    #[test]
    fn the_launcher_is_unreachable_once_something_has_been_launched() {
        let mut app = app();
        assert_eq!(app.status, Status::Running);
        assert!(!app.can_configure_launch());
        app.open_launcher();
        assert!(app.launcher.is_none());
    }

    #[test]
    fn an_idle_codex_thread_can_change_its_next_turn_model() {
        let mut app = App::new(Selection::new(Provider::Codex), "session-1");
        app.status = Status::Idle;
        assert!(app.can_configure_launch());
        app.open_launcher();
        assert!(app.launcher.is_some());
    }

    #[test]
    fn an_idle_claude_thread_can_change_model_but_not_effort() {
        let original = Selection::parse("claude:claude-sonnet-5/high").unwrap();
        let mut app = App::new(original.clone(), "session-1");
        app.status = Status::Idle;
        app.open_launcher();
        let launcher = app.launcher.as_mut().unwrap();
        while launcher.selection().model == original.model {
            launcher.next_column();
            launcher.next();
        }
        launcher.next_column();
        launcher.next();
        app.confirm_launcher();

        assert_ne!(app.selection.model, original.model);
        assert_eq!(app.selection.effort, original.effort);
    }

    /// The operator's switch has to reach the agent when they make it, not
    /// whenever they next happen to type something.
    #[test]
    fn switching_a_live_sessions_model_asks_the_server_to_apply_it_now() {
        let mut app = App::new(Selection::new(Provider::Codex), "session-1");
        app.status = Status::Idle;
        app.open_launcher();
        let launcher = app.launcher.as_mut().unwrap();
        launcher.next_column();
        launcher.next();
        app.confirm_launcher();

        assert_eq!(app.take_request(), Some(Request::ApplySelection));
    }

    #[test]
    fn confirming_the_same_selection_asks_the_server_for_nothing() {
        let mut app = App::new(Selection::new(Provider::Codex), "session-1");
        app.status = Status::Idle;
        app.open_launcher();
        app.confirm_launcher();

        assert_eq!(app.take_request(), None);
    }

    /// Before launch there is no interaction to tell; the first message
    /// carries the selection.
    #[test]
    fn choosing_a_model_before_launch_asks_the_server_for_nothing() {
        let mut app = App::pending(Selection::new(Provider::Codex));
        app.open_launcher();
        let launcher = app.launcher.as_mut().unwrap();
        launcher.next_column();
        launcher.next();
        app.confirm_launcher();

        assert_eq!(app.take_request(), None);
    }

    #[test]
    fn a_running_turn_cannot_change_model() {
        let mut app = App::new(Selection::new(Provider::Codex), "session-1");
        app.status = Status::Running;
        assert!(!app.can_configure_launch());
        app.open_launcher();
        assert!(app.launcher.is_none());
    }

    /// A live or replayed Session carries the exact selection it was created
    /// with, independently of the client's saved default for new Sessions.
    #[test]
    fn a_session_keeps_its_recorded_selection() {
        let app = App::new(
            styra_server::agent::Selection::parse("claude:opus/xhigh").unwrap(),
            "session-1",
        );
        assert_eq!(
            app.selection,
            Selection::parse("claude:opus/xhigh").unwrap()
        );
    }

    /// What the status line names before the agent has spoken: the launch it was
    /// started with, model and effort included, marked as not yet confirmed.
    #[test]
    fn the_launch_label_falls_back_to_the_requested_selection() {
        let app = App::new(
            styra_server::agent::Selection::parse("claude:opus/max").unwrap(),
            "s-1",
        );
        let label = app.launch_label();
        assert_eq!(label.agent, "claude");
        assert_eq!(label.model.as_deref(), Some("opus"));
        assert_eq!(label.effort.as_deref(), Some("max"));
        assert!(
            !label.model_reported,
            "nothing has been reported by the agent yet"
        );
        assert!(!label.effort_reported);

        // Short launch syntax is normalized to the provider's declared defaults.
        let app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s-2",
        );
        let label = app.launch_label();
        assert_eq!(label.agent, "codex");
        assert_eq!(
            label.model.as_deref(),
            Some(Provider::Codex.default_model())
        );
        assert_eq!(
            label.effort.as_deref(),
            Some(Provider::Codex.default_effort().as_str())
        );
    }

    /// The agent's own report is what is actually running, so it replaces the
    /// launch request.
    #[test]
    fn a_reported_model_and_effort_replace_the_requested_ones() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s-1",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(label.effort.as_deref(), Some("high"));
        assert!(label.model_reported);
        assert!(label.effort_reported);

        // A launch that asked for something else is overruled by the fact.
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex:gpt-5.6-luna/low").unwrap(),
            "s-2",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("xhigh".into()),
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(label.effort.as_deref(), Some("xhigh"));
    }

    /// Claude Code names a model but never an effort, so the effort the session
    /// was launched with must survive its report rather than being blanked.
    #[test]
    fn an_unreported_effort_keeps_the_one_the_session_was_launched_with() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("claude:opus/max").unwrap(),
            "s-1",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "s-1".into(),
            model: Some("claude-opus-4-8".into()),
            effort: None,
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("claude-opus-4-8"));
        assert!(label.model_reported);
        // The effort is the launch's own word, and is not claimed as the
        // agent's.
        assert_eq!(label.effort.as_deref(), Some("max"));
        assert!(!label.effort_reported);

        // A thread reported with neither leaves the display as it was.
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "s-2",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: None,
            effort: None,
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("gpt-5.6-sol"));
        assert!(!label.model_reported);
    }

    #[test]
    fn a_request_is_recorded_for_the_event_loop_and_taken_exactly_once() {
        for request in [
            Request::Quit,
            Request::Workspace,
            Request::Sessions,
            Request::OpenSession("s-1".into()),
            Request::Interactions,
            Request::Reset,
            Request::NewSession,
            Request::EditFile,
        ] {
            let mut app = app();
            assert_eq!(app.take_request(), None);
            app.ask(request.clone());
            assert_eq!(app.take_request(), Some(request));
            // Taking it clears it, so the loop acts on a request once rather
            // than reopening the same picker on the next frame.
            assert_eq!(app.take_request(), None);
        }
    }

    #[test]
    fn raw_view_toggles_and_selects_from_the_tail() {
        use styra_server::{Direction, RawLine};
        let mut app = app();
        assert_eq!(app.view, View::Events);
        app.toggle_raw();
        assert_eq!(app.view, View::Raw);

        for i in 0..5 {
            app.push_raw(RawLine {
                at_ms: 0,
                direction: Direction::FromAgent,
                text: format!("line {i}"),
            });
        }
        assert_eq!(app.raw_selected, 4, "starts pinned to the tail");
        assert!(app.raw_follow);

        app.raw_select_prev();
        assert_eq!(app.raw_selected, 3);
        assert!(!app.raw_follow);
        // A new line while a specific line is selected keeps that same line
        // in view rather than yanking to the new tail.
        app.push_raw(RawLine {
            at_ms: 0,
            direction: Direction::ToAgent,
            text: "new".into(),
        });
        assert_eq!(app.raw_selected, 3);

        app.raw_select_last();
        assert_eq!(app.raw_selected, 5);
        assert!(app.raw_follow);
        app.raw_select_first();
        assert_eq!(app.raw_selected, 0);
        assert!(!app.raw_follow);
    }

    #[test]
    fn entering_raw_view_focuses_the_selected_entrys_wire_line() {
        use styra_server::{Direction, RawLine};
        let mut app = app();
        for i in 0..3 {
            app.push_raw(RawLine {
                at_ms: 0,
                direction: Direction::FromAgent,
                text: format!("{{\"n\":{i}}}"),
            });
            app.push_event(AgentEvent::AgentMessage {
                text: format!("message {i}"),
            });
        }
        // Step off the tail so the list is no longer following, landing on
        // the middle entry.
        app.select_prev_line();
        assert_eq!(app.timeline.selected, 1);
        assert!(!app.timeline.follow);

        app.toggle_raw();
        assert_eq!(
            app.raw_selected, 1,
            "focuses the wire line behind the selected entry"
        );
        assert!(!app.raw_follow);
    }

    #[test]
    fn log_view_toggles_independently_and_scrolls() {
        use styra_server::LogEntry;
        let mut app = app();
        app.toggle_raw();
        assert_eq!(app.view, View::Raw);
        // Toggling the log from the raw view switches to it, not back to events.
        app.toggle_view(View::Log);
        assert_eq!(app.view, View::Log);
        app.toggle_view(View::Log);
        assert_eq!(app.view, View::Events);

        for i in 0..4 {
            app.push_log(LogEntry::info(format!("entry {i}")));
        }
        assert_eq!(app.log_scroll_back, 0);
        app.log_scroll_up();
        assert_eq!(app.log_scroll_back, 1);
        app.push_log(LogEntry::warn("more"));
        assert_eq!(app.log_scroll_back, 2, "scrolled-up view stays put");
        app.log_to_bottom();
        assert_eq!(app.log_scroll_back, 0);
    }

    #[test]
    fn transcript_view_toggles_independently_and_scrolls_from_the_top() {
        let mut app = app();
        app.toggle_raw();
        assert_eq!(app.view, View::Raw);
        // Toggling the transcript from the raw view switches to it, not back
        // to events.
        app.toggle_view(View::Transcript);
        assert_eq!(app.view, View::Transcript);
        app.toggle_view(View::Transcript);
        assert_eq!(app.view, View::Events);

        app.toggle_view(View::Transcript);
        assert_eq!(
            app.transcript_scroll, 0,
            "starts at the beginning, not the tail"
        );

        app.transcript_scroll_down();
        app.transcript_scroll_down();
        assert_eq!(app.transcript_scroll, 2);
        app.transcript_scroll_up();
        assert_eq!(app.transcript_scroll, 1);

        app.transcript_to_bottom();
        assert_eq!(app.transcript_scroll, u16::MAX);
        app.transcript_to_top();
        assert_eq!(app.transcript_scroll, 0);
    }

    #[test]
    fn conversation_only_filters_the_event_list_without_changing_views() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage {
            text: "hello".into(),
        });
        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test".into(),
        });

        app.toggle_conversation_only();

        assert_eq!(app.view, View::Events);
        assert!(app.timeline.conversation_only);
        assert!(app.timeline.is_visible(0));
        assert!(!app.timeline.is_visible(1));

        app.toggle_conversation_only();
        assert!(!app.timeline.conversation_only);
        assert!(app.timeline.is_visible(1));
    }

    #[test]
    fn minor_events_are_hidden_and_skipped_by_navigation() {
        let mut app = app();
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: None,
            effort: None,
        });
        // Multi-line so each entry has detail beyond its summary, and so
        // qualifies for the has-detail navigation this test also exercises.
        app.push_event(AgentEvent::AgentMessage {
            text: "a\nmore a".into(),
        });
        app.push_event(AgentEvent::TurnStarted);
        app.push_event(AgentEvent::AgentMessage {
            text: "b\nmore b".into(),
        });
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });

        // Hidden by default; no toggle needed to get here.
        assert!(!app.timeline.show_minor);

        app.select_first();
        assert_eq!(
            app.timeline.entries[app.timeline.selected].event,
            AgentEvent::AgentMessage {
                text: "a\nmore a".into()
            }
        );

        app.select_next();
        assert_eq!(
            app.timeline.entries[app.timeline.selected].event,
            AgentEvent::AgentMessage {
                text: "b\nmore b".into()
            }
        );

        // No more visible entries after "b"; select_next is a no-op.
        app.select_next();
        assert_eq!(
            app.timeline.entries[app.timeline.selected].event,
            AgentEvent::AgentMessage {
                text: "b\nmore b".into()
            }
        );

        app.select_prev();
        assert_eq!(
            app.timeline.entries[app.timeline.selected].event,
            AgentEvent::AgentMessage {
                text: "a\nmore a".into()
            }
        );

        app.toggle_minor();
        assert!(app.timeline.show_minor);
    }

    #[test]
    fn select_next_and_prev_skip_entries_with_no_detail_beyond_their_summary() {
        let mut app = app();
        // A single line of text is entirely a restatement of the summary, so
        // this entry has no arrow and should not be a stop for J/K.
        app.push_event(AgentEvent::AgentMessage {
            text: "no detail here".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "has detail\nsecond line".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "also no detail".into(),
        });
        assert!(!app.timeline.entries[0].has_detail());
        assert!(app.timeline.entries[1].has_detail());
        assert!(!app.timeline.entries[2].has_detail());

        // `select_first` (bound to `g`) is unaffected by the has-detail
        // restriction: it lands on the very first visible entry regardless.
        app.select_first();
        assert_eq!(app.timeline.selected, 0);

        // The only entry with detail is index 1; select_next skips index 0's
        // lack of detail to land there, then has nothing further to skip to.
        app.select_next();
        assert_eq!(app.timeline.selected, 1);
        app.select_next();
        assert_eq!(app.timeline.selected, 1);
        // Equally, there is no navigable entry before it to skip back to.
        app.select_prev();
        assert_eq!(app.timeline.selected, 1);

        // j/k ignore the has-detail restriction and move one line at a time.
        app.select_next_line();
        assert_eq!(app.timeline.selected, 2);
        app.select_next_line();
        assert_eq!(
            app.timeline.selected, 2,
            "already at the last visible entry"
        );
        app.select_prev_line();
        assert_eq!(app.timeline.selected, 1);
        app.select_prev_line();
        assert_eq!(app.timeline.selected, 0);
    }

    #[test]
    fn file_events_are_navigable_even_without_fold_detail() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage {
            text: "plain summary".into(),
        });
        app.push_event(AgentEvent::FileChanged {
            id: String::new(),
            paths: vec!["notes.txt".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "another plain summary".into(),
        });
        assert!(!app.timeline.entries[1].has_detail());

        app.select_first();
        app.select_next();
        assert_eq!(app.timeline.selected, 1);
        app.select_next();
        assert_eq!(app.timeline.selected, 1);
    }

    #[test]
    fn preview_scroll_resets_when_selection_changes() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage {
            text: "first\nbody".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "second\nbody".into(),
        });
        app.select_first();
        app.preview.note_limit(100);
        app.preview.page_down();
        assert_eq!(app.preview.offset, 10);

        app.select_next();
        assert_eq!(app.timeline.selected, 1);
        assert_eq!(app.preview.offset, 0);
    }

    #[test]
    fn preview_page_down_does_not_accumulate_past_the_rendered_end() {
        let mut app = app();
        app.preview.note_limit(23);

        for _ in 0..100 {
            app.preview.page_down();
        }
        assert_eq!(app.preview.offset, 23);

        app.preview.page_up();
        assert_eq!(app.preview.offset, 13);
    }

    #[test]
    fn toggling_minor_off_moves_selection_off_a_hidden_entry() {
        let mut app = app();
        app.toggle_minor(); // show minor events so follow can land on one
        assert!(app.timeline.show_minor);

        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::TurnStarted);
        // Selection sits on the just-pushed minor entry via follow.
        assert_eq!(app.timeline.selected, 1);

        app.toggle_minor(); // hide them again
        assert!(!app.timeline.show_minor);
        assert!(app.timeline.is_visible(app.timeline.selected));
        assert_eq!(
            app.timeline.entries[app.timeline.selected].event,
            AgentEvent::AgentMessage { text: "a".into() }
        );
    }

    #[test]
    fn workspace_root_is_unset_until_the_host_records_it() {
        let mut app = app();
        assert_eq!(app.workspace_root, None);
        app.set_workspace_root(PathBuf::from("/home/op/project"));
        assert_eq!(app.workspace_root, Some(PathBuf::from("/home/op/project")));
    }

    #[test]
    fn file_view_collects_focused_and_session_paths() {
        let root = std::env::temp_dir().join(format!("styra-file-view-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "read me\n").unwrap();
        let mut app = app();
        app.set_workspace_root(root.clone());
        app.push_event(AgentEvent::AgentMessage {
            text: "see README.md".into(),
        });
        app.timeline.follow = false;
        app.push_event(AgentEvent::FileChanged {
            id: "1".into(),
            paths: vec!["src/lib.rs".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });

        assert_eq!(app.file_paths(), vec!["README.md"]);
        app.toggle_file_scope();
        assert_eq!(app.file_paths(), vec!["README.md", "src/lib.rs"]);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The cycle ends at "no contract" so an operator who opened one can get
    /// back out of it without leaving the message box.
    #[test]
    fn cycling_contracts_returns_to_an_untyped_turn() {
        let mut app = app();
        assert_eq!(app.contract, None);
        for expected in App::CONTRACTS {
            app.cycle_contract();
            assert_eq!(app.contract, Some(expected));
        }
        app.cycle_contract();
        assert_eq!(app.contract, None);
    }

    /// A contract belongs to the message it was chosen for. Taking it is what
    /// stops the next, unrelated message from being typed too.
    #[test]
    fn taking_a_contract_clears_it_for_the_following_message() {
        let mut app = app();
        app.cycle_contract();
        assert_eq!(app.take_contract(), Some(Contract::Text));
        assert_eq!(app.contract, None);
        assert_eq!(app.take_contract(), None);
    }

    /// Opening the view asks for the answer; it cannot render one it never
    /// fetched, and the operator should not have to press a second key.
    #[test]
    fn opening_the_answer_view_requests_the_answer() {
        let mut app = app();
        app.toggle_answer();
        assert_eq!(app.view, View::Answer);
        assert_eq!(app.take_request(), Some(Request::Answer { contract: None }));

        app.toggle_answer();
        assert_eq!(app.view, View::Events);
        // Leaving asks for nothing: there is no answer to fetch for a view
        // that is no longer showing one.
        assert_eq!(app.take_request(), None);
    }

    #[test]
    fn re_reading_names_the_contract_to_read_under() {
        let mut app = app();
        app.reread_answer(Contract::Lines);
        assert_eq!(
            app.take_request(),
            Some(Request::Answer {
                contract: Some(Contract::Lines)
            })
        );
    }

    /// A fresh answer is read from its start, not from wherever the previous
    /// one happened to be scrolled to.
    #[test]
    fn a_new_answer_resets_the_selection() {
        let mut app = app();
        app.set_answer(Ok(Answer {
            contract: Contract::Lines,
            value: Some(AnswerValue::Lines(vec!["a".into(), "b".into()])),
            error: None,
            source: "…".into(),
        }));
        app.answer_select_next();
        assert_eq!(app.answer_selected, 1);

        app.set_answer(Ok(Answer {
            contract: Contract::Lines,
            value: Some(AnswerValue::Lines(vec!["c".into()])),
            error: None,
            source: "…".into(),
        }));
        assert_eq!(app.answer_selected, 0);
    }

    /// A failed fetch replaces the previous answer rather than leaving a stale
    /// one on screen under an error that contradicts it.
    #[test]
    fn a_failed_fetch_clears_the_previous_answer() {
        let mut app = app();
        app.set_answer(Ok(Answer {
            contract: Contract::Text,
            value: Some(AnswerValue::Text("hello".into())),
            error: None,
            source: "…".into(),
        }));
        app.set_answer(Err("no typed turn to answer".into()));
        assert!(app.answer.is_none());
        assert_eq!(app.answer_error.as_deref(), Some("no typed turn to answer"));
    }

    /// `e` in the answer view opens what the agent named, so the location has
    /// to resolve against the Workspace exactly as the Files view's does.
    #[test]
    fn a_files_answer_resolves_its_selection_against_the_workspace() {
        let mut app = app();
        app.set_workspace_root(PathBuf::from("/work"));
        app.view = View::Answer;
        app.set_answer(Ok(Answer {
            contract: Contract::Files,
            value: Some(AnswerValue::Files(vec![FileLocation {
                path: PathBuf::from("src/auth.rs"),
                line: Some(12),
                column: None,
                description: String::new(),
            }])),
            error: None,
            source: "…".into(),
        }));
        assert_eq!(
            app.selected_file_path(),
            Some(PathBuf::from("/work/src/auth.rs"))
        );
        assert_eq!(app.copy_text().as_deref(), Some("src/auth.rs:12"));
    }

    /// The driva view is a view like any other: reachable whether or not there
    /// is a sandbox to describe, and toggled off by the same key.
    #[test]
    fn the_driva_view_toggles_whether_or_not_a_policy_has_been_recorded() {
        let mut app = app();
        assert_eq!(app.launch.driva, None);

        assert_eq!(app.view, View::Events);
        app.toggle_view(View::Driva);
        assert_eq!(app.view, View::Driva);
        app.toggle_view(View::Driva);
        assert_eq!(app.view, View::Events);
    }

    #[test]
    fn preview_toggles_independently_of_other_view_state() {
        let mut app = app();
        assert!(!app.show_preview);
        app.toggle_preview();
        assert!(app.show_preview);
        app.toggle_preview();
        assert!(!app.show_preview);
    }

    #[test]
    fn fullscreen_preview_toggles_the_view_and_is_independent_of_the_side_panel() {
        let mut app = app();
        assert_eq!(app.view, View::Events);
        app.toggle_view(View::Preview);
        assert_eq!(app.view, View::Preview);
        // The side-panel flag (bound to lowercase `p`) is a separate toggle;
        // the full-screen shortcut (`P`) does not touch it.
        assert!(!app.show_preview);
        app.toggle_view(View::Preview);
        assert_eq!(app.view, View::Events);
    }

    #[test]
    fn files_view_opens_with_a_togglable_entry_preview() {
        let mut app = app();
        assert!(!app.show_preview);

        app.toggle_files();
        assert_eq!(app.view, View::Files);
        assert!(app.show_preview);

        app.toggle_preview();
        assert!(!app.show_preview);
        app.toggle_files();
        assert_eq!(app.view, View::Events);
    }

    #[test]
    fn focus_toggles_and_a_typed_message_is_taken_for_sending() {
        let mut app = app();
        assert_eq!(app.focus, Focus::List);
        app.enter_input();
        assert_eq!(app.focus, Focus::Input);

        app.composer.char('h');
        app.composer.char('i');
        assert_eq!(app.composer.text, "hi");
        assert_eq!(app.take_message(), Some("hi".into()));
        assert!(app.composer.text.is_empty());
        assert_eq!(app.take_message(), None);
    }

    #[test]
    fn copy_text_is_the_selected_entrys_full_presented_detail() {
        let mut app = app();
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        let text = app.copy_text().expect("an entry is selected");
        assert!(text.contains("cargo test"));
        assert!(text.contains("24 passed"));
    }

    #[test]
    fn copy_text_falls_back_to_the_summary_when_there_is_no_detail() {
        let mut app = app();
        app.timeline.show_minor = true;
        app.push_event(AgentEvent::TurnStarted);
        app.select_first();
        let text = app.copy_text().expect("a minor entry is selected");
        assert_eq!(text, AgentEvent::TurnStarted.summary());
    }

    #[test]
    fn copy_text_is_none_for_views_with_no_discrete_entry() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage { text: "hi".into() });
        for view in [View::Log, View::Transcript, View::Driva] {
            app.view = view;
            assert!(app.copy_text().is_none(), "{view:?}");
        }
    }

    #[test]
    fn copy_text_in_the_raw_view_is_the_selected_wire_line() {
        use styra_server::{Direction, RawLine};
        let mut app = app();
        app.push_raw(RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: r#"{"type":"turn.started"}"#.into(),
        });
        app.view = View::Raw;
        assert_eq!(
            app.copy_text().as_deref(),
            Some(r#"{"type":"turn.started"}"#)
        );
    }
}
