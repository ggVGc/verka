//! The data vocabulary that crosses the Styra socket boundary.
//!
//! These are the types a client receives and renders: the live update stream
//! ([`InteractionUpdate`] and its parts), the captured Driva policy
//! ([`DrivaOptions`]), and the stored-session listing ([`SessionSummary`]).
//! They carry no behaviour tied to running an interaction — the server machinery
//! that produces them lives in [`crate::interaction`], [`crate::journal`], and
//! [`crate::server`]. Keeping them here lets a client depend on the interface
//! without pulling in the interaction runner.

use crate::agent::Selection;
use crate::event::AgentEvent;
use driva::Mount;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A durable Styra Workspace, which groups provider Sessions that operate on
/// the same host directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    /// The Workspace directory name and stable wire identifier.
    pub id: String,
    /// Optional operator-facing name. The host directory name is the display
    /// fallback when this is absent.
    pub name: Option<String>,
    /// Canonical host directory mounted into Sessions in this Workspace.
    pub host_path: PathBuf,
    /// Canonical root of the Git checkout associated with this Workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_repository: Option<PathBuf>,
    /// Whether launches in this Workspace expose Styra's linked-worktree
    /// creation tool. Off by default; worktrees are an opt-in capability.
    #[serde(default)]
    pub worktrees_enabled: bool,
    /// Directory holding `workspace.json` and this Workspace's Sessions.
    pub path: PathBuf,
    /// Number of durable Sessions currently stored in the Workspace.
    pub session_count: usize,
    /// Roughly how long ago the Workspace was created.
    pub age: String,
    /// Millisecond creation timestamp retained for display and tie-breaking.
    pub created_at_ms: u64,
    /// Millisecond timestamp of the most recent explicit access.
    #[serde(default)]
    pub last_accessed_at_ms: u64,
    /// The Workspace's standing sandbox policy: what every launch here starts
    /// from before an individual launch adds to it. Empty until an operator
    /// stores one.
    #[serde(default, skip_serializing_if = "LaunchPolicy::is_empty")]
    pub launch: LaunchPolicy,
}

/// An update delivered from the interaction's threads to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum InteractionUpdate {
    /// A decoded agent event or an operator message, in occurrence order.
    Event(AgentEvent),
    /// One verbatim wire line, for the raw-interaction view.
    Raw(RawLine),
    /// A diagnostic message for the log view.
    Log(LogEntry),
    /// A plan-quota window worth the operator's attention — nearly full, or
    /// full. Only readings that say something new are sent; every reading,
    /// notable or not, is kept in the server's quota log for
    /// [`crate::protocol::Request::QuotaLog`] to return.
    Quota(QuotaEvent),
    /// The host directory used for subsequent agent turns.
    WorkingDirectoryChanged(PathBuf),
    /// The agent process ended; no further events will arrive.
    Ended(InteractionEnd),
}

/// Severity of a [`LogEntry`], used to colour the log view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line in the log view: a Styra-internal note or a line of agent stderr.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Info,
            message: message.into(),
        }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Warn,
            message: message.into(),
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Error,
            message: message.into(),
        }
    }
}

/// How much of a quota window is left, as the provider judges it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaStatus {
    /// Room remains, and the provider is not warning about the window.
    Allowed,
    /// The window is close enough to full that the provider says so, or its
    /// usage has passed [`crate::quota::WARN_THRESHOLD`].
    Warning,
    /// The window is full: turns are being refused until it resets.
    Exhausted,
}

/// One reading of a plan quota window, as a provider reported it mid-session.
///
/// Both interactive providers volunteer these unprompted, in different shapes
/// and with different amounts of detail — see [`crate::quota`], which reads
/// them off the wire and keeps them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuotaEvent {
    /// When the reading was seen, in milliseconds since the epoch.
    pub at_ms: u64,
    /// The interaction whose wire carried it. Quota is account-wide, so this
    /// says where the reading came from, not what it applies to.
    pub session_id: String,
    /// The window the reading is about, as the provider names it
    /// (`five_hour`) or as its length (`1h`, `7d`).
    pub window: String,
    pub status: QuotaStatus,
    /// How full the window is, as a fraction. `None` when the provider
    /// withholds it — Claude sends a figure only once it has something to warn
    /// about, so a permitted reading genuinely does not say how full it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization: Option<f64>,
    /// When the window resets, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
    /// Whatever else the provider said about the limit, e.g.
    /// `out_of_credits` for a rejected overage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl QuotaEvent {
    /// Whether this reading is worth telling the operator about unprompted,
    /// rather than only recording for them to look up.
    pub fn is_notable(&self) -> bool {
        match self.status {
            QuotaStatus::Allowed => false,
            QuotaStatus::Warning | QuotaStatus::Exhausted => true,
        }
    }

    /// The reading as one line of prose. Carries no timestamp: the reset is a
    /// moment, and only the caller knows what "now" is to measure it against.
    pub fn describe(&self) -> String {
        let mut line = match (self.status, self.utilization) {
            (QuotaStatus::Exhausted, _) => {
                format!("plan quota exhausted: the {} window is full", self.window)
            }
            (_, Some(utilization)) => format!(
                "plan quota: the {} window is {:.0}% used",
                self.window,
                utilization * 100.0
            ),
            (_, None) => format!("plan quota: the {} window is nearly full", self.window),
        };
        if let Some(detail) = &self.detail {
            line.push_str(&format!(" ({detail})"));
        }
        line
    }

    /// The percentage as the views show it, or `?` where the provider gave no
    /// figure.
    pub fn utilization_label(&self) -> String {
        match self.utilization {
            Some(utilization) => format!("{:.0}%", utilization * 100.0),
            None => "?".into(),
        }
    }
}

/// Which way a wire line travelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// A line Styra wrote to the agent's stdin.
    ToAgent,
    /// A line received on the agent's stdout.
    FromAgent,
}

/// One verbatim line of the agent interaction, undecoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLine {
    pub direction: Direction,
    pub text: String,
    /// When this line was recorded. Lets a client resolve "branch the
    /// session here" for a selected entry back to a moment in time, which is
    /// compared against the native provider transcript's own timestamps to
    /// decide how much of the history a branch keeps — the two are decoded
    /// differently (Styra's journal vs. the provider's own file) and cannot
    /// otherwise be lined up. Defaults to `0` for a line from a client that
    /// predates this field, which is not a moment any real branch resolves to.
    #[serde(default)]
    pub at_ms: u64,
}

/// How an interaction finished.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionEnd {
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

/// A human-facing summary of the Driva policy an interaction was launched with:
/// the isolation backend, the command it runs, and the mount/network policy
/// enforced around it. Captured once at spawn time from the same
/// `ExecutionRequest` Driva itself executes (see [`DrivaOptions::capture`] in
/// [`crate::interaction`]), so it can never drift from what is actually running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrivaOptions {
    pub isolation_backend: String,
    pub command: Vec<String>,
    pub working_directory: PathBuf,
    pub network: bool,
    pub mounts: Vec<AttributedMount>,
}

impl DrivaOptions {
    /// The mounts alone, for the questions that only care about what the
    /// sandbox holds and not about who asked for it.
    pub fn plain_mounts(&self) -> Vec<Mount> {
        self.mounts
            .iter()
            .map(|mount| mount.mount.clone())
            .collect()
    }
}

/// Which layer of the launch policy put a mount in the sandbox.
///
/// The effective policy is one flat list by the time Driva runs it, but the
/// layers that produced it are not interchangeable to an operator: a grant from
/// the agent profile is a property of the agent they picked, one from a
/// template is a property of the template, and one they typed is theirs to take
/// back. Recording the layer at the point the list is built is the only place
/// the answer is known for certain — afterwards a mount is just a path pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountOrigin {
    /// The operator's project, bound writable as the agent's workspace.
    Workspace,
    /// The Git checkout durably associated with the Workspace.
    GitRepository,
    /// An empty writable filesystem discarded when the run ends.
    Scratch,
    /// Granted by the agent profile — its credentials, tools, and caches.
    Profile,
    /// Granted by one of the selected Driva templates.
    Template,
    /// Asked for by hand, through the launch policy's mount key.
    Operator,
    /// The hidden control mount the sandbox broker needs for its tmux shell.
    Broker,
}

impl MountOrigin {
    /// How the origin reads as a heading over the mounts it contributed.
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::GitRepository => "git repository",
            Self::Scratch => "scratch",
            Self::Profile => "agent profile",
            Self::Template => "templates",
            Self::Operator => "your mounts",
            Self::Broker => "broker control",
        }
    }
}

/// A resolved mount together with the layer that asked for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributedMount {
    pub origin: MountOrigin,
    pub mount: Mount,
}

/// One extra host directory the operator asked to be bound into the sandbox,
/// on top of what the profile and the selected templates already grant.
///
/// This is a *request*, not a resolved mount: the source is whatever the
/// operator typed, and the server canonicalizes it (rejecting a path that does
/// not exist) before it becomes part of a launch. An absent `destination`
/// means "the same path inside the sandbox", matching Driva's own rule for a
/// bind mount with no destination.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchMount {
    pub source: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    /// Read-only unless the operator explicitly asked for write access, so the
    /// quiet default is the one that grants least.
    #[serde(default)]
    pub writable: bool,
}

/// The sandbox policy inputs a launch asks for beyond the agent selection.
///
/// The same type serves two roles, and the difference is only where it is
/// stored. A Workspace holds one as its *standing* policy — what every launch
/// there starts from ([`WorkspaceSummary::launch`]). A launch request carries
/// one as its own *overlay* — what this interaction adds to, or says instead
/// of, the Workspace's. [`LaunchPolicy::merge`] is the only place the two are
/// combined, and the server merges them for `create_session`, `plan_session`
/// and `resume_session` alike, so a plan cannot disagree with the launch it
/// describes.
///
/// Nothing here is resolved: `templates` are names and `mounts` are requests.
/// The server resolves both against the Workspace's `driva.toml` and the host
/// filesystem after merging.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchPolicy {
    /// Whether agent networking is permitted. `None` inherits — from the
    /// Workspace's policy on an overlay, and from nothing at all on the
    /// Workspace's own, which then leaves the decision to the profile and the
    /// templates. `Some` states it for this layer and wins over the layer
    /// below. Even `Some(false)` only withdraws *this* permission: the server
    /// ORs the profile's and the templates' own network policy in afterwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<bool>,
    /// Named Driva templates, layered in the order given (later names win on
    /// conflict). On an overlay these are appended after the Workspace's,
    /// so an interaction's template wins over the Workspace's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<String>,
    /// Host directories to bind in on top of the workspace mount. On an
    /// overlay these are added to the Workspace's, except that one landing on
    /// the same destination replaces it rather than colliding with it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<LaunchMount>,
    /// On an overlay: ignore the Workspace's standing policy entirely rather
    /// than adding to it. This is how a single interaction drops a template or
    /// a mount the Workspace grants, which appending alone cannot express.
    /// Meaningless on a Workspace's own policy, which has nothing below it.
    #[serde(skip_serializing_if = "is_false")]
    pub standalone: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl LaunchPolicy {
    /// Whether this policy asks for anything at all. A Workspace with an empty
    /// policy is indistinguishable from one that never stored a policy, which
    /// is why the field is skipped on the wire when it is empty.
    pub fn is_empty(&self) -> bool {
        self.network.is_none()
            && self.templates.is_empty()
            && self.mounts.is_empty()
            && !self.standalone
    }

    /// The single policy a launch runs under: the Workspace's `base` with one
    /// launch's `overlay` layered over it.
    ///
    /// Additive by default, because that is what a standing policy is for: the
    /// Workspace grants the mounts and templates every launch there needs, and
    /// an interaction says what is particular to it. An overlay overrides
    /// rather than adds in exactly three ways — a stated `network`, a template
    /// name repeating one of the Workspace's (which moves it later in the
    /// layering, where it wins), and a mount on a destination the Workspace
    /// already binds. `standalone` is the escape hatch for the case none of
    /// those cover: dropping something the Workspace grants.
    ///
    /// The result is always additive-shaped (`standalone` cleared): it is a
    /// resolved policy, with nothing left below it to ignore.
    pub fn merge(base: &Self, overlay: &Self) -> Self {
        if overlay.standalone {
            return Self {
                standalone: false,
                ..overlay.clone()
            };
        }
        let mut merged = Self {
            network: overlay.network.or(base.network),
            templates: base.templates.clone(),
            mounts: base.mounts.clone(),
            standalone: false,
        };
        for name in &overlay.templates {
            merged.templates.retain(|existing| existing != name);
            merged.templates.push(name.clone());
        }
        for mount in &overlay.mounts {
            merged
                .mounts
                .retain(|existing| !same_target(existing, mount));
            merged.mounts.push(mount.clone());
        }
        merged
    }

    /// Whether this policy asks for networking. An absent answer is "no" here:
    /// the profile and the templates are ORed in by the caller, so this only
    /// ever reports what the operator asked for.
    pub fn grants_network(&self) -> bool {
        self.network.unwrap_or(false)
    }
}

/// Whether two mount requests would land in the same place inside the sandbox.
///
/// Compared before canonicalization, on the paths as asked for, because that
/// is what a merge has to work with — an overlay is layered before the server
/// resolves anything. A mount naming no destination lands on its own source,
/// matching Driva's rule, so that is what stands in for it. Two requests that
/// only collide once resolved (via a symlink, say) are not caught here; they
/// are still refused at plan and launch time by `ensure_distinct_destinations`.
fn same_target(left: &LaunchMount, right: &LaunchMount) -> bool {
    left.destination.as_ref().unwrap_or(&left.source)
        == right.destination.as_ref().unwrap_or(&right.source)
}

/// A Driva execution template the server can offer, named and described, so a
/// client can present the real set rather than asking the operator to recall
/// template names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// What a live interaction is currently waiting on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionActivity {
    /// The agent is idle and waiting for the operator's next message.
    #[default]
    Pending,
    /// The agent is working on an operator message.
    Running,
    /// The agent is waiting for input while a background task is active.
    Background,
}

/// An interaction the server is currently running (this process's live sessions),
/// enough to list it and to reattach a client to it. Distinct from
/// [`SessionSummary`], which describes a session persisted in the store
/// whether or not it is still live.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionSummary {
    /// The session id, as used everywhere else on the wire.
    pub id: String,
    /// Optional operator-facing Session name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The Workspace containing the Session served by this Interaction.
    pub workspace_id: String,
    /// The provider, model, and effort the interaction is running.
    pub selection: Selection,
    /// The host directory bound as the agent's workspace, so a reattaching
    /// client can resolve changed-file previews.
    pub workspace: PathBuf,
    /// The Driva policy the interaction was launched under, for the driva view.
    pub driva: DrivaOptions,
    /// Whether the interaction's agent process is alive and still takes messages.
    pub accepting: bool,
    /// Whether the live interaction is working or waiting for user input.
    #[serde(default)]
    pub activity: InteractionActivity,
    /// The most recent message the agent sent, flattened to a single line and
    /// clipped, so a list of interactions says what each one is actually
    /// talking about. `None` before the agent has said anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
}

/// Where a Session came from, when it was not launched fresh but branched
/// from another one — see [`crate::server::ServerState::branch_session`].
/// Recorded once, at branch time; it never updates as the source Session
/// keeps being worked on afterwards, the same way a git branch's fork point
/// does not move when the source gets new commits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOrigin {
    /// The Session this one was branched from.
    pub session_id: String,
    /// The provider the source Session was running under at branch time.
    pub provider: crate::agent::Provider,
    /// The moment in the source's history the branch was taken at, matching
    /// a [`RawLine::at_ms`] the operator selected. `None` means the branch
    /// kept the whole history — what a plain provider conversion is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
}

/// A stored session, enough to display and select it from a list — see
/// [`crate::journal::list_sessions`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// The session's directory name, and the id `--view` expects.
    pub id: String,
    /// Optional operator-facing name; the stable id remains the identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The owning Workspace.
    pub workspace_id: String,
    /// Its directory, ready to pass straight to `--view`.
    pub path: PathBuf,
    /// The provider, model, and effort that produced it.
    pub selection: Selection,
    /// Roughly how long ago it was created, e.g. "3h ago".
    pub age: String,
    /// The millisecond timestamp embedded in `id`, used to sort newest
    /// first; `None` for an id that doesn't match the expected shape.
    pub created_at_ms: Option<u64>,
    /// Set when this Session was branched from another one, rather than
    /// launched fresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SessionOrigin>,
}

/// An operator message persisted but not yet sent.
///
/// Carries the contract it was queued under, so a message the operator asked
/// for a shape while the agent was busy still asks for it when the queue
/// drains. Without that the shape would be dropped at exactly the moment the
/// operator could least do anything about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueuedMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
}

impl QueuedMessage {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            contract: None,
        }
    }

    pub fn asking_for(mut self, contract: Option<Contract>) -> Self {
        self.contract = contract;
        self
    }
}

/// Queues written before contracts existed are arrays of bare strings. They
/// are read as untyped messages rather than failing the resume that finds
/// them, which would strand the operator's unsent work.
impl<'de> Deserialize<'de> for QueuedMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Text(String),
            Message {
                text: String,
                #[serde(default)]
                contract: Option<Contract>,
            },
        }
        Ok(match Stored::deserialize(deserializer)? {
            Stored::Text(text) => Self::new(text),
            Stored::Message { text, contract } => Self { text, contract },
        })
    }
}

/// The shape a client asks a turn's answer to come back in.
///
/// A contract is applied at both ends of one turn: it frames the message sent
/// to the agent with instructions describing the shape, and it parses the
/// agent's reply back into [`AnswerValue`]. Framing server-side is what keeps
/// clients honest — every caller asks for a shape the same way, so the parser
/// only has to understand one phrasing. See [`crate::contract`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Contract {
    /// Prose. The weakest contract, and the one that cannot fail to parse.
    Text,
    /// One item per line.
    Lines,
    /// One file location per line, `path[:line[:column]][: description]`.
    Files,
    /// A single JSON value.
    Json,
}

impl Contract {
    /// The wire spelling, for diagnostics and client-side display.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Lines => "lines",
            Self::Files => "files",
            Self::Json => "json",
        }
    }
}

/// A place in the Workspace, as named by a [`Contract::Files`] answer.
///
/// `line` and `column` are 1-based and `None` when the agent named none, which
/// is the difference between "this file" and "this position", and is why they
/// are not zero-defaulted into a position that does not exist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLocation {
    /// Relative to the Workspace root, as the agent was asked to report it.
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// The agent's note about this location; empty when it gave none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl FileLocation {
    /// `path`, `path:line`, or `path:line:column` — the spelling the agent was
    /// asked for, and the one an editor's jump-to-location expects back.
    pub fn located(&self) -> String {
        let mut text = self.path.display().to_string();
        if let Some(line) = self.line {
            text.push(':');
            text.push_str(&line.to_string());
            if let Some(column) = self.column {
                text.push(':');
                text.push_str(&column.to_string());
            }
        }
        text
    }
}

/// A parsed answer. The variant always matches the [`Contract`] that produced
/// it, so a client can dispatch on the value alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "contract", content = "value", rename_all = "snake_case")]
pub enum AnswerValue {
    Text(String),
    Lines(Vec<String>),
    Files(Vec<FileLocation>),
    Json(serde_json::Value),
}

impl AnswerValue {
    /// The contract this value was parsed under.
    pub fn contract(&self) -> Contract {
        match self {
            Self::Text(_) => Contract::Text,
            Self::Lines(_) => Contract::Lines,
            Self::Files(_) => Contract::Files,
            Self::Json(_) => Contract::Json,
        }
    }
}

/// One turn's typed answer.
///
/// A reply that did not satisfy its contract is an [`Answer`] too, not an error
/// in place of one: `value` is absent, `error` says what was wrong, and
/// `source` still carries what the agent actually said. An agent that answered
/// well but framed it badly has produced something worth reading, and a client
/// that was handed only "no answer block" could not show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The contract the reply was read under, whether or not it satisfied it.
    pub contract: Contract,
    /// The parsed value; absent when the reply did not satisfy `contract`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<AnswerValue>,
    /// Why the reply could not be read as `contract`; absent on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The agent message this was read from, verbatim.
    pub source: String,
}

impl Answer {
    /// Whether the reply satisfied its contract.
    pub fn is_parsed(&self) -> bool {
        self.value.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(source: &str, destination: Option<&str>, writable: bool) -> LaunchMount {
        LaunchMount {
            source: PathBuf::from(source),
            destination: destination.map(PathBuf::from),
            writable,
        }
    }

    /// The ordinary case: the Workspace says what every launch here needs, the
    /// launch says what is particular to it, and the sandbox gets both.
    #[test]
    fn a_launch_adds_to_the_workspace_policy_by_default() {
        let base = LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![mount("/srv/corpus", None, false)],
            standalone: false,
        };
        let overlay = LaunchPolicy {
            templates: vec!["browser".into()],
            mounts: vec![mount("/srv/scratch", None, true)],
            ..LaunchPolicy::default()
        };

        let merged = LaunchPolicy::merge(&base, &overlay);
        assert_eq!(merged.templates, vec!["rust", "browser"]);
        assert_eq!(
            merged.mounts,
            vec![
                mount("/srv/corpus", None, false),
                mount("/srv/scratch", None, true)
            ]
        );
        // Inherited, because the overlay said nothing about it.
        assert_eq!(merged.network, Some(true));
        assert!(merged.grants_network());
    }

    /// Overriding, in the three ways an additive overlay can express it: a
    /// stated network answer, a template name that moves later in the layering,
    /// and a mount on a destination the Workspace already binds.
    #[test]
    fn an_overlay_overrides_the_workspace_on_the_grants_it_names() {
        let base = LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into(), "browser".into()],
            mounts: vec![mount("/srv/corpus", Some("/mnt/corpus"), false)],
            standalone: false,
        };
        let overlay = LaunchPolicy {
            network: Some(false),
            templates: vec!["rust".into()],
            mounts: vec![mount("/srv/other", Some("/mnt/corpus"), true)],
            standalone: false,
        };

        let merged = LaunchPolicy::merge(&base, &overlay);
        assert_eq!(merged.network, Some(false));
        // Naming a template the Workspace already layers moves it last, where
        // it wins on conflict, rather than duplicating it.
        assert_eq!(merged.templates, vec!["browser", "rust"]);
        // One destination, bound to what this launch asked for: the two would
        // otherwise collide and be refused outright.
        assert_eq!(
            merged.mounts,
            vec![mount("/srv/other", Some("/mnt/corpus"), true)]
        );
    }

    /// A mount naming no destination lands on its own source, so that is what
    /// an overlay has to match to replace it.
    #[test]
    fn a_mount_with_no_destination_is_replaced_through_its_source() {
        let base = LaunchPolicy {
            mounts: vec![mount("/srv/corpus", None, false)],
            ..LaunchPolicy::default()
        };
        let overlay = LaunchPolicy {
            mounts: vec![mount("/srv/corpus", None, true)],
            ..LaunchPolicy::default()
        };

        let merged = LaunchPolicy::merge(&base, &overlay);
        assert_eq!(merged.mounts, vec![mount("/srv/corpus", None, true)]);
    }

    /// Dropping a grant the Workspace makes cannot be said by adding to it, so
    /// `standalone` is the one way a launch says "not that policy, this one".
    #[test]
    fn a_standalone_launch_ignores_the_workspace_policy_entirely() {
        let base = LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![mount("/srv/corpus", None, false)],
            standalone: false,
        };
        let overlay = LaunchPolicy {
            templates: vec!["browser".into()],
            standalone: true,
            ..LaunchPolicy::default()
        };

        let merged = LaunchPolicy::merge(&base, &overlay);
        assert_eq!(merged.templates, vec!["browser"]);
        assert!(merged.mounts.is_empty());
        assert_eq!(merged.network, None);
        assert!(!merged.grants_network());
        // The result is a resolved policy: there is nothing left below it.
        assert!(!merged.standalone);
    }

    /// A launch that asks for nothing runs under exactly the Workspace's
    /// policy — the property that makes a standing policy worth storing.
    #[test]
    fn an_empty_overlay_leaves_the_workspace_policy_as_it_is() {
        let base = LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![mount("/srv/corpus", None, false)],
            standalone: false,
        };
        assert!(LaunchPolicy::default().is_empty());
        assert_eq!(LaunchPolicy::merge(&base, &LaunchPolicy::default()), base);
    }
}
