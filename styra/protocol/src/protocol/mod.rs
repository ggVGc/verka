//! Stable JSON contract shared by the Styra Unix-socket server and its clients.
//!
//! This module is the single entry point for every value that crosses the
//! Styra client/server boundary. It describes what is said, not how it is
//! carried: the framing and the socket belong to the peers. See
//! `protocol/README.md` for the transport framing and behavioral rules.

use crate::agent::Selection;
use crate::event::AgentEvent;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod types;

pub use types::{
    Answer, AnswerValue, AttributedMount, AttributedVariable, BaseCapability, BaseEntry,
    BranchCheckout, BranchHistory, BranchPoint, CheckoutState, CleanedWorktree, CompletionState,
    Contract, DiffBase, Direction, DrivaOptions, EditReport, EditReportFile, FileEditReport,
    FileLocation, Integration, IntegrationResult, InteractionActivity, InteractionActivityReason,
    InteractionEnd, InteractionQuery, InteractionSummary, InteractionUpdate, LaunchMount,
    LaunchPolicy, LogEntry, LogLevel, ModelSummary, MountOrigin, ProjectAction, QueuedMessage,
    QuotaEvent, QuotaStatus, RawLine, RebaseStatus, RecentBranch, RecentMessage, SandboxProcess,
    SavedBranch, SessionOrigin, SessionSummary, TemplateSummary, TurnDiff, VariableOrigin,
    WorkspaceSummary, WorktreeCleanup, WorktreeSession, WorktreeSummary, WORKTREES_DIR,
};

// These external vocabularies are serialized inside protocol payloads. Re-export
// them here so the complete wire surface is discoverable from this module.
pub use crate::agent::{Effort, Provider, Selection as AgentSelection};
pub use crate::event::AgentEvent as ProtocolAgentEvent;
pub use crate::{EnvironmentOrigin, FloorEntry, FloorKind, Mount, MountAccess, WritableMountMode};

/// `serde` default for the protocol's opt-out booleans, whose absence must
/// mean "as before".
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub service: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspace {
    pub host_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A path at or inside the Git repository this Workspace should carry.
    /// The server resolves and stores the checkout root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_repository: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSession {
    pub workspace_id: String,
    pub selection: Selection,
    /// This launch's own sandbox policy, layered over the Workspace's standing
    /// one (see [`LaunchPolicy::merge`]). Templates are names and mounts are
    /// requests; the server resolves both after merging.
    #[serde(default)]
    pub launch: LaunchPolicy,
    /// Create a linked Git worktree and branch for this session. This is a
    /// per-session choice; Workspaces never create them automatically.
    #[serde(default)]
    pub create_worktree: bool,
    /// Reuse the checkout associated with this Session, when it has one.
    /// With `create_worktree`, fork its current branch into a fresh checkout
    /// instead. The source must belong to `workspace_id`. A source without a
    /// linked checkout falls back to the Workspace checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_from: Option<String>,
    /// With `create_worktree`, the local branch the fresh checkout starts
    /// from, and so the branch it integrates into. It takes the place of
    /// `checkout_from`'s branch and of the Workspace checkout's. Must name a
    /// branch under `refs/heads/`; revision syntax is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional operator-facing name. When absent, the server derives one
    /// from `message` when possible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Apply a return contract to the seed `message`, exactly as
    /// [`SendMessage::contract`] does for a later turn. This is what makes a
    /// one-shot question a single request: create, ask, and type the answer in
    /// one call. Ignored when there is no seed message to frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
    /// Ask every client showing an interaction in this Workspace to switch to
    /// the new one, as an editor does when it starts work the operator is
    /// about to watch. Carried to clients as
    /// [`InteractionSummary::focus_requested`].
    #[serde(default)]
    pub focus: bool,
}

/// Ask what a new session in this Workspace *would* be launched under, without
/// creating one. Carries exactly the launch inputs of [`CreateSession`] that
/// shape the sandbox, so the answer is the policy that session would get.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSession {
    pub workspace_id: String,
    pub selection: Selection,
    #[serde(default)]
    pub launch: LaunchPolicy,
    /// Whether the prospective session should get its own linked worktree.
    #[serde(default)]
    pub create_worktree: bool,
    /// Match [`CreateSession::checkout_from`] for the prospective launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_from: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameSession {
    pub id: String,
    /// `None` (or whitespace-only text) clears the name.
    pub name: Option<String>,
}

/// Change a Workspace's optional operator-facing name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameWorkspace {
    pub id: String,
    /// `None` (or whitespace-only text) clears the name and restores the
    /// host-directory display fallback.
    pub name: Option<String>,
}

/// Replace a Session's operator-assigned tags.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSessionTags {
    pub id: String,
    pub tags: Vec<String>,
}

/// What an operator has half-written to a Session, and what they have already
/// sent it: the message box, kept by the server so it is there again from any
/// client, after the client is closed, and after the interaction is resumed.
///
/// The server stores it as given and keeps the newest
/// [`COMPOSER_HISTORY_LIMIT`] history entries; it does not interpret it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerState {
    /// The boxes a message is being built from, in order. A client treats an
    /// empty list as one empty box.
    #[serde(default)]
    pub parts: Vec<String>,
    /// Which of `parts` the operator was last typing in.
    #[serde(default)]
    pub focused: usize,
    /// Messages already sent to this Session from the message box, oldest
    /// first, for walking back through.
    #[serde(default)]
    pub history: Vec<String>,
}

/// How many sent messages a Session's [`ComposerState`] keeps.
pub const COMPOSER_HISTORY_LIMIT: usize = 200;

/// One server-owned edit to a Workspace's standing launch policy.
///
/// Clients send intent instead of replacing a locally cached copy. This lets
/// the server apply the edit to the latest stored policy and return the
/// authoritative result, avoiding lost updates between multiple Styra UIs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", content = "value", rename_all = "snake_case")]
pub enum WorkspaceLaunchChange {
    SetNetwork(Option<bool>),
    SetWritableWorkspace(Option<bool>),
    SetTemplates(Vec<String>),
    AddMounts(Vec<LaunchMount>),
    RemoveMount(LaunchMount),
    Replace(LaunchPolicy),
    /// State the Workspace's command for a [`ProjectAction`]; `None` leaves it
    /// to the templates' default.
    SetCommand {
        action: ProjectAction,
        command: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSession {
    pub id: String,
    #[serde(default)]
    pub launch: LaunchPolicy,
    /// The model and reasoning effort to revive the session on. `None` keeps
    /// the selection the Session is stored with, which is what an unattended
    /// resume (a rate-limit retry) wants; a client sends its own so an
    /// operator who changed the model while nothing was running gets the
    /// model they chose rather than the one the stopped turn ran on.
    ///
    /// A different provider converts the native transcript in place and adds
    /// a provider-change marker before resuming the same Session. Requires a
    /// stopped interaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub workspace_id: String,
    pub selection: Selection,
    pub workspace: PathBuf,
    pub journal_path: PathBuf,
    pub driva: DrivaOptions,
    /// Update cursor immediately after any journal history used to initialize
    /// this interaction. The client which already rendered that journal starts
    /// here; a later attachment starts at zero to receive the full history.
    #[serde(default)]
    pub updates_after: u64,
    /// Operator messages the interaction has queued but not yet sent, carried
    /// over from a previous attachment (e.g. one stopped before the interaction
    /// went idle), so a fresh client hydrates its input queue instead of
    /// silently dropping it.
    #[serde(default)]
    pub queued: Vec<QueuedMessage>,
}

/// Host-side tmux endpoint for the shell owned by a live session's sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellInfo {
    pub tmux: PathBuf,
    pub socket: PathBuf,
}

/// A [`ProjectAction`] started in a live session's sandbox: the tmux session
/// running it, beside the session's shell on the same server, for a client to
/// attach a terminal to. Its window stays open after the command exits, until
/// someone dismisses it, so a failure can still be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectActionSession {
    pub tmux: PathBuf,
    pub socket: PathBuf,
    /// The tmux session to attach to.
    pub session: String,
    /// The shell command it runs, as resolved.
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessage {
    pub text: String,
    /// Per-turn Codex selection. Older clients omit this and retain the
    /// interaction's current defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
    /// Ask this turn's reply to come back in a named shape. The server frames
    /// `text` with the contract's instructions before sending and records the
    /// contract with the session, so [`Request::TurnAnswer`] can parse the
    /// reply without the client restating it. Absent means an ordinary,
    /// untyped turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
}

impl SendMessage {
    /// A plain turn: text alone, under whatever the session already runs.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            selection: None,
            contract: None,
        }
    }

    /// Run this turn under `selection`, switching the session onto it.
    pub fn under(mut self, selection: Selection) -> Self {
        self.selection = Some(selection);
        self
    }

    /// Ask this turn's reply to come back in `contract`'s shape.
    pub fn asking_for(mut self, contract: Contract) -> Self {
        self.contract = Some(contract);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencedUpdate {
    pub sequence: u64,
    pub update: InteractionUpdate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Updates {
    pub updates: Vec<SequencedUpdate>,
    /// Cursor to pass as `after` on the next request.
    pub next: u64,
}

/// Everything a client needs to make one live interaction current, returned
/// by one ordinary blocking request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoadedInteraction {
    pub summary: InteractionSummary,
    pub updates: Updates,
    pub queued: Vec<QueuedMessage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredSession {
    pub summary: SessionSummary,
    pub events: Vec<AgentEvent>,
    /// Empty when the request asked for no raw lines.
    pub raw: Vec<RawLine>,
    /// The host directory the Session was last working in, when its journal
    /// says it moved out of the one it was launched in — a Codex told to work
    /// elsewhere, a Claude Code that made a worktree of its own. `None` when it
    /// worked where it was launched, which is the Workspace directory or the
    /// Session's checkout; a replay screen shows that instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
}

/// A provider's own persisted session file, kept distinct from Styra's wire
/// journal. The text is verbatim JSONL, including records that never appeared
/// on the app-server wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderRaw {
    pub provider: Provider,
    pub text: String,
}

/// One JSON request sent as a single line over the Unix socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Request {
    /// Rebase and squash-integrate a Session into its recorded source branch.
    IntegrateSession {
        id: String,
        message: String,
    },
    /// Ask an errand for an editable squash commit message.
    GenerateIntegrationMessage {
        id: String,
    },
    /// Have an errand describe each edited file of the Session's checkout —
    /// what changed in it and why — and keep the report with the Session,
    /// replacing any earlier one. The client sends the patches, split per file
    /// exactly as it shows them, so the report covers the files it lists.
    GenerateEditReport {
        id: String,
        /// The commit the patches are measured from.
        base: String,
        files: Vec<EditReportFile>,
        /// Keep the earlier report's descriptions of the files not sent,
        /// replacing only those of the files sent: describing one file again.
        #[serde(default)]
        keep_others: bool,
    },
    /// The Session's last generated edit report, if it has one.
    EditReport {
        id: String,
    },
    /// None while integration is running; otherwise its final result.
    SessionIntegrationResult {
        id: String,
    },
    /// Rebase onto the recorded source branch, retaining the checkout.
    RebaseSession {
        id: String,
    },
    /// Save a copy of the interaction branch, then replay its commits onto another
    /// local branch, keeping the branch name.
    RetargetSession {
        id: String,
        target: String,
    },
    /// Saved copies of the interaction branch, oldest first. Their Git refs are retained.
    SessionBranchHistory {
        id: String,
    },
    /// Return the interaction branch to a copy saved by retargeting, without rebasing it.
    RestoreSessionBranch {
        id: String,
        branch: String,
    },
    SessionRebaseStatus {
        id: String,
    },
    Health,
    CreateWorkspace(CreateWorkspace),
    ListWorkspaces,
    /// Deleted Workspaces are available only for listing and restoration.
    ListDeletedWorkspaces,
    /// Soft delete: preserve metadata, Sessions, and checkouts on disk.
    DeleteWorkspace {
        id: String,
    },
    RestoreWorkspace {
        id: String,
    },
    Workspace {
        id: String,
    },
    /// Name the Workspace a host directory belongs to. The path need not be a
    /// Workspace root: a directory anywhere beneath one answers with that
    /// Workspace, so a client holding only a working directory — an editor,
    /// a script run from a subdirectory — can find the Workspace over it
    /// without listing them all and comparing paths itself. The innermost
    /// Workspace wins when they nest, and the most recently accessed one when
    /// several name the same directory. Answers with nothing rather than an
    /// error when no Workspace covers the path, which is an ordinary answer.
    WorkspaceForPath {
        path: PathBuf,
    },
    RenameWorkspace(RenameWorkspace),
    /// Associate (or disassociate) a Workspace with a Git checkout. The path
    /// may be anywhere inside the checkout; the server stores its root.
    SetWorkspaceGitRepository {
        workspace_id: String,
        git_repository: Option<PathBuf>,
    },
    /// Point a Workspace at a different host directory. The path must be
    /// absolute and must exist; the server stores it canonicalized. The
    /// Workspace keeps its id, Sessions and policy; Interactions already
    /// running stay where they were spawned.
    SetWorkspaceHostPath {
        workspace_id: String,
        host_path: PathBuf,
    },
    /// Read the server-owned Workspace launch policy without touching the
    /// Workspace's last-accessed timestamp. Used as a lightweight change feed
    /// by clients displaying the Driva options view.
    WorkspaceLaunch {
        workspace_id: String,
    },
    CreateSession(CreateSession),
    /// Report the Driva policy a `CreateSession` with these inputs would run
    /// under. Creates nothing and touches no session state.
    PlanSession(PlanSession),
    /// Name the Driva templates a session in this Workspace could be launched
    /// with: Driva's built-ins, overridden by any `driva.toml` the Workspace
    /// carries. Resolves the same set `templates` on a launch request is
    /// looked up in, so a client can offer exactly what would be accepted.
    ListTemplates {
        workspace_id: String,
    },
    /// Models a session can launch on, recently selected first, then catalog
    /// order, each with the reasoning-effort rungs it accepts.
    ListModels,
    /// Remember an explicit picker selection in the shared model history.
    RememberModel {
        selection: Selection,
    },
    ResumeSession(ResumeSession),
    /// Create and associate a linked Git worktree for an existing Session.
    /// Refused when it already has one; the association is used on its next
    /// launch or resume.
    CreateSessionWorktree {
        id: String,
    },
    /// Remove the linked Git worktrees of Sessions the operator has finished
    /// with and whose checkouts have nothing uncommitted in them, and record
    /// each such Session as working on its branch alone.
    ///
    /// The branch is never touched: the work an agent committed stays exactly
    /// where the operator expects to find it in `git branch`, and resuming the
    /// Session checks that branch out again. What goes is the directory, which
    /// after a completed Session is a copy of the repository per conversation
    /// and nothing else.
    ///
    /// Scoped to one Workspace, or to every one when no id is given. Answers
    /// with a [`CleanedWorktree`] per Session considered, including those left
    /// alone and why.
    CleanWorktrees {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
    },
    /// List the linked Git worktrees Styra knows of, each with the Sessions
    /// that record it. Changes nothing: it is what an operator asks before
    /// deciding what [`Request::CleanWorktrees`] should take, or to find which
    /// conversation a directory under their worktree parent came from.
    ///
    /// Covers every checkout a Session records, including a branch whose
    /// directory was cleaned up, and every directory in the Workspace's
    /// worktree parent that no Session records. Scoped to one Workspace, or to
    /// every one when no id is given.
    ListWorktrees {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
    },
    /// Switch a stopped Session's native transcript to the other interactive
    /// provider, retaining its id, checkout, and journal. Records the provider
    /// change in the conversation log; the Session can then be resumed.
    ConvertSessionProvider {
        id: String,
    },
    /// Branch a stored Session's native provider transcript into a new
    /// sibling Session in the same Workspace. `history` chooses a prefix
    /// through `at_ms` or only the entry at that point; an absent cutoff keeps
    /// the whole history and is valid only for the prefix choice. The provider
    /// may also change. The source Session, native transcript, and Styra
    /// journal are left untouched.
    BranchSession {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_ms: Option<u64>,
        #[serde(default)]
        history: BranchHistory,
        /// Choose a shared source checkout or a new Git branch and worktree.
        /// Omission preserves the legacy behavior: fork a managed source
        /// checkout, otherwise share the Workspace directory. History never
        /// rewinds the working files.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkout: Option<BranchCheckout>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<Provider>,
        /// Exact model and effort for the branch; its provider must agree with
        /// `provider` when both are supplied.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<Selection>,
    },
    RenameSession(RenameSession),
    /// Ask an errand for an editable Session name, read from its first
    /// prompt — for a branch, the first one after its branch point. The
    /// Session keeps its name until a rename saves one.
    GenerateSessionName {
        id: String,
    },
    /// Replace a Session's tags. Tags live with the durable Session, so they
    /// remain available after its live interaction stops or resumes.
    SetSessionTags(SetSessionTags),
    /// All tags known to the server, alphabetically.
    ListTags,
    /// Read a Session's message box (see [`ComposerState`]). A Session nobody
    /// has typed in yet answers with an empty one.
    SessionComposer {
        id: String,
    },
    /// Replace a Session's stored message box.
    SetSessionComposer {
        id: String,
        composer: ComposerState,
    },
    /// Transcribe one host audio file and return its text.
    ///
    /// The server runs a local Whisper model in its own process. No agent
    /// provider, session, or sandbox is involved, so the operation names no
    /// selection and costs no interactive quota.
    TranscribeAudio {
        path: PathBuf,
    },
    /// Tell the server that its client has begun capturing microphone audio.
    /// Recording happens on the client's host; this notification exists so
    /// the server's stdout/log still carries the complete transcription
    /// lifecycle.
    AudioRecordingStarted,
    /// Tell the server that microphone capture has stopped and transcription
    /// is about to be requested.
    AudioRecordingStopped,
    /// Report a client-side recording failure to the server's stdout/log.
    /// Transcription failures are logged by the server directly.
    AudioTranscriptionError {
        error: String,
    },
    /// Apply one edit to the latest stored Workspace sandbox policy. Applies to
    /// launches made after it, not to interactions already running under the
    /// old one.
    ChangeWorkspaceLaunch {
        workspace_id: String,
        change: WorkspaceLaunchChange,
    },
    SendMessage {
        id: String,
        message: SendMessage,
    },
    /// Switch a live interaction onto another model, applied now and recorded
    /// with the session so reopening it keeps the switch. To change provider,
    /// stop the interaction and resume it with the new selection instead.
    SetSessionSelection {
        id: String,
        selection: Selection,
    },
    /// Change the directory used by later turns of a live interaction. The
    /// path is on the host and must stay inside the interaction's Workspace.
    SetInteractionWorkingDirectory {
        id: String,
        directory: PathBuf,
    },
    /// Keep at it after a rate limit, or stop doing so: when a plan window
    /// refuses this interaction's work, resume the Session and ask it again
    /// once the window turns over.
    ///
    /// The setting belongs to the interaction the operator is looking at —
    /// they are answering for this conversation, not for the account — and is
    /// stored with its Session, since the retry itself replaces the
    /// interaction it applied to.
    SetInteractionAutoRetry {
        id: String,
        enabled: bool,
    },
    /// Commit the work this interaction leaves in its checkout each time it
    /// goes idle, or stop doing so. Until the operator answers, it is on for a
    /// Session working in a linked checkout of its own and off for one working
    /// in the Workspace directory; the answer is stored with the Session, so
    /// resuming it keeps it.
    ///
    /// The commit is subject-lined with the start of the turn's last agent
    /// message, and its body holds the message that started the turn and
    /// every agent message in it.
    SetInteractionAutoCommit {
        id: String,
        enabled: bool,
    },
    /// Persist an operator message in the session's durable input queue
    /// without sending it yet, so it survives the client disconnecting before
    /// the interaction is idle enough to accept it.
    QueueMessage {
        id: String,
        message: SendMessage,
    },
    /// Send and remove the oldest durably queued message, if any.
    SendQueuedMessage {
        id: String,
    },
    /// Discard the session's durably queued messages.
    ClearQueuedMessages {
        id: String,
    },
    InterruptInteraction {
        id: String,
    },
    StopInteraction {
        id: String,
    },
    /// Set whether the operator is finished with a Session. Anything but
    /// [`CompletionState::Active`] stops any live interaction serving it (the
    /// row stays listed, but clients normally hide completed rows). The state
    /// lives with the Session, not the interaction, so it survives the
    /// interaction stopping and is what the stored-sessions picker filters
    /// on; resuming the Session clears it back to `Active`. Sealing is the
    /// exception in both directions: nothing sets a sealed Session back to
    /// `Active`, and a sealed Session cannot be resumed.
    SetSessionCompleted {
        id: String,
        completed: CompletionState,
    },
    /// Stop an interaction and drop the server's record of it, so the Session
    /// is only what is stored on disk: it no longer appears in the
    /// current-interactions list and can be resumed like any other history.
    CloseInteraction {
        id: String,
    },
    LoadInteraction {
        id: String,
    },
    Updates {
        id: String,
        after: u64,
        /// Include `InteractionUpdate::Raw` wire lines. Clients with no raw
        /// view (a preview pane) pass `false` and skip the bulk of a long
        /// interaction's volume. Defaults to `true`, which is what a client
        /// that predates the field expects.
        #[serde(default = "yes")]
        raw: bool,
    },
    /// The interactions the server lists — its own and those a previous run
    /// left open — that the query admits, newest first. A newtype over an
    /// `Option` rather than a struct variant so that a request with no `data`
    /// at all, which is what this was before it took a query, still parses,
    /// and lists everything.
    ListInteractions(Option<InteractionQuery>),
    ListSessions {
        workspace_id: String,
    },
    StoredSession {
        id: String,
        /// Include the journal's verbatim wire lines. Reconstructing them
        /// re-reads the whole journal, so a caller that only renders decoded
        /// events passes `false`. Defaults to `true` for older clients.
        #[serde(default = "yes")]
        raw: bool,
    },
    /// Read the provider-native, resumable session JSONL for this Session.
    ProviderRaw {
        id: String,
    },
    Shell {
        id: String,
    },
    /// Start a project command in the live session's sandbox, replacing the
    /// same action's previous run if it is still going.
    StartProjectAction {
        id: String,
        action: ProjectAction,
        /// The command to run instead of the one the session's own launch
        /// policy, its Workspace, or its templates state — the client's own
        /// interaction-level override, which it may have changed since the
        /// launch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    /// Parse the session's most recent agent message under the contract its
    /// last typed turn was sent with, and return the typed value.
    ///
    /// Separate from sending, rather than a reply to it, because a turn takes
    /// minutes: the client polls [`Request::Updates`] as it would for any
    /// session and asks for the answer once the turn completes. It reads the
    /// same journal the interface renders, so it works on a live interaction
    /// and a stored session alike — an answer can be re-parsed long after the
    /// interaction it came from has ended.
    TurnAnswer {
        id: String,
        /// Parse under this contract instead of the session's recorded one.
        /// Lets a client re-read an answer as another shape without asking
        /// again, and read one from a session that was never typed at all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contract: Option<Contract>,
    },
    /// Read the server's in-memory log of plan-quota readings seen on any
    /// interaction's wire, oldest first. Server-wide because quota belongs to
    /// the account rather than to one session, and in-memory because it is a
    /// live reading rather than a record: it starts empty with the daemon.
    QuotaLog,
    /// Read this interaction's durable action history, including earlier runs.
    InteractionActions {
        id: String,
    },
    /// List the processes running in this interaction's sandbox right now:
    /// every process in the sandbox's PID namespace, the agent and whatever it
    /// started, including daemons that left their parent. A snapshot, not a
    /// stream; clients showing it ask again to keep it current.
    InteractionProcesses {
        id: String,
    },
    /// Read the Driva policy a listed interaction was launched under: what its
    /// agent runs, and what it can see and write. Asked of one interaction
    /// when a client shows it, rather than carried on every listed row.
    InteractionDriva {
        id: String,
    },
    /// Local comparison branches, newest tip commit first.
    DiffBranches {
        id: String,
    },
    /// Save the comparison branch; None restores the automatic baseline.
    SetDiffBranch {
        id: String,
        branch: Option<String>,
    },
    /// What checkout diff mode compares the working tree with: the saved
    /// comparison branch’s merge base with HEAD, or by default where the
    /// interaction's branch would be integrated — the merge base of its
    /// checkout and the branch it targets, which a rebase moves on — or, for
    /// a checkout with no target branch, its first snapshot rebuilt as a Git
    /// tree, uncommitted work at the time included.
    DiffBase {
        id: String,
    },
    /// What one operator turn changed in the checkout, read from the
    /// snapshots taken as it started and as it ended — everything, whoever
    /// wrote it, rather than what the agent reported writing.
    ///
    /// Every interaction working in a Git checkout has an initial snapshot
    /// recorded before the provider starts, then as each turn ends and before
    /// each operator message is sent. A snapshot names
    /// the commit checked out and, when `git status` reports anything, carries
    /// the diff from that commit to the working tree, untracked files
    /// included. The turn-end snapshot is taken after any auto-commit of the
    /// turn; the one before a message is skipped when the checkout is exactly
    /// as the last snapshot recorded it. Snapshots are appended to the
    /// Session's durable directory and outlive the interaction.
    ///
    /// A turn that has not ended yet — the latest one, still running — is
    /// answered from the most recent snapshot to the checkout as it is now,
    /// so asking again shows the turn's progress.
    ///
    /// `turn` counts the Session's operator messages from 1, as a client
    /// listing them counts them; for a branch that includes the inherited
    /// ones. Answered for a stored Session as for a live one.
    SnapshotTurnDiff {
        id: String,
        turn: u64,
    },
    /// Ask the server to remove its socket and exit. Any live interactions it owns die
    /// with it, so this is the deliberate counterpart to the daemon outliving
    /// its clients.
    Shutdown,
}

/// A server action, captured at the common dispatch boundary. The original
/// request preserves every input, including fields added by future operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Action {
    Request(Request),
    CommitTurn { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOrigin {
    Client,
    Automatic,
}

/// Started without a terminal record means the server stopped before it could
/// report an outcome; it must never be interpreted as success.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Started,
    Succeeded,
    Failed,
}

/// An append-only action lifecycle record. Records sharing `id` describe one
/// invocation. Sessions created by an action are linked by its terminal record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub id: String,
    pub at_ms: u64,
    pub sessions: Vec<String>,
    pub action: Action,
    pub origin: ActionOrigin,
    pub status: ActionStatus,
    pub detail: Option<String>,
}

/// Versioned request envelope. Flattening keeps `operation` at the top level.
/// Successful response payload. The variant must match the request operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
// Like WireResponse, this is built once per request and immediately serialised;
// boxing its payloads would add allocations to that path.
#[allow(clippy::large_enum_variant)]
pub enum Response {
    RebaseStarted,
    SessionBranchHistory(Vec<SavedBranch>),
    SessionRebaseStatus(RebaseStatus),
    IntegrationStarted,
    IntegrationMessage(String),
    EditReport(Option<EditReport>),
    SessionIntegrationResult(Option<IntegrationResult>),
    Health(Health),
    WorkspaceCreated(WorkspaceSummary),
    Workspaces(Vec<WorkspaceSummary>),
    Workspace(WorkspaceSummary),
    /// The Workspace covering a host directory, or nothing if none does.
    WorkspaceForPath(Option<WorkspaceSummary>),
    WorkspaceRenamed(WorkspaceSummary),
    WorkspaceGitRepositoryUpdated(WorkspaceSummary),
    WorkspaceHostPathUpdated(WorkspaceSummary),
    WorkspaceLaunch(LaunchPolicy),
    SessionCreated(SessionInfo),
    SessionPlan(DrivaOptions),
    Templates(Vec<TemplateSummary>),
    Models(Vec<ModelSummary>),
    ModelRemembered,
    SessionResumed(SessionInfo),
    SessionWorktreeCreated,
    WorktreesCleaned(Vec<CleanedWorktree>),
    Worktrees(Vec<WorktreeSummary>),
    SessionConverted(SessionSummary),
    SessionBranched(SessionSummary),
    SessionRenamed(SessionSummary),
    SessionName(String),
    SessionTagsUpdated(SessionSummary),
    Tags(Vec<String>),
    SessionComposer(ComposerState),
    SessionComposerStored,
    WorkspaceLaunchUpdated(LaunchPolicy),
    Accepted,
    Queued(usize),
    SentQueuedMessage(Option<QueuedMessage>, Vec<QueuedMessage>),
    QueuedMessages(Vec<QueuedMessage>),
    InteractionLoaded(LoadedInteraction),
    Updates(Updates),
    Interactions(Vec<InteractionSummary>),
    StoredSessions(Vec<SessionSummary>),
    StoredSession(StoredSession),
    ProviderRaw(ProviderRaw),
    AudioTranscript(String),
    Shell(ShellInfo),
    ProjectActionStarted(ProjectActionSession),
    Answer(Answer),
    QuotaLog(Vec<QuotaEvent>),
    InteractionActions(Vec<ActionRecord>),
    /// `None` when no sandbox is running for the Session: it is stopped, or
    /// its interaction is still starting.
    InteractionProcesses(Option<Vec<SandboxProcess>>),
    InteractionDriva(DrivaOptions),
    /// `None` when the turn has no snapshots to compare: it ran outside a
    /// Git checkout, a snapshot of it failed, or it predates snapshots.
    SnapshotTurnDiff(Option<TurnDiff>),
    /// `None` when the Session has neither a target branch nor snapshots.
    DiffBase(Option<DiffBase>),
    DiffBranches(Vec<String>),
}

/// Response envelope returned for every syntactically valid connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
// One envelope is built per request and immediately serialised, so boxing the
// success payload would only trade a stack copy for an allocation.
#[allow(clippy::large_enum_variant)]
pub enum WireResponse {
    Ok { response: Response },
    Error { error: String },
}

#[cfg(test)]
mod tests;
