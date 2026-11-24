//! Application state: session status, which view is up, the raw/log/files
//! panels, and the pending request for the event loop.
//!
//! Anything with enough state of its own to be worth a module has one, and is
//! carried here as a field rather than spread across this struct: the event
//! list is a [`Timeline`], the sandbox policy a [`Launch`], and likewise
//! [`Launcher`] and [`Composer`]. What stays here is what belongs to
//! no single one of them, and the few methods that have to join two — a
//! selection move also resets the preview scroll, an edit to the launch policy
//! is refused while an interaction is running.
//!
//! This module is pure state and transitions — no terminal, no threads, no IO —
//! so the whole interaction model is unit-testable. [`crate::presentation`] renders it and
//! `main` feeds it input and session updates.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
#[cfg(test)]
use styra_protocol::agent::PROVIDERS;
use styra_protocol::agent::{Provider, Selection};
use styra_protocol::event::{AgentEvent, DetailBlock, PresentationMode};
use styra_protocol::Contract;
use styra_protocol::{InteractionEnd, LogEntry, QuotaEvent, QuotaStatus};
use styra_ui::event_list::ListAnchor;
use styra_ui::scroll::Scroll;
use styra_ui::search::Search;

use crate::activity::{Activity, RateLimit, Status, StopReason};
use crate::answer::AnswerView;
use crate::audio::Recorded;
use crate::branch::BranchPrompt;
use crate::composer::Composer;
use crate::entry_log::EntryLog;
use crate::files::{self, FilesView};
use crate::help::Help;
use crate::ingest;
use crate::insert::Prompt;
use crate::interactions::LiveInteractions;
use crate::launch::{self, Launch};
use crate::launcher::{Choice, Launcher};
use crate::link_menu::LinkMenu;
use crate::notices::Notices;
use crate::outbox::Outbox;
use crate::overview::Overview;
use crate::picker::TemplatePicker;
use crate::preview::{self, Preview};
use crate::raw::{ProviderRawView, RawView};
use crate::tag_picker::TagPicker;
use crate::tail::Tail;
use crate::timeline::{Entry, Timeline};
use crate::workspace::Location;

/// Which region receives keys, like vim's normal/insert split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Navigate and fold the event list.
    List,
    /// Type into the message box.
    Input,
}

/// Whether pretty Markdown shows a link's destination beside its title.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkDisplay {
    /// Keep replies compact: show the label alone.
    #[default]
    Compact,
    /// Include the destination after the label.
    Full,
}

/// The link selected while walking the rendered conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkHighlight {
    pub entry: usize,
    pub link: usize,
}

impl LinkDisplay {
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Compact => Self::Full,
            Self::Full => Self::Compact,
        };
    }
}

/// What the main region shows: the decoded event list, the raw wire stream,
/// the diagnostic log, the rendered transcript, server details and Driva policy,
/// the selected entry's full-screen preview, or the overview of every active
/// interaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Events,
    Raw,
    Messages,
    Log,
    /// The plan-quota readings the server has seen, newest last.
    Quota,
    /// The processes running in the interaction's sandbox, refreshed while
    /// the view is open.
    Processes,
    Transcript,
    Driva,
    Files,
    /// The last turn's typed answer, rendered as the shape it was asked for.
    Answer,
    Preview,
    /// Every interaction still taking turns, as a grid of tiles; see
    /// [`Overview`].
    Overview,
}

/// Which page of the details view is visible.
///
/// The sandbox account is intentionally separate from the Workspace and
/// interaction metadata: it is long enough to need the whole panel, while the
/// policy controls belong with the details they change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailsTab {
    #[default]
    Details,
    Sandbox,
}

impl DetailsTab {
    pub fn other(self) -> Self {
        match self {
            Self::Details => Self::Sandbox,
            Self::Sandbox => Self::Details,
        }
    }
}

/// What a launch is asked for beyond the agent selection: the sandbox policy
/// inputs the server resolves into a concrete Driva request.
///
/// Defined on the wire rather than here ([`styra_protocol::LaunchPolicy`]),
/// because both layers of it are the server's to resolve: the Workspace's
/// standing policy and this interaction's own are merged there, once, for every
/// launch path. The client holds the two apart (in [`Launch`]) only so the driva
/// view can say where each grant comes from and edit one without the other.
///
/// Re-exported because the modules that persist and send a policy
/// ([`crate::preferences`], [`crate::session`]) want the type without wanting
/// the state machine around it.
pub use styra_protocol::LaunchPolicy;

/// The agent, model, and reasoning effort a session is on, as the status line
/// names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchLabel {
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Whether the agent confirmed the model, rather than it only being the
    /// launch request.
    pub model_reported: bool,
    /// Whether the agent confirmed the reasoning effort.
    pub effort_reported: bool,
}

/// The complete UI state.
pub struct App {
    /// The event list and the operator's place in it; see [`Timeline`].
    pub timeline: Timeline,
    pub focus: Focus,
    pub view: View,
    /// The open page within the details view.
    pub details_tab: DetailsTab,
    /// Scroll position for the Details page's Workspace, interaction, and
    /// sandbox-summary document. The Sandbox page keeps its own position in
    /// [`Launch::scroll`].
    pub details_scroll: Scroll,
    /// The full-screen keyboard reference; see [`Help`].
    pub help: Help,
    /// The message being typed and the ones already sent; see [`Composer`].
    pub composer: Composer,
    /// When the message box was last handed to the server; see
    /// [`crate::session::save_composer`].
    pub composer_saved_at: std::time::Instant,
    /// Server-wide interaction navigation shown above the event timeline.
    pub interactions: LiveInteractions,
    /// The overview's cursor over that same snapshot.
    pub overview: Overview,
    /// Messages on their way out and the shape the next one asks for; see
    /// [`Outbox`].
    pub outbox: Outbox,
    /// The Interaction's status and the bookkeeping around it; see
    /// [`Activity`].
    pub activity: Activity,
    /// Recent actions Styra performed without a direct operator command; see
    /// [`Notices`].
    pub notices: Notices,
    /// The panel showing one entry in full, and how; see [`Preview`].
    pub preview: Preview,
    /// Whether pretty Markdown includes link destinations beside their labels.
    pub link_display: LinkDisplay,
    /// What the next session launches with: agent, model, reasoning effort.
    /// This is the choice for the current workspace, edited through [`Launcher`]
    /// while nothing is running. The terminal client only persists it as the
    /// standing default when the operator explicitly asks.
    pub selection: Selection,
    /// Confirmation flags for the title; model and effort live in `selection`.
    pub model_reported: bool,
    pub effort_reported: bool,
    /// The open launch picker, while the operator is choosing.
    pub launcher: Option<Launcher>,
    /// The Driva template chooser, including its server-loading state. Unlike
    /// the older nested picker loop this belongs to the root application loop,
    /// so session updates and rendering continue while templates are fetched.
    pub template_picker: Option<TemplatePicker>,
    /// Number of durable Workspace-policy edits accepted by the background
    /// worker but not yet acknowledged. Policy controls pause while nonzero so
    /// a second toggle cannot be derived from a stale server snapshot.
    pub workspace_launch_pending: usize,
    /// Interactions whose edits the server is describing, which the footer
    /// shows from whichever view the operator is in: the report takes a while,
    /// and its interaction may no longer be the one on screen.
    pub describing_edits: usize,
    /// A mount was added or removed while the interaction was idle, so its
    /// running sandbox no longer matches the policy. The event loop restarts it
    /// once any Workspace edit has been stored; see [`App::note_mount_change`].
    pub restart_for_mounts: bool,
    /// Models the operator has confirmed in the launch picker, most recent
    /// first. Loaded from the saved preferences when the loop starts and
    /// written back as the picker is used, so the ordering of the model
    /// column outlives the client.
    pub recent_models: Vec<String>,
    /// Which Workspace this screen is showing and where it is; see
    /// [`Location`].
    pub workspace: Location,
    /// The Git checkout path being entered for the current Workspace from the
    /// details view. An empty value deliberately clears the association.
    pub git_repository_prompt: Option<String>,
    /// The current interaction's display name being edited.
    pub rename_prompt: Option<String>,
    pub integration_prompt: Option<String>,
    /// Commit-message drafts kept in this client, keyed by interaction.
    integration_drafts: std::collections::HashMap<String, String>,
    /// The last rebase this client started, and the interaction it is of.
    /// The server locks that interaction while it runs; this only follows it.
    pub rebase_status: Option<(String, styra_protocol::RebaseStatus)>,
    pub pending_integration: Option<String>,
    /// The tag editor opened from the main view or live-interactions navigator.
    pub tag_picker: Option<TagPicker>,
    /// The event list's `/` search. Not carried across screens by
    /// [`App::take_operator_state`]: it is asked of the entries in front of
    /// the operator, and the next screen holds different ones.
    pub search: Search,
    pub session_id: String,
    /// Session whose linked checkout a pending new Session shares with Enter
    /// or branches from with Ctrl-Enter.
    /// Set by `n`; the server resolves and validates the association rather
    /// than accepting a client-supplied path.
    pub checkout_from: Option<String>,
    /// The branch the Workspace's own checkout is on, which a new Session
    /// starts from unless `checkout_from` lends a linked one. Read once when
    /// the blank screen opens rather than on every frame.
    pub workspace_branch: Option<String>,
    /// The local branch a pending new Session's checkout is to start from,
    /// chosen with alt-b. When set, the launch makes a fresh checkout of
    /// that branch whichever way the message is sent, rather than sharing
    /// or branching what `checkout_from` lends.
    pub base_branch: Option<String>,
    /// The branch list open over the screen: the base of a pending new
    /// Session (alt-b before its first message), or the branch to retarget
    /// the current one onto (alt-b after).
    pub branch_picker: Option<crate::branch_picker::BranchPicker>,
    /// The sandbox policy: both of its layers, the sandbox they resolve to, and
    /// which layer the driva view's keys are editing. See [`Launch`].
    pub launch: Launch,
    /// The verbatim wire interaction and the place in it; see [`RawView`].
    pub raw: RawView,
    /// Incoming wire messages, retained in arrival order for this interaction.
    pub messages: RawView,
    /// The provider's own persisted session JSONL, loaded on demand from the
    /// raw view. `true` selects it instead of Styra's wire capture.
    pub provider_raw: Option<ProviderRawView>,
    pub provider_raw_open: bool,
    /// Diagnostic log entries, in occurrence order; see [`Tail`].
    pub log: Tail<LogEntry>,
    /// Plan-quota readings, oldest first. Filled by asking the server, which
    /// holds the log — quota belongs to the account, so this is every
    /// interaction's readings, not just this session's.
    pub quota: Tail<QuotaEvent>,
    /// The quota readings this client has already raised a notice for; see
    /// [`App::note_quota`].
    pub quota_noticed: HashSet<QuotaNotice>,
    /// The last listing of the current interaction's sandbox processes.
    pub processes: crate::processes::ProcessView,
    /// Whether that listing is open in the bottom half below the event list,
    /// in the entry log's place.
    pub processes_pane: bool,
    /// Whether the server will pick this interaction back up when the plan
    /// window that refused it turns over.
    ///
    /// The server's answer, not this client's opinion: the waiting is its, the
    /// setting is stored with the Session, and this is read from the
    /// interaction summary whenever one is attached. Changed only by asking
    /// (see [`Request::SetAutoRetry`]), so what is shown is what was accepted.
    pub auto_retry: bool,
    /// Whether the server commits what each of this interaction's turns
    /// leaves in its checkout as the interaction goes idle. The server's
    /// answer in the same way [`Self::auto_retry`] is, read from the
    /// interaction summary and changed only by asking (see
    /// [`Request::SetAutoCommit`]).
    pub auto_commit: bool,
    /// How far down the rendered transcript is scrolled; 0 shows its start.
    /// Unlike the raw/log views, the transcript reads as a document from the
    /// beginning rather than anchoring to the tail.
    pub transcript: Scroll,
    /// The diff the diff pane shows. Kept after the pane is closed, but read
    /// afresh each time it is opened.
    pub checkout_diff: Option<styra_diff::CheckoutDiff>,
    pub diff_branch_picker: Option<crate::checkout_diff::BranchPicker>,
    /// Temporarily use the preview pane for the diff without changing the
    /// operator's preview choice.
    pub diff_preview: bool,
    /// While a side diff is open, whether navigation belongs to the interaction.
    pub diff_interaction_focus: bool,
    /// Whether the side diff shows the selected turn's diff rather than the
    /// checkout's; see [`crate::turn_diff`].
    pub diff_turn: bool,
    /// Highlight the selected turn within the checkout diff.
    pub diff_turn_highlight: bool,
    /// Whether the per-file view lists its files in a strip under the patch
    /// rather than in a column to its left.
    pub diff_files_below: bool,
    /// Whether the diff pane takes the whole interaction view, hiding the
    /// log; a newly opened pane starts under the log again.
    pub diff_maximized: bool,
    /// The turn diff the side diff shows while [`Self::diff_turn`] is set.
    pub turn_diff: Option<crate::turn_diff::TurnDiffPanel>,
    /// What the message being written is about, when it was started from the
    /// diff: the file it was on, and the line the line cursor was on. Shown
    /// under the message box, and put under the message when it is sent.
    pub message_source: Option<String>,
    /// The entry-log pane below the event list: whether it is open, which of
    /// the two windows has the navigation keys, and where it is; see
    /// [`EntryLog`]. It is deliberately a pane state, not a [`View`]: it and
    /// the event list are on screen together, and `Tab` moves between them.
    pub entry_log: EntryLog,
    /// Selected file in the Files view and whether it aggregates the session;
    /// see [`FilesView`].
    pub files: FilesView,
    /// The typed answer last fetched for this session, and the selection
    /// within it; see [`AnswerView`].
    pub answer: AnswerView,
    /// The Markdown link selected while walking the conversation with `f`.
    pub link_highlight: Option<LinkHighlight>,
    /// Which of the branches drawn beneath a `branched to` marker carries the
    /// cursor, as the marker's entry index and the branch's place in
    /// [`crate::interactions::Interactions::branch_tree`]. Only meant while
    /// that entry stays selected; see [`Self::selected_branch`].
    branch_cursor: Option<(usize, usize)>,
    /// Preview visibility to restore when leaving link navigation.
    link_preview_open: Option<bool>,
    /// The actions offered on the highlighted link, while that menu is open.
    pub link_menu: Option<LinkMenu>,
    /// The current window's actions, searchable, while Space has them open.
    pub action_menu: Option<crate::action_menu::ActionMenu>,
    /// The Workspaces with live work, searchable, while `v` has them open.
    pub workspace_jump: Option<crate::workspace_jump::WorkspaceJump>,
    /// The modal choice of how the selected entry seeds a new Session.
    pub branch_prompt: Option<BranchPrompt>,
    /// The linked Session's log, while the preview is on a `branch` marker;
    /// see [`crate::branch_log`].
    pub branch_log: Option<crate::branch_log::BranchLog>,
    /// What the turn the side diff's turn diff is about changed according to
    /// its snapshots; see [`crate::turn_snapshot`].
    pub turn_snapshot: Option<crate::turn_snapshot::TurnSnapshot>,
    /// The microphone capture that is running, if one is; see [`Recorded`].
    /// While it is set the message box is the level meter, and the keys that
    /// would type into the box finish, cancel, or boost the recording instead:
    /// there is no text being edited to compete with them.
    pub recording: Option<Recorded>,
    /// What the client is blocked doing, while a synchronous call the operator
    /// started is running — creating a Git workspace, above all. While it is
    /// set the input is hidden and progress appears in the bottom status box.
    /// No key is read until the call returns.
    pub busy: Option<String>,
    /// The open "insert a path" prompt, while the operator is using it; see
    /// [`crate::insert`]. Held here rather than in [`Composer`] because its
    /// second question is about the sandbox, not about the message.
    pub insert: Option<Prompt>,
    /// Ordered effects the operator has asked the event loop to perform.
    /// A queue is essential here: handling one effect can produce another
    /// (template discovery followed by a Workspace edit), and no later key is
    /// allowed to overwrite that follow-up.
    pub requests: VecDeque<Request>,
}

/// Something the operator asked for that [`App`] cannot carry out itself,
/// because it means leaving this screen or this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Rebase,
    Retarget(String),
    Integrate(String),
    GenerateIntegrationMessage,
    /// Fill the rename prompt with a name an errand suggests.
    GenerateSessionName,
    /// Have each file of the checkout diff on screen described, in an edit
    /// report (see [`crate::edit_report`]), or only the file at this path,
    /// keeping the report's other descriptions.
    GenerateEditReport(Option<String>),
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
    /// Move backward through explicitly entered interactions.
    JumpBack,
    /// Move forward through explicitly entered interactions.
    JumpForward,
    /// Choose an explicitly entered interaction from a fuzzy history list.
    InteractionHistory,
    /// Open the server's live interactions above the main event timeline.
    Interactions,
    /// Choose among the Workspaces with live work — or, when `listed`, with
    /// any Interaction the navigator lists — to open the live interactions at
    /// the first of that Workspace's.
    WorkspaceJump {
        listed: bool,
    },
    /// Show every running and idle interaction as a grid, with a fresh
    /// listing behind it.
    Overview,
    /// Make the interaction with this id current, as choosing its tile in the
    /// overview does, and return to its event list.
    ShowInteraction(String),
    /// Make the interaction with this id current while staying on the
    /// overview, and open its message box, as `i` on a tile does.
    MessageInteraction(String),
    /// Choose a Session by the worktree it works in.
    Worktrees,
    /// Make a newly idle interaction current when one is unseen; otherwise,
    /// make the next live interaction current in navigator order.
    NextLiveInteraction,
    /// Make the next actively working interaction current, skipping idle
    /// interactions that are waiting for the operator.
    NextWorkingInteraction,
    /// Return to the blank start screen without stopping the current interaction.
    NewSession,
    /// Give a Session a Git branch and linked workspace of its own: `Ctrl+W` asks
    /// it for the Session being viewed, and `Ctrl-Enter` for the one its
    /// `message` is about to start or continue. Either way the checkout is
    /// made before the agent runs in it, so both wait behind the same notice.
    CreateWorktree {
        message: Option<String>,
    },
    /// Open the selected entry in the Files view in the configured opener.
    EditFile,
    /// Open the live Session's sandbox shell in a terminal window of its own.
    /// The window is the operator's to configure, so the event loop — which
    /// holds the configuration — runs it rather than the key handler.
    OpenShell,
    /// Launch a project command from the Workspace directory in a terminal.
    /// The event loop runs it using the configured terminal opener.
    ProjectAction(styra_protocol::ProjectAction),
    /// Open a host shell in the directory the interaction is working in, in a
    /// terminal window of its own. Like [`Request::OpenShell`] the window is
    /// configured, so the event loop runs it.
    OpenDirectory,
    /// Read the checkout diff into the preview pane. Reading it runs `git`,
    /// so the event loop does it rather than the key handler.
    ShowDiff(String),
    /// Open the diff at the selected file or file reference, when it occurs
    /// in the checkout's changes.
    ShowDiffAt {
        id: String,
        path: PathBuf,
        line: Option<u32>,
    },
    /// Open a Markdown link's resolved path in the configured editor.
    OpenPath(PathBuf),
    /// Open the file at the top of the diff view in the configured editor, at
    /// the line there when the diff says which.
    OpenPathAt(PathBuf, Option<u32>),
    /// Open a web address in the configured browser.
    OpenUrl(String),
    /// Choose which Driva templates the next interaction launches with. The
    /// list of them lives on the server, so the event loop fetches it and runs
    /// the picker.
    Templates,
    /// Send an edit operation to the server which owns the Workspace policy.
    /// The UI never applies this optimistically; it adopts the policy returned
    /// by the server after the edit is durably stored.
    ChangeWorkspaceLaunch {
        change: styra_protocol::WorkspaceLaunchChange,
        clear_interaction: bool,
    },
    /// Tell the server the live interaction has been switched onto
    /// [`App::selection`], so the change lands now and outlives this client.
    ApplySelection,
    /// Move this Session's history onto another agent, and open the result.
    ///
    /// A stopped Session's transcript is written in its agent's own native
    /// format, so choosing a different agent for it is a conversion rather
    /// than a setting: the server copies the history into the new agent's
    /// format as a sibling Session, which is the one the operator continues in.
    ConvertProvider(Selection),
    /// Fetch the server's plan-quota log, which is server-wide and lives only
    /// in the daemon's memory, so there is nothing to read locally.
    Quota,
    /// List the processes in the current interaction's sandbox, which only
    /// the server, which launched it, can see.
    Processes,
    /// Ask the server to keep this interaction at it after a rate limit, or to
    /// stop doing so. The waiting is the server's — it outlives this client
    /// and the interaction the limit stopped — so this only asks.
    SetAutoRetry(bool),
    /// Ask the server to commit each turn's work as this interaction goes
    /// idle, or to stop doing so. Stored with the Session by the server.
    SetAutoCommit(bool),
    /// Fetch the last turn's typed answer from the server, which parses it.
    /// `contract` names a shape to read the reply under instead of the one the
    /// turn was sent with, which is how a mis-shaped answer is recovered
    /// without asking the agent again.
    Answer {
        contract: Option<Contract>,
    },
}

/// The parts of a screen that belong to the operator rather than to the
/// Interaction it is showing.
///
/// Switching which Interaction is current rebuilds the whole [`App`] from the
/// server, because the timeline, status and wire history are the Interaction's
/// and only the server has them. What must not be rebuilt is everything the
/// operator themselves put on the screen: the navigator they are moving
/// through, the display choices they made, and the message they have half
/// written. Those are taken out of the old screen and adopted by the new one.
///
/// Deliberately not carried, because they are the Interaction's and rebuilding
/// them is the point: the timeline, status, usage, answer, wire history — and
/// the diagnostic log, which is partly fed from the server
/// ([`styra_protocol::InteractionUpdate::Log`]), so carrying it across a re-attach
/// would show the same entries twice.
#[derive(Default)]
pub struct OperatorState {
    interactions: LiveInteractions,
    integration_drafts: std::collections::HashMap<String, String>,
    /// The event list's own filter, which is a display choice like the rest;
    /// a caller that wants a different one sets it after adopting.
    all_events: bool,
    /// The preview panel's own display choices; see
    /// [`Preview::choices`](crate::preview::Preview::choices).
    preview: preview::Choices,
    opened: Opened,
    link_display: LinkDisplay,
    recent_models: Vec<String>,
    /// The shape the next reply is to come back in. The message itself is
    /// not carried: it belongs to the Session it was being written to, and the
    /// server keeps it there (see [`Composer`]).
    contract: Option<Contract>,
    file_show_all: bool,
    /// The plan-quota readings. Unlike the diagnostic log these are not the
    /// Interaction's: they measure the account, so the same readings are true
    /// of whichever screen this client switches to, and re-fetching them on
    /// every switch would only blank the footer's warning until it landed.
    quota: Tail<QuotaEvent>,
    /// Which readings have had their notice, so an interaction re-entered —
    /// which replays its updates, announcements included — does not raise
    /// the same warning again.
    quota_noticed: HashSet<QuotaNotice>,
    /// Whether the interaction, rather than the side diff, had the keys, so
    /// stepping to another interaction with the pane open does not hand them
    /// to its diff.
    diff_interaction_focus: bool,
}

/// What the operator had open in place of the plain event list. Moving to
/// another Interaction keeps it open, showing that Interaction's instead: the
/// operator is comparing Interactions, not leaving the view.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Opened {
    #[default]
    Events,
    Preview,
    DiffPane,
}

/// One announced quota reading, as told apart from another: where it was
/// seen, when, and about which window.
pub type QuotaNotice = (String, u64, String);

fn quota_notice(reading: &QuotaEvent) -> QuotaNotice {
    (
        reading.session_id.clone(),
        reading.at_ms,
        reading.window.clone(),
    )
}

impl App {
    pub fn new(selection: Selection, session_id: impl Into<String>) -> Self {
        Self {
            timeline: Timeline::default(),
            focus: Focus::List,
            view: View::Events,
            details_tab: DetailsTab::default(),
            details_scroll: Scroll::default(),
            help: Help::default(),
            composer: Composer::default(),
            composer_saved_at: std::time::Instant::now(),
            interactions: LiveInteractions::default(),
            overview: Overview::default(),
            outbox: Outbox::default(),
            activity: Activity::default(),
            notices: Notices::default(),
            preview: Preview::default(),
            link_display: LinkDisplay::default(),
            selection,
            model_reported: false,
            effort_reported: false,
            launcher: None,
            template_picker: None,
            workspace_launch_pending: 0,
            describing_edits: 0,
            restart_for_mounts: false,
            recent_models: Vec::new(),
            workspace: Location::default(),
            git_repository_prompt: None,
            rename_prompt: None,
            integration_prompt: None,
            integration_drafts: std::collections::HashMap::new(),
            rebase_status: None,
            pending_integration: None,
            tag_picker: None,
            search: Search::default(),
            session_id: session_id.into(),
            checkout_from: None,
            workspace_branch: None,
            base_branch: None,
            branch_picker: None,
            launch: Launch::default(),
            raw: RawView::default(),
            messages: RawView::default(),
            provider_raw: None,
            provider_raw_open: false,
            log: Tail::default(),
            quota: Tail::default(),
            quota_noticed: HashSet::new(),
            processes: Default::default(),
            processes_pane: false,
            auto_retry: false,
            auto_commit: false,
            transcript: Scroll::default(),
            checkout_diff: None,
            diff_branch_picker: None,
            message_source: None,
            diff_preview: false,
            diff_turn: false,
            diff_turn_highlight: false,
            diff_files_below: false,
            diff_maximized: false,
            turn_diff: None,
            diff_interaction_focus: false,
            entry_log: EntryLog::default(),
            files: FilesView::default(),
            answer: AnswerView::default(),
            link_highlight: None,
            branch_cursor: None,
            link_preview_open: None,
            link_menu: None,
            action_menu: None,
            workspace_jump: None,
            branch_prompt: None,
            branch_log: None,
            turn_snapshot: None,
            recording: None,
            busy: None,
            insert: None,
            requests: VecDeque::new(),
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
        app.activity.status = Status::Pending;
        app
    }

    /// Take the operator-owned state off this screen, leaving the rest of it
    /// intact; see [`OperatorState`]. The screen is expected to be replaced
    /// straight afterwards, so what is left behind is the Interaction's own
    /// state, which the replacement rebuilds from the server.
    pub fn take_operator_state(&mut self) -> OperatorState {
        self.close_integration_prompt();
        self.clear_link_highlight();
        let opened = if self.view == View::Preview {
            Opened::Preview
        } else if !self.diff_visible() {
            Opened::Events
        } else {
            Opened::DiffPane
        };
        OperatorState {
            opened,
            interactions: std::mem::take(&mut self.interactions),
            integration_drafts: std::mem::take(&mut self.integration_drafts),
            all_events: self.timeline.all_events,
            preview: self.preview.choices(),
            link_display: self.link_display,
            recent_models: std::mem::take(&mut self.recent_models),
            contract: self.outbox.take_contract(),
            file_show_all: self.files.shows_all(),
            quota: std::mem::take(&mut self.quota),
            quota_noticed: std::mem::take(&mut self.quota_noticed),
            diff_interaction_focus: self.diff_interaction_focus,
        }
    }

    /// Put operator-owned state onto a screen freshly attached from the server.
    ///
    /// The counterpart to [`App::take_operator_state`], and the only place that
    /// says which fields outlive a screen — so a new field is carried, or
    /// deliberately not, in one place rather than once per switching path.
    pub fn adopt(&mut self, state: OperatorState) {
        let mut interactions = state.interactions;
        interactions.adopt_names_from(&self.interactions);
        self.interactions = interactions;
        self.integration_drafts = state.integration_drafts;
        self.timeline.all_events = state.all_events;
        self.preview.adopt(state.preview);
        self.link_display = state.link_display;
        self.recent_models = state.recent_models;
        self.outbox.set_contract(state.contract);
        self.files.set_scope(state.file_show_all);
        self.quota = state.quota;
        self.quota_noticed.extend(state.quota_noticed);
        // The diff is read from this Interaction's checkout, which only the
        // event loop can reach; one without a checkout has none to show.
        let has_checkout = self
            .interactions
            .current(&self.session_id)
            .is_some_and(|interaction| interaction.checkout.is_some());
        match state.opened {
            Opened::Events => {}
            Opened::Preview => self.set_view(View::Preview),
            Opened::DiffPane => {
                // Keep the pane enabled even when this interaction cannot
                // provide a diff, so the next interaction tries again.
                self.diff_preview = true;
                // Whichever side had the keys keeps them; browsing the live
                // list keeps its keys while the new interaction's diff is
                // loaded.
                self.diff_interaction_focus =
                    state.diff_interaction_focus || self.interactions.open;
                if has_checkout {
                    self.ask(Request::ShowDiff(self.session_id.clone()));
                }
            }
        }
    }

    pub fn open_integration_prompt(&mut self) {
        self.integration_prompt = Some(
            self.integration_drafts
                .get(&self.session_id)
                .cloned()
                .unwrap_or_else(|| {
                    let name = self.interactions.name(&self.session_id).unwrap_or_default();
                    match self.workspace.name.as_deref() {
                        Some(workspace) => format!("{workspace}: {name}"),
                        None => name.to_owned(),
                    }
                }),
        );
    }

    pub fn close_integration_prompt(&mut self) {
        if let Some(message) = self.integration_prompt.take() {
            self.integration_drafts
                .insert(self.session_id.clone(), message);
        }
    }

    /// Point this screen at `workspace`: which one it is, what to call it,
    /// and the standing policy every launch in it is layered onto.
    ///
    /// The three moved together at three call sites that each carried a
    /// different subset of them, so they are stated here once. Where the
    /// agent is *working* is deliberately not included: a live interaction may
    /// have been told to work somewhere other than the Workspace root, and
    /// this is also called to refresh a screen already showing it.
    pub fn show_workspace(&mut self, workspace: &styra_protocol::WorkspaceSummary) {
        self.workspace.show(workspace);
        self.launch.set_workspace(workspace.launch.clone());
    }

    /// Whether the picker is reachable.
    ///
    /// Whenever no agent process is running — before launch, and again once one
    /// has stopped or ended — the whole launch is a choice about the *next*
    /// one, exactly as the sandbox policy is (see [`crate::launch::editable`]).
    /// Both reopen together: a stopped session's next message resumes it, and
    /// that resume launches under whatever is chosen here.
    ///
    /// While a process is live only its model can move: idle Codex and Claude
    /// threads accept a model change before their next turn (Codex additionally
    /// accepts an effort change).
    pub fn can_configure_launch(&self) -> bool {
        self.can_edit_launch()
            || (self.activity.status.is_idle()
                && matches!(self.selection.provider, Provider::Codex | Provider::Claude))
    }

    /// Whether an agent process is up. This is what holds the session's agent
    /// fixed: the process *is* the agent, and it is the one thing a running
    /// interaction cannot be talked out of — moving the session onto another
    /// agent means ending this one and converting its transcript.
    fn agent_is_live(&self) -> bool {
        !self.can_edit_launch()
    }

    /// Switch between compact link labels and labels with their destinations.
    pub fn toggle_link_display(&mut self) {
        self.link_display.toggle();
        self.preview.scroll.reset();
    }

    /// Open the launch picker on the current selection, if anything can still
    /// be chosen.
    pub fn open_launcher(&mut self) {
        if self.branch_prompt.is_some() || self.can_configure_launch() {
            // While a process is live the agent is fixed, so the picker shows
            // it without ever letting the cursor onto it.
            self.launcher = Some(Launcher::from_selection(
                self.editor_selection(),
                &self.recent_models,
                self.branch_prompt.is_none() && self.agent_is_live(),
            ));
        }
    }

    /// Adopt what the picker describes, and close it.
    ///
    /// Before launch nothing is sent: the operator's first message still starts
    /// the agent. On a live session the change is asked of the server right
    /// away ([`Request::ApplySelection`]) rather than riding on the next
    /// message, so the model the status line names is the model that is loaded.
    /// On a stopped one nothing is sent either: there is no process to tell, and
    /// the resume the next message triggers carries the selection.
    pub fn confirm_launcher(&mut self) {
        let Some(launcher) = self.launcher.take() else {
            return;
        };
        self.confirm_selection(launcher.selection());
    }

    /// Open a provider picker without changing the current selection.
    pub fn open_provider_picker(&mut self) {
        if self.branch_prompt.is_none() && self.agent_is_live() {
            return self.show_warning("stop the interaction before switching provider");
        }
        self.open_choice_picker(Choice::Provider);
    }

    pub fn open_model_picker(&mut self) {
        if self.branch_prompt.is_none() && !self.can_configure_launch() {
            return self.show_warning("wait for the turn to finish before changing model");
        }
        self.open_choice_picker(Choice::ModelEffort);
    }

    fn open_choice_picker(&mut self, choice: Choice) {
        self.launcher = Some(Launcher::for_choice(
            self.editor_selection(),
            choice,
            self.branch_prompt.is_none() && self.agent_is_live(),
        ));
    }

    fn confirm_selection(&mut self, mut selection: Selection) {
        if self.branch_prompt.is_some() {
            self.note_recent_model(&selection.model);
            self.branch_prompt.as_mut().unwrap().selection = selection;
            return;
        }
        let live = self.agent_is_live();
        if selection.provider != self.selection.provider {
            // The picker never gives a live session's agent column the keys, so
            // this is not a choice it can offer — but it stays the answer for
            // any other way of asking.
            if live {
                return self.show_warning("stop the interaction before switching provider");
            }
            // A stopped Session's history is in its old agent's native format,
            // so moving it takes a conversion; a Session that was never
            // launched has no history and is simply set to the new agent.
            if !self.session_id.is_empty() {
                self.note_recent_model(&selection.model);
                self.ask(Request::ConvertProvider(selection));
                return;
            }
        }
        if live
            && selection.provider == Provider::Claude
            && selection.effort != self.selection.effort
        {
            selection.effort = self.selection.effort;
            self.show_warning(
                "Claude Code can change model between turns; effort remains session-wide",
            );
        }
        let apply = live && selection != self.selection;
        self.note_recent_model(&selection.model);
        self.set_selection(selection);
        if apply {
            self.ask(Request::ApplySelection);
        }
    }

    /// Record a model as the most recently selected one, so the picker lists
    /// it first the next time it opens. The list is capped: past a certain
    /// depth it stops being a shortlist and starts being the catalog again.
    fn note_recent_model(&mut self, model: &str) {
        styra_server::model_picker::note(&mut self.recent_models, model);
    }

    /// Close the picker, leaving the selection as it was.
    pub fn cancel_launcher(&mut self) {
        self.launcher = None;
    }

    /// What the status line names: the agent, and the model and reasoning effort
    /// in use.
    ///
    /// `selection` is updated when the agent reports the model it resolved, so
    /// this is also the exact selection a reopened launcher starts from.
    pub fn launch_label(&self) -> LaunchLabel {
        // A model that takes no effort setting has none to name: the
        // selection still carries one, because a `Selection` always does, but
        // it is a placeholder nothing sends and showing it would claim the
        // session is running at a rung the model has never heard of.
        let effort = self
            .selection
            .provider
            .supports_effort(&self.selection.model)
            .then(|| self.selection.effort.as_str().to_owned());
        LaunchLabel {
            agent: self.selection.provider.as_str().to_owned(),
            model: Some(self.selection.model.clone()),
            effort,
            model_reported: self.model_reported,
            effort_reported: self.effort_reported,
        }
    }

    pub fn editor_selection(&self) -> &Selection {
        self.branch_prompt
            .as_ref()
            .map(|prompt| &prompt.selection)
            .unwrap_or(&self.selection)
    }

    pub fn editor_composer_mut(&mut self) -> &mut Composer {
        match self.branch_prompt.as_mut() {
            Some(prompt) => &mut prompt.composer,
            None => &mut self.composer,
        }
    }

    /// Record the launch choice so the status line names what an `Enter` would start.
    pub fn set_selection(&mut self, selection: Selection) {
        self.model_reported &= self.selection.model == selection.model;
        self.effort_reported &= self.selection.effort == selection.effort;
        self.selection = selection;
    }

    /// Tell the operator about an action Styra took on their behalf.
    pub fn show_action_message(&mut self, message: impl Into<String>) {
        self.notices.show(message);
    }

    pub fn show_error(&mut self, message: impl Into<String>) {
        self.notices
            .show_level(message, styra_protocol::LogLevel::Error);
    }

    pub fn show_warning(&mut self, message: impl Into<String>) {
        self.notices
            .show_level(message, styra_protocol::LogLevel::Warn);
    }

    /// Show a failed operation immediately and retain its full details in the log.
    /// Keep this separate from `push_log`, which also receives replayed history.
    pub fn report_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.show_error(message.clone());
        self.push_log(LogEntry::error(message));
    }

    /// Replace the message box's contents outright, used to restore a message
    /// that failed to launch so it isn't lost.
    pub fn set_input(&mut self, text: String) {
        self.composer.set(text);
    }

    /// Append a diagnostic log entry; see [`Tail::push`] for what it does to
    /// a view the operator has scrolled back through.
    pub fn push_log(&mut self, entry: LogEntry) {
        self.log.push(entry);
    }

    /// Take a quota reading the server judged worth announcing.
    ///
    /// It lands in three places on purpose: the quota view, so the reading is
    /// there to look up; the log, so it stays on the record for the rest of the
    /// session; and a notice, so an operator watching the event list learns
    /// their window is filling without having gone looking for it.
    ///
    /// The notice is raised once per reading: entering an interaction replays
    /// its announcements along with the rest of its history, and a warning
    /// already given is not news the second time.
    pub fn note_quota(&mut self, reading: QuotaEvent) {
        self.push_log(match reading.status {
            QuotaStatus::Exhausted => LogEntry::error(reading.describe()),
            _ => LogEntry::warn(reading.describe()),
        });
        if self.quota_noticed.insert(quota_notice(&reading)) {
            match reading.status {
                QuotaStatus::Exhausted => self.show_error(reading.describe()),
                _ => self.show_warning(reading.describe()),
            }
        }
        // A spent window is not just news about the plan: it is why this
        // interaction is about to stop working, and the status that follows
        // has to be able to say so.
        if reading.status == QuotaStatus::Exhausted {
            self.activity.note_refused(RateLimit::from(&reading));
        }
        // The view is otherwise filled wholesale by asking the server; an
        // announced reading is appended so it shows without a round trip.
        self.quota.push(reading);
    }

    /// Keep incoming traffic independently of the decoded conversation.
    pub fn push_wire(&mut self, line: styra_protocol::RawLine) {
        if line.direction == styra_protocol::Direction::FromAgent {
            self.messages.push(line.clone());
        }
        self.raw.push(line);
    }

    /// Toggle the raw wire view on, or back to the event list. Entering it
    /// focuses the wire line behind the currently selected entry (or the
    /// tail, while the list is following it, or if no line is known for the
    /// selection), so switching views keeps the same point in the session
    /// in view rather than resetting to wherever the raw view was last left.
    ///
    /// The join between the two lists is why this stays here: which line to
    /// enter on is the timeline's to say, and what to do with it is
    /// [`RawView::enter`]'s.
    pub fn toggle_raw(&mut self) {
        if self.view == View::Raw {
            self.set_view(View::Events);
            return;
        }
        self.set_view(View::Raw);
        let line = (!self.timeline.follow)
            .then(|| {
                self.timeline
                    .selected_entry()
                    .and_then(|entry| entry.raw_index)
            })
            .flatten();
        self.raw.enter(line);
    }

    /// Show `view`, or return to the event list if it is already showing, so
    /// one key both opens and closes each alternate view. [`Self::toggle_raw`]
    /// stays separate because entering the raw view also has to line its
    /// selection up with the event list's.
    pub fn toggle_view(&mut self, view: View) {
        self.set_view(if self.view == view {
            View::Events
        } else {
            view
        });
    }

    /// Change views, ending link navigation and restoring its preview choice.
    pub fn set_view(&mut self, view: View) {
        if self.view != view {
            self.clear_link_highlight();
            self.view = view;
        }
    }

    /// Toggle the entry-log pane below the event list. Opening it returns to
    /// the event list, which is where the keys start; `Tab` is what moves them
    /// to the pane. The cursor starts on the pane's last, newest entry, which
    /// is what the preview then shows.
    pub fn toggle_entry_log(&mut self) {
        self.set_view(View::Events);
        self.processes_pane = false;
        let len = self.entry_log_len();
        self.entry_log.toggle(len);
        self.preview.scroll.reset();
    }

    /// Toggle the processes pane below the event list. It takes the entry
    /// log's place, so opening it closes that, and it returns to the event
    /// list, which keeps the navigation keys: the pane is for watching, and
    /// the full-screen view is where its rows are selected.
    pub fn toggle_processes_pane(&mut self) {
        self.set_view(View::Events);
        self.processes_pane = !self.processes_pane;
        if self.processes_pane && self.entry_log.open {
            let len = self.entry_log_len();
            self.entry_log.toggle(len);
        }
    }

    /// Whether a processes listing is on screen, full-screen or in the pane,
    /// and so worth keeping current.
    pub fn processes_visible(&self) -> bool {
        self.view == View::Processes || (self.view == View::Events && self.processes_pane)
    }

    /// Move the navigation keys between the event list and the entry-log pane,
    /// which are the two windows the Events screen shows at once. Taking the
    /// keys leaves the cursor on the entry the list was already on, so the
    /// preview does not jump on the press itself.
    pub fn toggle_entry_log_focus(&mut self) {
        if !self.entry_log.open {
            return;
        }
        if self.entry_log.focused() {
            self.entry_log.unfocus();
        } else {
            self.entry_log.focus();
        }
        self.preview.scroll.reset();
    }

    /// The list selection as a position in what the entry-log pane is showing.
    /// A hidden entry is not there to land on, so this is the first row at or
    /// after it.
    fn entry_log_cursor_for_selection(&self) -> usize {
        self.entry_log_indices()
            .iter()
            .position(|&idx| idx >= self.timeline.selected)
            .unwrap_or(0)
    }

    /// The timeline entries the entry-log pane is showing: the selected
    /// message's stretch, which is what its cursor moves within.
    ///
    /// The pane ignores the list's conversation-only filter — showing what
    /// that filter hides is the whole point of it — but not `m`: an operator
    /// who has minor lifecycle events turned off has said they are noise on
    /// this screen, and a pane that kept them would put them back.
    pub(crate) fn entry_log_indices(&self) -> Vec<usize> {
        let show_minor = self.timeline.show_minor;
        self.timeline
            .conversation_span()
            .filter(|&idx| show_minor || !self.timeline.entries[idx].event().is_minor())
            .collect()
    }

    /// How many entries the entry-log pane is showing; see
    /// [`Self::entry_log_indices`].
    pub(crate) fn entry_log_len(&self) -> usize {
        self.entry_log_indices().len()
    }

    /// Where the entry-log pane's cursor is in the timeline, while the pane is
    /// open. `None` whenever it is closed, so callers can treat it as "the
    /// pane is not driving this". Whether the pane holds the keys does not
    /// come into it: an open pane is what the preview reads either way.
    pub(crate) fn entry_log_index(&self) -> Option<usize> {
        if !self.entry_log.open {
            return None;
        }
        let shown = self.entry_log_indices();
        let cursor = self.entry_log.cursor(shown.len())?;
        shown.get(cursor).copied()
    }

    /// The entry the entry-log pane's cursor is on; see
    /// [`Self::entry_log_index`].
    pub(crate) fn entry_log_entry(&self) -> Option<&Entry> {
        self.timeline.entries.get(self.entry_log_index()?)
    }

    /// Move the entry-log pane's cursor, when it is the window holding the
    /// keys. The preview follows the cursor, so its offset — taken against
    /// another entry's content — goes back to the top.
    pub fn entry_log_select_next(&mut self) {
        let len = self.entry_log_len();
        self.entry_log.select_next(len);
        self.preview.scroll.reset();
    }

    pub fn entry_log_select_prev(&mut self) {
        let len = self.entry_log_len();
        self.entry_log.select_prev(len);
        self.preview.scroll.reset();
    }

    pub fn entry_log_page_down(&mut self) {
        let len = self.entry_log_len();
        self.entry_log.page_down(len);
        self.preview.scroll.reset();
    }

    pub fn entry_log_page_up(&mut self) {
        let len = self.entry_log_len();
        self.entry_log.page_up(len);
        self.preview.scroll.reset();
    }

    pub fn entry_log_select_first(&mut self) {
        self.entry_log.select_first();
        self.preview.scroll.reset();
    }

    pub fn entry_log_select_last(&mut self) {
        let len = self.entry_log_len();
        self.entry_log.select_last(len);
        self.preview.scroll.reset();
    }

    /// True when the operator can still send messages.
    pub fn can_send(&self) -> bool {
        self.activity.status.is_active()
    }

    /// What to ask the server for to fill the navigator: see
    /// [`LiveInteractions::query`](crate::interactions::LiveInteractions::query).
    /// The interactions this Session's branch markers lead to are kept too,
    /// finished or not, since the markers are drawn by their name, state and
    /// latest message. So are the branches drawn beneath a `branched to`
    /// marker, which this screen cannot name until the server has sent
    /// them. The navigator's own filters still leave them out.
    pub fn interactions_query(&self) -> styra_protocol::InteractionQuery {
        let mut query = self.interactions.query(&self.session_id);
        for entry in &self.timeline.entries {
            let event = entry.event();
            if let Some(id) = event.branch_target() {
                if !query.keep.iter().any(|kept| kept == id) {
                    query.keep.push(id.to_owned());
                }
                let leads_to_branch = matches!(
                    event,
                    AgentEvent::Branched {
                        direction: styra_protocol::event::BranchDirection::To,
                        ..
                    }
                );
                if leads_to_branch && !query.keep_branches_of.iter().any(|kept| kept == id) {
                    query.keep_branches_of.push(id.to_owned());
                }
            }
        }
        query
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

    /// Read integration off the listing into the status of the interaction
    /// on screen, once its agent is gone. Integration stops the agent itself,
    /// so the update stream only says the process went; what the operator
    /// wants to read is that the work landed. Returns whether the status
    /// changed.
    pub fn adopt_integration(&mut self) -> bool {
        if !matches!(
            self.activity.status,
            Status::Stopped(_) | Status::Ended { .. }
        ) {
            return false;
        }
        let Some(reason) = self
            .interactions
            .current(&self.session_id)
            .and_then(|interaction| {
                StopReason::integrated(interaction.completed, interaction.integration.as_ref())
            })
        else {
            return false;
        };
        let status = Status::Stopped(reason);
        if self.activity.status == status {
            return false;
        }
        self.activity.status = status;
        true
    }

    /// Put the selection on the last entry, where following leaves it, and
    /// start its preview from the top. The list scrolls to follow it again:
    /// only here, with the newest entry selected, does incoming activity move
    /// a viewport that navigation left alone.
    pub(crate) fn select_tail(&mut self) {
        self.timeline.select_tail();
        if self.timeline.list_anchor != ListAnchor::Free {
            self.timeline.list_anchor = ListAnchor::Selection;
        }
        self.preview.scroll.reset();
        self.entry_log_follow_selection();
    }

    // --- List navigation ----------------------------------------------------
    //
    // Where the selection lands is [`Timeline`]'s; the preview and entry-log
    // pane both follow it, so a move resets their scroll positions. The list
    // itself keeps its viewport unless the selection would leave it. See
    // [`Timeline::select_forward`] for what "moved" means.

    fn moved(&mut self, moved: bool) {
        self.branch_cursor = None;
        self.timeline.list_anchor = ListAnchor::Kept;
        self.timeline.list_scroll_delta = 0;
        if moved {
            self.preview.scroll.reset();
            self.entry_log_follow_selection();
        }
    }

    /// A filter change renumbers the rows the list shows, so the viewport it
    /// rendered last no longer means anything; anchor afresh on the selection.
    fn refiltered(&mut self, moved: bool) {
        self.moved(moved);
        self.timeline.list_anchor = ListAnchor::Selection;
        self.timeline.list_row_offset = 0;
    }

    pub fn scroll_interaction_down(&mut self) {
        self.timeline.scroll_view_down();
    }

    pub fn scroll_interaction_up(&mut self) {
        self.timeline.scroll_view_up();
    }

    /// Put the entry-log pane's cursor back on the list selection after that
    /// selection moved; see [`EntryLog::follow_list`].
    fn entry_log_follow_selection(&mut self) {
        let cursor = self.entry_log_cursor_for_selection();
        self.entry_log.follow_list(cursor);
    }

    /// Move between operator turns, keeping the chosen turn pinned as events arrive.
    pub(crate) fn select_diff_turn(&mut self, next: bool) {
        // Inherited history is hidden, so its turns are not there to visit.
        let is_message = |(index, entry): &(usize, &Entry)| {
            self.timeline.is_visible(*index)
                && matches!(
                    entry.event(),
                    styra_protocol::event::AgentEvent::UserMessage { .. }
                )
        };
        let entries = self.timeline.entries.iter().enumerate();
        let after_selection = self.timeline.selected.saturating_add(1);
        let target = if next {
            entries.skip(after_selection).find(is_message)
        } else {
            // The first message at or before the selection starts the current turn.
            entries
                .take(after_selection)
                .rev()
                .filter(is_message)
                .nth(1)
        }
        .map(|(index, _)| index);
        if let Some(target) = target {
            self.timeline.selected = target;
            self.timeline.follow = false;
            self.moved(true);
            self.sync_turn_diff();
        }
    }

    /// Move to the next visible entry.
    ///
    /// A `branched to` marker's branches are rows of their own here: the step
    /// walks down through them before leaving the marker.
    ///
    /// An entry running past the foot of the view is scrolled through, ten
    /// rows at a time, until all of it shows; only then does the step leave it.
    pub fn select_next_line(&mut self) {
        let next = self.selected_branch().map_or(0, |at| at + 1);
        if next < self.marker_branches().len() {
            self.moved(true);
            self.branch_cursor = Some((self.timeline.selected, next));
            return;
        }
        if self.timeline.reveal_selection_down() {
            return;
        }
        let branch_cursor = self.branch_cursor;
        let moved = self.timeline.select_forward();
        if !moved && self.timeline.selection_in_view() {
            // Re-anchoring would put a tall last entry back at its first
            // rows, undoing the scroll that just reached its end.
            return;
        }
        self.moved(moved);
        if !moved {
            // At the end of the list, stay on the last branch rather than
            // dropping back up to the marker's own row.
            self.branch_cursor = branch_cursor;
        }
    }

    /// Move to the previous visible entry; see [`Self::select_next_line`].
    /// Stepping up onto a marker lands on the last of its branches, and an
    /// entry cut off at the top of the view is first scrolled up into it.
    pub fn select_prev_line(&mut self) {
        if let Some(at) = self.selected_branch() {
            self.moved(true);
            self.branch_cursor = at.checked_sub(1).map(|at| (self.timeline.selected, at));
            return;
        }
        if self.timeline.reveal_selection_up() {
            return;
        }
        let moved = self.timeline.select_backward();
        if !moved && self.timeline.selection_in_view() {
            return;
        }
        self.moved(moved);
        if moved {
            self.branch_cursor = self
                .marker_branches()
                .len()
                .checked_sub(1)
                .map(|last| (self.timeline.selected, last));
        }
    }

    /// The sessions drawn beneath the selected entry when it is a `branched
    /// to` marker, in the order they are drawn. Empty for any other entry.
    fn marker_branches(&self) -> Vec<&str> {
        match self.timeline.selected_entry().map(|entry| entry.event()) {
            Some(AgentEvent::Branched {
                direction: styra_protocol::event::BranchDirection::To,
                session,
                ..
            }) => self
                .interactions
                .branch_tree(session, &self.session_id)
                .into_iter()
                .map(|(_, id)| id)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Which of the selected marker's branches carries the cursor, counted as
    /// [`Self::marker_branches`] lists them. `None` while the marker's own
    /// row — or any other entry — does.
    pub fn selected_branch(&self) -> Option<usize> {
        let (entry, at) = self.branch_cursor?;
        (entry == self.timeline.selected && at < self.marker_branches().len()).then_some(at)
    }

    /// The session the selected row leads to: the branch carrying the cursor
    /// beneath a marker, else the selected marker's own target.
    pub fn selected_branch_target(&self) -> Option<String> {
        match self.selected_branch() {
            Some(at) => self.marker_branches().get(at).map(|id| (*id).to_owned()),
            None => self
                .timeline
                .selected_entry()
                .and_then(|entry| entry.event().branch_target())
                .map(str::to_owned),
        }
    }

    pub fn select_first(&mut self) {
        let moved = self.timeline.select_first();
        self.moved(moved);
    }

    pub fn select_last(&mut self) {
        let moved = self.timeline.select_last();
        self.moved(moved);
    }

    /// Select the `branched to` marker leading to `branch`, centered in the
    /// viewport, so returning to a source lands where the branch left it.
    /// The newest marker wins should the journal carry several; a hidden one
    /// leaves the selection alone. Reports whether one was selected.
    pub(crate) fn select_branch_marker(&mut self, branch: &str) -> bool {
        let marker = (0..self.timeline.entries.len()).rev().find(|&at| {
            self.timeline.is_visible(at)
                && matches!(
                    self.timeline.entries[at].event(),
                    AgentEvent::Branched {
                        direction: styra_protocol::event::BranchDirection::To,
                        session,
                        ..
                    } if session == branch
                )
        });
        let Some(marker) = marker else { return false };
        self.timeline.selected = marker;
        self.timeline.follow = false;
        self.moved(true);
        self.timeline.list_anchor = ListAnchor::Centered;
        true
    }

    /// Select the newest entry of an interaction just opened from the live
    /// roster, centered in the viewport.
    pub(crate) fn select_last_on_interaction_open(&mut self) {
        self.select_last();
        self.timeline.list_anchor = ListAnchor::Centered;
    }

    /// Toggle whether minor lifecycle events (thread/turn/usage) are shown.
    pub fn toggle_minor(&mut self) {
        let moved = self.timeline.toggle_minor();
        self.refiltered(moved);
    }

    /// Toggle whether the main event list shows every event rather than only
    /// operator/agent messages.
    pub fn toggle_all_events(&mut self) {
        let moved = self.timeline.toggle_all_events();
        self.refiltered(moved);
    }

    /// Open the combined interaction/files layout with its entry preview
    /// visible, or return to the ordinary event list when already open.
    pub fn toggle_files(&mut self) {
        if self.view == View::Files {
            self.set_view(View::Events);
        } else {
            self.set_view(View::Files);
            self.preview.show();
        }
    }

    // --- Typed turn answers ---------------------------------------------------

    /// Open the overview, or leave it when already there. Opening asks the
    /// event loop for a fresh listing: the tiles name their Workspaces, and the
    /// Workspace list is only fetched on request.
    pub fn toggle_overview(&mut self) {
        if self.view == View::Overview {
            self.set_view(View::Events);
        } else {
            self.ask(Request::Overview);
        }
    }

    /// Show the answer view, asking the event loop to fetch under the
    /// session's recorded contract; or leave it when already there.
    pub fn toggle_answer(&mut self) {
        if self.view == View::Answer {
            self.set_view(View::Events);
        } else {
            self.set_view(View::Answer);
            self.ask(Request::Answer { contract: None });
        }
    }

    /// Re-read the same reply under another shape.
    pub fn reread_answer(&mut self, contract: Contract) {
        self.ask(Request::Answer {
            contract: Some(contract),
        });
    }

    pub fn file_select_next(&mut self) {
        let last = self.file_paths().len().saturating_sub(1);
        self.files.select_next(last);
    }

    pub fn file_select_prev(&mut self) {
        self.files.select_prev();
    }

    pub fn toggle_file_scope(&mut self) {
        self.files.toggle_scope();
    }

    /// Files touched or named by the focused entry, or by the whole session;
    /// see [`files::mentioned`].
    pub fn file_paths(&self) -> Vec<String> {
        let entries: Box<dyn Iterator<Item = &Entry> + '_> = if self.files.shows_all() {
            Box::new(self.timeline.entries.iter())
        } else {
            Box::new(self.timeline.selected_entry().into_iter())
        };
        files::mentioned(entries, self.workspace.root())
    }

    /// Resolve the selected Files-view entry to the corresponding host path.
    ///
    /// In the answer view it is the selected location of a `files` answer
    /// instead, so `e` opens what the agent named without a second mechanism
    /// for doing the same thing.
    pub fn selected_file_path(&self) -> Option<PathBuf> {
        let root = self.workspace.root_or_current_directory()?;
        if self.view == View::Answer {
            let file = self.answer.selected_file()?;
            return Some(if file.path.is_absolute() {
                file.path.clone()
            } else {
                root.join(&file.path)
            });
        }
        files::items(&root, self.file_paths())
            .get(self.files.selected_index())
            .map(|item| item.resolved.clone())
    }

    // --- Markdown-link navigation ------------------------------------------

    /// Select the first link at or below the selected event, wrapping to the
    /// start of the visible conversation when the selection is after its last
    /// link. The selection is kept in timeline coordinates, so it remains
    /// correct as messages wrap or link destinations are toggled. Opens the
    /// preview until link navigation ends, preserving its previous visibility.
    pub fn highlight_first_link(&mut self) {
        self.set_view(View::Events);
        let start = self.timeline.selected;
        self.link_highlight = self
            .find_link_from(start, 0)
            .or_else(|| self.find_link_before(start));
        if self.link_highlight.is_some() {
            self.link_preview_open.get_or_insert(self.preview.open);
            self.preview.show();
        } else {
            self.clear_link_highlight();
            self.show_warning("no links or file references in the visible conversation");
        }
        self.reveal_highlighted_link();
    }

    pub fn highlight_next_link(&mut self) {
        let Some(current) = self.link_highlight else {
            return;
        };
        self.link_highlight = self
            .find_link_from(current.entry, current.link + 1)
            .or(self.link_highlight);
        self.reveal_highlighted_link();
    }

    pub fn highlight_prev_link(&mut self) {
        let Some(current) = self.link_highlight else {
            return;
        };
        let mut found = None;
        for index in 0..=current
            .entry
            .min(self.timeline.entries.len().saturating_sub(1))
        {
            if !self.timeline.is_visible(index) {
                continue;
            }
            let count = self.entry_link_count(index);
            let limit = if index == current.entry {
                current.link
            } else {
                count
            };
            if limit > 0 {
                found = Some(LinkHighlight {
                    entry: index,
                    link: limit - 1,
                });
            }
        }
        self.link_highlight = found.or(self.link_highlight);
        self.reveal_highlighted_link();
    }

    /// Leave link navigation, restoring ordinary keys and preview visibility.
    pub fn clear_link_highlight(&mut self) {
        self.link_highlight = None;
        // The offset was into the linked file, not the entry shown again now.
        self.preview.scroll.reset();
        if let Some(open) = self.link_preview_open.take() {
            self.preview.open = open;
        }
    }

    /// Offer the actions that can be taken on the highlighted link.
    pub fn open_link_menu(&mut self) {
        let Some(destination) = self.highlighted_link_destination() else {
            return self.show_warning("selected link no longer exists");
        };
        if styra_ui::answer::link_section(&destination).is_some() {
            self.open_highlighted_link();
            return;
        }
        let web = is_web_address(&destination);
        self.link_menu = Some(LinkMenu::new(destination, web));
    }

    /// Ask the event loop to open the selected link: a web address in the
    /// configured browser, anything else in the configured editor. A file
    /// link is looked for as [`files::locate`] describes; one that cannot be
    /// found is reported rather than opened, and stays highlighted.
    pub fn open_highlighted_link(&mut self) {
        let Some(destination) = self.highlighted_link_destination() else {
            return self.show_warning("selected link no longer exists");
        };
        if styra_ui::answer::link_section(&destination).is_some() {
            let Some(answer) = self.highlighted_answer() else {
                return self.show_warning("selected answer no longer exists");
            };
            self.answer.set(Ok(answer));
            self.set_view(View::Answer);
        } else if is_web_address(&destination) {
            self.ask(Request::OpenUrl(destination));
        } else if let Some((_, path, _)) = self.highlighted_link_target() {
            match path {
                Ok(path) => self.ask(Request::OpenPath(path)),
                Err(problem) => return self.show_error(problem),
            }
        }
        self.clear_link_highlight();
    }

    /// Read the highlighted answer from its own message and request contract,
    /// shared by the preview and the full answer view.
    pub(crate) fn highlighted_answer(&self) -> Option<styra_protocol::Answer> {
        let section = styra_ui::answer::link_section(&self.highlighted_link_destination()?)?;
        let highlight = self.link_highlight?;
        let AgentEvent::AgentMessage { text } = self.timeline.entries.get(highlight.entry)?.event()
        else {
            return None;
        };
        let (start, end) = styra_ui::answer::sections(text).nth(section)?;
        let contract = self.timeline.entries[..highlight.entry]
            .iter()
            .rev()
            .find(|entry| matches!(entry.event(), AgentEvent::UserMessage { .. }))
            .and_then(|entry| entry.contract)
            .unwrap_or(Contract::Text);
        Some(styra_protocol::contract::read(&text[start..end], contract))
    }

    /// The active file link as written, the file it names on this host (or
    /// why none was found), and its optional one-based line number. Every use
    /// of a file link — opening, previewing, mounting — goes through this, so
    /// a `path:line` citation never attempts to read a file literally named
    /// `path:line`, and none of them can find a different file from the
    /// others. A web address names no file, so it has no target.
    pub fn highlighted_link_target(
        &self,
    ) -> Option<(String, Result<PathBuf, String>, Option<u32>)> {
        let destination = self.highlighted_link_destination()?;
        if is_web_address(&destination) || styra_ui::answer::link_section(&destination).is_some() {
            return None;
        }
        let (path, line) = split_link_location(&destination);
        let workspace = self.workspace.link_boundary();
        // The checkout the Workspace root is in, rather than the one recorded
        // for the Workspace: in a linked worktree they are different trees,
        // and the file wanted is the one this interaction is working on.
        let repository = self.workspace.git_repository.as_ref().map(|recorded| {
            workspace
                .and_then(files::enclosing_checkout)
                .unwrap_or_else(|| recorded.clone())
        });
        let resolved = match self
            .workspace
            .working_directory_or_current()
            .or_else(|| workspace.map(Path::to_path_buf))
        {
            Some(start) => files::locate(&start, workspace, repository.as_deref(), path),
            None => Err(format!("{path}: no directory to look for it in")),
        };
        Some((destination, resolved, line))
    }

    /// The active link's destination, exactly as the reply wrote it.
    fn highlighted_link_destination(&self) -> Option<String> {
        let highlight = self.link_highlight?;
        styra_ui::answer::presented_detail(
            self.timeline.entries.get(highlight.entry)?.event(),
            self.selection.provider.protocol(),
        )
        .into_iter()
        .filter_map(|block| match block {
            DetailBlock::Text(text) => Some(text),
            DetailBlock::Code { .. } => None,
        })
        .scan(highlight.link, |remaining, text| {
            let count = styra_ui::markdown::markdown_link_count(&text);
            if *remaining < count {
                let destination = styra_ui::markdown::markdown_link_destination(&text, *remaining);
                *remaining = 0;
                Some(destination)
            } else {
                *remaining -= count;
                Some(None)
            }
        })
        .flatten()
        .next()
    }

    fn find_link_from(&self, start: usize, first_link: usize) -> Option<LinkHighlight> {
        for index in start..self.timeline.entries.len() {
            if !self.timeline.is_visible(index) {
                continue;
            }
            let link = if index == start { first_link } else { 0 };
            if link < self.entry_link_count(index) {
                return Some(LinkHighlight { entry: index, link });
            }
        }
        None
    }

    /// Find the last visible link before `end`. This is the wrapped portion
    /// of the initial `f` search, so it lands on the nearest citation above
    /// the current selection; next/previous link navigation itself stays
    /// bounded at the ends so it never unexpectedly loops while reading.
    fn find_link_before(&self, end: usize) -> Option<LinkHighlight> {
        for index in (0..end.min(self.timeline.entries.len())).rev() {
            if !self.timeline.is_visible(index) {
                continue;
            }
            let count = self.entry_link_count(index);
            if count > 0 {
                return Some(LinkHighlight {
                    entry: index,
                    link: count - 1,
                });
            }
        }
        None
    }

    fn entry_link_count(&self, index: usize) -> usize {
        styra_ui::answer::presented_detail(
            self.timeline.entries[index].event(),
            self.selection.provider.protocol(),
        )
        .into_iter()
        .map(|block| match block {
            DetailBlock::Text(text) => styra_ui::markdown::markdown_link_count(&text),
            DetailBlock::Code { .. } => 0,
        })
        .sum()
    }

    fn reveal_highlighted_link(&mut self) {
        let Some(highlight) = self.link_highlight else {
            return;
        };
        self.timeline.selected = highlight.entry;
        self.timeline.entries[highlight.entry].expanded = true;
        self.timeline.follow = false;
        self.preview.scroll.reset();
        // A file link to a line opens its preview with that line in the
        // middle, from where PgUp/PgDn read the rest of the file.
        if let Some((_, Ok(_), Some(line))) = self.highlighted_link_target() {
            let line = u16::try_from(line.saturating_sub(1)).unwrap_or(u16::MAX);
            self.preview.scroll.offset = line.saturating_sub(self.preview.scroll.half_page());
        }
        self.entry_log_follow_selection();
    }

    /// Whether the checkout diff is on screen, including while composing.
    pub fn diff_visible(&self) -> bool {
        self.diff_preview && matches!(self.view, View::Events | View::Files)
    }

    /// Whether the live interactions navigator is on screen. It stays open
    /// across views until something closes it, but is only drawn over the
    /// event list, so it only takes keys there.
    pub fn navigator_shown(&self) -> bool {
        self.interactions.open && self.view == View::Events
    }

    /// Whether the diff, rather than the interaction or message box, owns keys.
    pub fn diff_focused(&self) -> bool {
        self.diff_visible() && self.focus == Focus::List && !self.diff_interaction_focus
    }

    /// The entry the preview panel and the `y` shortcut act on: the one the
    /// entry-log pane's cursor is on while that pane holds the navigation
    /// keys, otherwise the selected one — or, in [`PreviewTarget::Command`],
    /// the newest shell entry. That entry holds both the command and its
    /// result, since a completion replaces its start row in place (see
    /// [`Self::push_event`]). Falls back to the selection while no command has
    /// run yet, so the panel is never blank just because the mode is on.
    pub fn preview_entry(&self) -> Option<&Entry> {
        // The entry-log pane comes first, and ahead of the command mode: while
        // it holds the keys it is the window the operator is moving, and a
        // panel that did not follow that cursor would be showing something
        // they are not looking at.
        if let Some(entry) = self.entry_log_entry() {
            return Some(entry);
        }
        if self.preview.follows_command() {
            if let Some(entry) = self.timeline.newest_command() {
                return Some(entry);
            }
        }
        self.timeline.selected_entry()
    }

    /// The Session named by the `branch` marker the preview is on, and which
    /// way the marker points, while the preview is on screen. A marker that
    /// names this Session has nothing else to show. When the cursor is on one
    /// of the branches listed beneath the selected marker, it is that branch.
    pub(crate) fn preview_branch_target(
        &self,
    ) -> Option<(&str, styra_protocol::event::BranchDirection)> {
        let shown = match self.view {
            View::Preview => true,
            View::Events | View::Files => self.preview.open,
            _ => false,
        };
        if !shown {
            return None;
        }
        let entry = self.preview_entry()?;
        let on_selection = self
            .timeline
            .selected_entry()
            .is_some_and(|selected| std::ptr::eq(selected, entry));
        if let Some(at) = self.selected_branch().filter(|_| on_selection) {
            // Each listed branch was branched off the one above it, so it is
            // read as a `branched to` marker naming it would be.
            return self
                .marker_branches()
                .get(at)
                .map(|id| (*id, styra_protocol::event::BranchDirection::To));
        }
        match entry.event() {
            AgentEvent::Branched {
                direction, session, ..
            } if *session != self.session_id => Some((session, *direction)),
            _ => None,
        }
    }

    /// Whether the launch policy can still be edited; see [`launch::editable`].
    pub fn can_edit_launch(&self) -> bool {
        launch::editable(&self.activity.status)
    }

    /// Refuse an edit to the launch policy when there is nothing to edit,
    /// saying why. Returns whether the caller may proceed. Public because the
    /// keys for it live in [`crate::launch`], and because one of them leaves
    /// this process to do its work (writing the defaults file) and must refuse
    /// with the same words as the rest.
    pub fn allow_launch_edit(&mut self) -> bool {
        if self.workspace_launch_pending > 0 {
            self.show_warning("the Workspace launch policy is still saving");
            return false;
        }
        if self.can_edit_launch() {
            return true;
        }
        self.show_warning("the launch policy is fixed while an interaction is running");
        false
    }

    /// Whether mounts can be added or removed; see [`launch::mounts_editable`].
    pub fn can_change_mounts(&self) -> bool {
        launch::mounts_editable(&self.activity.status)
    }

    /// [`Self::allow_launch_edit`] for mounts, which an idle interaction is
    /// restarted to take rather than refused.
    pub fn allow_mount_edit(&mut self) -> bool {
        if self.workspace_launch_pending > 0 {
            self.show_warning("the Workspace launch policy is still saving");
            return false;
        }
        if self.can_change_mounts() {
            return true;
        }
        self.show_warning(
            "mounts are fixed while the agent is working — they can change once it is idle",
        );
        false
    }

    /// Record that a mount was just added or removed. Where nothing is running
    /// the next launch picks it up anyway; an idle interaction's sandbox has
    /// to be restarted for it to apply, which the event loop does.
    pub fn note_mount_change(&mut self) {
        if self.mount_change_restarts() {
            self.restart_for_mounts = true;
        }
    }

    /// Whether changing a mount now restarts the interaction, which every
    /// place a mount can be changed says before it is.
    pub fn mount_change_restarts(&self) -> bool {
        !self.can_edit_launch() && self.activity.status.is_idle()
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
                for block in protocol.presented_detail(entry.event(), PresentationMode::Pretty) {
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
                    text = protocol.presented_summary(entry.event(), PresentationMode::Pretty);
                }
                Some(text)
            }
            View::Raw if self.provider_raw_open => self
                .provider_raw
                .as_ref()
                .and_then(|raw| raw.selected().map(str::to_owned)),
            View::Raw => self.raw.selected().map(|line| line.text.clone()),
            View::Messages => self.messages.selected().map(|line| line.text.clone()),
            View::Processes => self.processes.selected_command(),
            View::Files => self
                .selected_file_path()
                .map(|path| path.display().to_string()),
            // A navigable answer copies the selected row; one that is read
            // rather than navigated copies the whole value, which is what an
            // operator reaching for `y` on a JSON or prose answer wants.
            View::Answer => self.answer.copy_text(),
            View::Log | View::Quota | View::Transcript | View::Driva | View::Overview => None,
        }
    }

    /// The session as the transcript view shows it: the entries the list's
    /// filters leave visible, laid out by genta's `render_events`. The view
    /// renders this fresh each frame, so a filter change needs no other wiring.
    pub fn transcript_text(&self) -> String {
        self.render_entries(|app, idx, _| app.timeline.is_visible(idx))
    }

    /// The whole session as a readable exchange, for the `Y` shortcut. Only
    /// the conversational events go in — the same set the `c` filter leaves on
    /// screen ([`AgentEvent::is_conversation`]) — but they are laid out by the
    /// same renderer the transcript view uses, so what lands on the clipboard
    /// is the transcript an operator has been reading rather than a second
    /// formatting of the same events.
    ///
    /// The list's own conversation-only filter is deliberately not consulted:
    /// `Y` says what it copies, and an operator who has the tool calls on
    /// screen still asked for the conversation.
    ///
    /// `None` when the session has said nothing yet, so `Y` reports that
    /// instead of copying a blank.
    pub fn conversation_text(&self) -> Option<String> {
        let text = self.render_entries(|_, _, entry| entry.event().is_conversation());
        (!text.trim().is_empty()).then_some(text)
    }

    /// Render the entries `keep` selects, the one way this program lays events
    /// out as text.
    fn render_entries(&self, keep: impl Fn(&Self, usize, &Entry) -> bool) -> String {
        let events = self
            .timeline
            .entries
            .iter()
            .enumerate()
            .filter(|(idx, entry)| keep(self, *idx, entry))
            .map(|(_, entry)| entry.event().clone())
            .collect::<Vec<_>>();
        styra_protocol::render::render_events(&events, false, self.timeline.show_minor)
    }

    // --- Focus ---------------------------------------------------------------

    pub fn enter_input(&mut self) {
        self.focus = Focus::Input;
        self.message_source = None;
    }

    /// Recover diff annotations from the current session history.
    pub(crate) fn diff_prompts(&self) -> Vec<styra_diff::Prompt> {
        self.timeline
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry.event() {
                AgentEvent::UserMessage { text } => styra_diff::Prompt::from_message(text, index),
                _ => None,
            })
            .collect()
    }

    /// Reveal the latest prompt attached to the current file or line.
    pub(crate) fn jump_to_diff_prompt(&mut self) -> bool {
        let entry = self
            .shown_diff()
            .and_then(|diff| diff.source())
            .and_then(|source| {
                self.diff_prompts()
                    .iter()
                    .rev()
                    .find(|p| p.matches(source))
                    .map(|p| p.entry)
            });
        let Some(entry) = entry else { return false };
        self.set_view(View::Events);
        self.diff_turn = false;
        self.diff_interaction_focus = self.diff_preview;
        self.entry_log.unfocus();
        self.timeline.selected = entry;
        self.timeline.entries[entry].expanded = true;
        self.timeline.follow = false;
        self.moved(true);
        true
    }

    /// Start a message about what the diff on screen — the checkout's or the
    /// turn's — is on: the file it shows, and the line cursor's line, if any
    /// (see [`styra_diff::CheckoutDiff::source`]).
    pub fn enter_input_from_diff(&mut self) {
        self.enter_input();
        self.message_source =
            self.shown_diff()
                .and_then(|diff| diff.source())
                .map(|(path, line)| match line {
                    Some(line) => format!("{path}:{line}"),
                    None => path.to_owned(),
                });
    }

    /// Leave the message box. Coming back to it means typing again, into
    /// whichever box was last highlighted, rather than choosing between them.
    pub fn enter_list(&mut self) {
        self.focus = Focus::List;
        self.composer.stop_choosing();
        self.message_source = None;
    }

    // --- Message editing -----------------------------------------------------
    //
    // The buffer and its history are [`Composer`]'s; what is left here is the
    // one part of sending that is not — a sent message leaves the box, and the
    // rest of the client wants it.

    /// Take the trimmed message for sending, clearing the buffer, with a note
    /// under it of what it was started from in the diff. Returns `None` when
    /// the buffer holds only whitespace.
    pub fn take_message(&mut self) -> Option<String> {
        let message = self.composer.take()?;
        let message = self.with_message_source(message);
        self.message_source = None;
        Some(message)
    }

    /// `message` with a note under it of what it was started from in the
    /// diff, if it was. The source is kept, so a send that fails can be
    /// tried again with it.
    pub fn with_message_source(&self, message: String) -> String {
        match &self.message_source {
            Some(source) => format!("{message}\n\nSource: {source}"),
            None => message,
        }
    }

    /// The rebase being followed, when it is of the interaction on screen.
    pub fn shown_rebase(&self) -> Option<&styra_protocol::RebaseStatus> {
        self.rebase_status
            .as_ref()
            .filter(|(id, _)| *id == self.session_id)
            .map(|(_, status)| status)
    }

    /// Ask the event loop for something this screen cannot do itself; see
    /// [`Request`].
    pub fn ask(&mut self, request: Request) {
        self.requests.push_back(request);
    }

    /// Take the operator's pending request, if any, for the event loop to act on.
    pub fn take_request(&mut self) -> Option<Request> {
        self.requests.pop_front()
    }

    /// Take the next effect only when it is a Workspace launch edit. Modal
    /// launch controls use this to dispatch their own follow-up immediately
    /// without consuming an unrelated request that was already ahead of it.
    pub fn take_workspace_launch_request(&mut self) -> Option<Request> {
        matches!(
            self.requests.front(),
            Some(Request::ChangeWorkspaceLaunch { .. })
        )
        .then(|| self.requests.pop_front())
        .flatten()
    }
}

/// Split a Markdown destination's optional `:line[:column]` suffix without
/// disturbing drive-letter or colon-bearing paths. The rightmost numeric part
/// is a line unless another numeric part precedes it, in which case it is a
/// column and the preceding part is the line.
/// Whether a link destination is a web page rather than a file.
fn is_web_address(destination: &str) -> bool {
    destination.starts_with("http://") || destination.starts_with("https://")
}

fn split_link_location(destination: &str) -> (&str, Option<u32>) {
    let Some((before_last, last)) = destination.rsplit_once(':') else {
        return (destination, None);
    };
    let Ok(last) = last.parse::<u32>() else {
        return (destination, None);
    };
    if let Some((path, line)) = before_last
        .rsplit_once(':')
        .and_then(|(path, line)| line.parse::<u32>().ok().map(|line| (path, line)))
    {
        (path, Some(line))
    } else {
        (before_last, Some(last))
    }
}

#[cfg(test)]
mod tests;
