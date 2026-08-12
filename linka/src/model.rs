//! The records Linka stores and the state it derives from them.
//!
//! A node separates structured data from prose: `node.toml` and
//! `description.md` form its definition, `result.toml` and `result.md` its one
//! result. Nothing here stores a status: [`NodeState`] and its four dimensions
//! are computed from these records by [`crate::graph`] on every read.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const DEFINITION_SCHEMA: u32 = 1;
pub const RESULT_SCHEMA: u32 = 1;
pub const SNAPSHOT_SCHEMA: u32 = 1;
pub const OBSERVATION_SCHEMA: u32 = 1;
pub const ATTACHMENT_SCHEMA: u32 = 1;
pub const CANDIDATE_SCHEMA: u32 = 1;

// --- validated identifiers ---------------------------------------------------

/// The one rule for every portable path component the store writes: node ids,
/// project path components, attachment namespaces and keys. It is deliberately
/// stricter than any single platform, because a store is meant to be cloned
/// onto all of them.
pub fn component(value: &str) -> Result<&str, String> {
    if value.is_empty() {
        return Err("name must not be empty".into());
    }
    if value.contains(['/', '\\', ':']) {
        return Err(format!("name `{value}` must not contain `/`, `\\` or `:`"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!(
            "name `{value}` must not contain control characters"
        ));
    }
    if value == "." || value == ".." {
        return Err(format!("`{value}` is not a usable name"));
    }
    if value.eq_ignore_ascii_case(".git") {
        return Err("`.git` must never be addressed as graph content".into());
    }
    if is_windows_reserved(value) {
        return Err(format!("`{value}` is a reserved device name"));
    }
    if value.ends_with('.') || value.ends_with(' ') {
        return Err(format!("name `{value}` must not end with `.` or a space"));
    }
    Ok(value)
}

/// Whether a name is one of the DOS device names Windows still refuses to
/// create a file for, with or without an extension.
///
/// Every comparison here is on bytes rather than string slices: this runs on
/// names read off disk, and slicing a stem at a fixed index would panic the
/// moment one of them held a multi-byte character.
fn is_windows_reserved(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or(value).as_bytes();
    const NAMES: [&[u8]; 4] = [b"CON", b"PRN", b"AUX", b"NUL"];
    if NAMES.iter().any(|name| stem.eq_ignore_ascii_case(name)) {
        return true;
    }
    let [prefix @ .., digit] = stem else {
        return false;
    };
    (prefix.eq_ignore_ascii_case(b"COM") || prefix.eq_ignore_ascii_case(b"LPT"))
        && digit.is_ascii_digit()
        && *digit != b'0'
}

/// A validated string identifier: a tuple struct over `String` with the usual
/// `as_str`/`Display`/`AsRef<str>`/`String` conversions and a `FromStr` that
/// runs `$validate` (an `fn(&str) -> Result<String, String>`, returning the
/// string to store — letting it normalize, not just check) on construction.
/// Serde round-trips through `String`, so validation runs on deserialization
/// too.
macro_rules! validated_string {
    ($name:ident, $validate:path) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                value.parse()
            }
        }
        impl FromStr for $name {
            type Err = String;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self($validate(value)?))
            }
        }
    };
}

validated_string!(NodeId, validate_component);
validated_string!(CandidateId, validate_candidate_id);
validated_string!(ProjectPath, validate_project_path);
validated_string!(Namespace, validate_component);
validated_string!(AttachmentKey, validate_component);

fn validate_component(value: &str) -> Result<String, String> {
    component(value).map(str::to_owned)
}

fn validate_candidate_id(value: &str) -> Result<String, String> {
    component(value)?;
    if !value.starts_with("candidate-") {
        return Err(format!(
            "candidate id `{value}` must start with `candidate-`"
        ));
    }
    Ok(value.into())
}

impl CandidateId {
    pub fn new() -> Self {
        Self(format!("candidate-{}", ulid::Ulid::new()))
    }
}

impl Default for CandidateId {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeId {
    /// Mint a fresh node id. Well-formed by construction.
    pub fn new() -> Self {
        Self(format!("node-{}", ulid::Ulid::new()))
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for NodeId {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

/// A path inside the paired project, normalized to `/` separators. Every
/// component obeys [`component`], so no project path can address `.git` or
/// traverse out of the project root.
fn validate_project_path(value: &str) -> Result<String, String> {
    let normalized = value.replace('\\', "/");
    if normalized.is_empty() {
        return Err("project path must not be empty".into());
    }
    if normalized.starts_with('/') {
        return Err(format!("project path `{value}` must be relative"));
    }
    for part in normalized.split('/') {
        component(part)?;
    }
    Ok(normalized)
}

impl PartialEq<str> for ProjectPath {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for ProjectPath {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl AsRef<std::path::Path> for ProjectPath {
    fn as_ref(&self) -> &std::path::Path {
        std::path::Path::new(self.as_str())
    }
}

// --- definitions ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Human,
    Machine,
}
impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Machine => "machine",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    DependsOn,
    DerivedFrom,
}
impl DepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on",
            Self::DerivedFrom => "derived_from",
        }
    }
}

/// Contents of `node.toml`. Edges are *ids only*: which versions the work was
/// built against is a fact about the work, recorded in the result's pins, so
/// updating a pin never counts as a definition change. There is deliberately
/// no extension map: an unrelated tool writing a key here would move the
/// definition version and invalidate every result and pin in the graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeMeta {
    pub schema: u32,
    pub author: Author,
    /// Who the work is for; `None` means anyone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Author>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<NodeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<NodeId>,
    /// The exact candidate this review node verifies. Present ⇒ review node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifies: Option<CandidateId>,
}

impl NodeMeta {
    pub fn is_verification(&self) -> bool {
        self.verifies.is_some()
    }
}

/// A definition's version: the git blob ids of `node.toml` and `description.md`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DefinitionVersion {
    pub metadata: String,
    pub description: String,
}

/// A result's version: the git blob ids of `result.toml` and `result.md`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResultVersion {
    pub metadata: String,
    pub notes: Option<String>,
}

/// A reference to content in an artifact system (for git, a commit).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub scheme: String,
    pub repository: String,
    pub id: String,
}

// --- results -------------------------------------------------------------------

/// The five recordable outcomes, in two families. Which family a node accepts
/// is fixed by its definition, so a mismatch is a corrupt record rather than a
/// state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    Accepted,
    Rejected,
    Abandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeFamily {
    Work,
    Verification,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn family(self) -> OutcomeFamily {
        match self {
            Self::Done | Self::Failed => OutcomeFamily::Work,
            Self::Accepted | Self::Rejected | Self::Abandoned => OutcomeFamily::Verification,
        }
    }

    /// Whether this outcome is the kind a node that does (or does not) verify
    /// a candidate may record.
    pub fn suits(self, verification_node: bool) -> bool {
        (self.family() == OutcomeFamily::Verification) == verification_node
    }

    /// Whether a `depends_on` edge onto a node with this outcome is satisfied.
    pub fn satisfies_dependency(self) -> bool {
        matches!(self, Self::Done | Self::Accepted)
    }

    /// Whether a result with this outcome must carry a full pin for every
    /// declared edge: it asserts a conclusion built on those inputs.
    pub fn requires_full_pins(self) -> bool {
        !matches!(self, Self::Failed)
    }
}

/// A dependency or lineage node pinned at submission time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumedNode {
    pub id: NodeId,
    pub definition: DefinitionVersion,
    pub result: Option<ResultVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    pub output: Option<ArtifactRef>,
}

/// A consumed file that is no node's output, pinned by content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPin {
    pub path: ProjectPath,
    pub identity: String,
    /// Whether the pin was discovered after execution rather than declared.
    pub observed: bool,
}

/// Contents of `observed-context.toml`: context discovered *after* execution,
/// pinned at the result's frozen project revision. It is a separate file
/// because amending the result would move the result version and invalidate
/// every candidate and pin referencing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedContext {
    pub schema: u32,
    pub result: ResultVersion,
    #[serde(default)]
    pub pins: Vec<ContextPin>,
}

/// Namespaced application data Linka stores verbatim and never interprets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Namespaced {
    pub namespace: Namespace,
    pub data: toml::Value,
}

/// Contents of `result.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultMeta {
    pub schema: u32,
    /// Unix milliseconds when the result was recorded.
    pub at: i64,
    pub author: Author,
    /// The definition version this result covered.
    pub definition: DefinitionVersion,
    pub outcome: Outcome,
    pub project: ProjectSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed: Vec<ConsumedNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<Namespaced>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub scheme: String,
    pub repository: String,
    pub revision: String,
    pub tree: String,
}

// --- attachments ----------------------------------------------------------------

/// Metadata for opaque, immutable bytes associated with a node. Attachments are
/// deliberately outside the definition and result: Linka stores and versions
/// their bytes but never admits them to graph state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub schema: u32,
    pub namespace: Namespace,
    pub key: AttachmentKey,
    /// Unix milliseconds when the attachment was first recorded.
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Git blob identity of the payload bytes.
    pub content: String,
    pub size: u64,
}

/// Caller-supplied data for one immutable attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAttachment {
    pub namespace: Namespace,
    pub key: AttachmentKey,
    pub media_type: Option<String>,
    pub data: Vec<u8>,
}

// --- candidates ------------------------------------------------------------------

/// A producer-owned idempotency key. Linka never interprets the namespace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIdentity {
    pub namespace: Namespace,
    pub id: String,
}

/// A proposed project output attached to an exact node result and an immutable
/// artifact commit.
///
/// The record is immutable in full and carries *no* decision state: whether it
/// was accepted or rejected is derived from the results of the verification
/// nodes that name it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub schema: u32,
    pub id: CandidateId,
    pub node: NodeId,
    /// The exact result this candidate proposes output for.
    pub result: ResultVersion,
    pub artifact: ArtifactRef,
    /// Display branch, informational: it may be moved or deleted freely.
    pub branch: String,
    /// Intended target branch, without `refs/heads/`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalIdentity>,
}

impl Candidate {
    /// The candidate's target as a full ref name.
    pub fn target_ref(&self) -> String {
        format!("refs/heads/{}", self.target)
    }
}

pub struct NewCandidate {
    pub node: NodeId,
    pub branch: String,
    pub target: String,
    pub external: Option<ExternalIdentity>,
}

/// What the current verifications concluded about a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateDecision {
    /// No current verification has decided it yet.
    Pending,
    Accepted,
    Rejected,
}

// --- the snapshot/submission protocol -----------------------------------------------

/// The frozen input of one unit of work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkSnapshot {
    pub schema: u32,
    pub node: NodeId,
    pub definition: DefinitionVersion,
    pub dependencies: Vec<ConsumedNode>,
    pub lineage: Vec<ConsumedNode>,
    pub context: Vec<ContextPin>,
    pub project: ProjectSnapshot,
    pub previous_result: Option<ResultVersion>,
}

/// What a submission concludes. Answering a question is `Done { output: None }`;
/// concluding a review is one of the three verification variants.
#[derive(Clone, Debug, PartialEq)]
pub enum Conclusion {
    Done { output: Option<ArtifactRef> },
    Failed,
    Accepted,
    Rejected,
    Abandoned,
}

impl Conclusion {
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Done { .. } => Outcome::Done,
            Self::Failed => Outcome::Failed,
            Self::Accepted => Outcome::Accepted,
            Self::Rejected => Outcome::Rejected,
            Self::Abandoned => Outcome::Abandoned,
        }
    }

    pub fn output(&self) -> Option<&ArtifactRef> {
        match self {
            Self::Done { output } => output.as_ref(),
            _ => None,
        }
    }

    /// Whether the conclusion asserts that the work succeeded — the one case
    /// where readiness is enforced.
    pub fn asserts_success(&self) -> bool {
        self.outcome().satisfies_dependency()
    }
}

/// Everything one call to `submit` records.
pub struct Submission {
    pub snapshot: WorkSnapshot,
    pub conclusion: Conclusion,
    pub notes: String,
    pub author: Author,
    pub producer: Option<Namespaced>,
    /// Recorded in the same commit; validated before anything is written.
    pub attachments: Vec<NewAttachment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmissionConflict {
    DefinitionChanged,
    DependenciesChanged,
    LineageChanged,
    ContextChanged { path: ProjectPath },
    ProjectChanged,
    ReadinessChanged,
    PreviousResultChanged,
}

// --- derived state --------------------------------------------------------------

/// The result evidence currently recorded for a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedOutcome {
    Open,
    Succeeded,
    Failed,
    Accepted,
    Rejected,
    Abandoned,
}

impl RecordedOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
        }
    }
}

impl From<Outcome> for RecordedOutcome {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Done => Self::Succeeded,
            Outcome::Failed => Self::Failed,
            Outcome::Accepted => Self::Accepted,
            Outcome::Rejected => Self::Rejected,
            Outcome::Abandoned => Self::Abandoned,
        }
    }
}

/// Whether recorded evidence still covers the current graph and project facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Current,
    Stale,
}

/// Whether the current successful result must be, or has been, integrated into
/// its candidate's target branch. Direct results need no integration step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    NotRequired,
    Pending,
    Accepted,
    Published,
    Rejected,
}

impl IntegrationStatus {
    /// Whether integration has reached a terminal answer, so the node is not
    /// waiting on anyone.
    pub fn is_done(self) -> bool {
        matches!(self, Self::NotRequired | Self::Published | Self::Rejected)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not-required",
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Published => "published",
            Self::Rejected => "rejected",
        }
    }
}

/// A machine-readable reason that recorded evidence is stale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StalenessReason {
    DefinitionChanged { metadata: bool, description: bool },
    ConsumedDefinitionChanged { id: NodeId },
    ConsumedNodeMissing { id: NodeId },
    ConsumedResultChanged { id: NodeId },
    ConsumedOutputChanged { id: NodeId },
    ContextChanged { path: ProjectPath },
    ContextMissing { path: ProjectPath },
    OutputDrifted { artifact: String, detail: String },
}

/// Why one `depends_on` target does not satisfy a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerReason {
    Missing,
    Open,
    Failed,
    Rejected,
    Abandoned,
    Stale,
    AwaitingIntegration,
    Error,
}

impl BlockerReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Open => "open",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
            Self::Stale => "stale",
            Self::AwaitingIntegration => "awaiting integration",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub id: NodeId,
    pub reason: BlockerReason,
}

/// The complete derived state of one node.
///
/// A node whose own records cannot be read, parsed, or reconciled is `Error`:
/// never ready, never complete, and blocking its dependents — but it does not
/// stop the rest of the graph from being queried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NodeState {
    Error {
        message: String,
    },
    Known {
        outcome: RecordedOutcome,
        currency: Currency,
        integration: IntegrationStatus,
        staleness: Vec<StalenessReason>,
        blockers: Vec<Blocker>,
    },
}

/// The single answer callers act on: what, if anything, to do with this node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Workability {
    Complete,
    Ready,
    AwaitingIntegration,
    Blocked,
    Error,
}

impl Workability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Ready => "ready",
            Self::AwaitingIntegration => "awaiting-integration",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }
}

impl NodeState {
    pub fn workability(&self) -> Workability {
        let Self::Known {
            outcome,
            currency,
            integration,
            blockers,
            ..
        } = self
        else {
            return Workability::Error;
        };
        let current = *currency == Currency::Current;
        let complete = current
            && match outcome {
                RecordedOutcome::Succeeded => {
                    matches!(
                        integration,
                        IntegrationStatus::NotRequired | IntegrationStatus::Published
                    )
                }
                RecordedOutcome::Accepted
                | RecordedOutcome::Rejected
                | RecordedOutcome::Abandoned => true,
                RecordedOutcome::Open | RecordedOutcome::Failed => false,
            };
        if complete {
            Workability::Complete
        } else if current && *outcome == RecordedOutcome::Succeeded && !integration.is_done() {
            // Waiting on a decision or a publication, not on more work.
            Workability::AwaitingIntegration
        } else if blockers.is_empty() {
            Workability::Ready
        } else {
            Workability::Blocked
        }
    }

    pub fn is_complete(&self) -> bool {
        self.workability() == Workability::Complete
    }

    pub fn is_ready(&self) -> bool {
        self.workability() == Workability::Ready
    }

    pub fn is_awaiting_integration(&self) -> bool {
        self.workability() == Workability::AwaitingIntegration
    }

    pub fn is_blocked(&self) -> bool {
        self.workability() == Workability::Blocked
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// The outcome recorded for the node; `None` when its records are bad.
    pub fn outcome(&self) -> Option<RecordedOutcome> {
        match self {
            Self::Error { .. } => None,
            Self::Known { outcome, .. } => Some(*outcome),
        }
    }

    pub fn currency(&self) -> Option<Currency> {
        match self {
            Self::Error { .. } => None,
            Self::Known { currency, .. } => Some(*currency),
        }
    }

    pub fn integration(&self) -> Option<IntegrationStatus> {
        match self {
            Self::Error { .. } => None,
            Self::Known { integration, .. } => Some(*integration),
        }
    }

    pub fn staleness(&self) -> &[StalenessReason] {
        match self {
            Self::Error { .. } => &[],
            Self::Known { staleness, .. } => staleness,
        }
    }

    pub fn blockers(&self) -> &[Blocker] {
        match self {
            Self::Error { .. } => &[],
            Self::Known { blockers, .. } => blockers,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Error { message } => Some(message),
            Self::Known { .. } => None,
        }
    }
}

/// A node's display title: the first non-empty line of its description. There
/// is no stored title — the description is the definition, and its opening line
/// names the node wherever a one-liner is needed.
pub fn title_of(description: &str) -> &str {
    description
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("(no description)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_the_first_non_empty_line_of_the_description() {
        assert_eq!(title_of("Parse config\n\nDetails follow."), "Parse config");
        assert_eq!(title_of("\n  \n  Leading blanks\nrest"), "Leading blanks");
        assert_eq!(title_of("one-liner"), "one-liner");
        assert_eq!(title_of(""), "(no description)");
        assert_eq!(title_of("  \n\t\n"), "(no description)");
    }

    #[test]
    fn one_component_rule_covers_every_stored_name() {
        for invalid in [
            "",
            ".",
            "..",
            ".git",
            ".GIT",
            "a/b",
            r"a\b",
            "C:node",
            "bad\nname",
            "CON",
            "nul.txt",
            "COM1",
            "LPT9",
            "trailing.",
            "trailing ",
        ] {
            assert!(component(invalid).is_err(), "accepted `{invalid}`");
        }
        for valid in ["node-1", "orka", "a report", "com0", "com10", "file.txt"] {
            assert!(component(valid).is_ok(), "rejected `{valid}`");
        }
    }

    #[test]
    fn multi_byte_names_are_judged_rather_than_panicked_on() {
        // Validation runs on names read off disk, so every one of these is a
        // question to answer, not a slice to take: "𝄞" is four *bytes* and one
        // character, which used to be sliced apart inside the device-name check.
        for name in ["𝄞", "𝄞.md", "ré1", "çom1", "…", "COM𝄞"] {
            assert!(component(name).is_ok(), "rejected `{name}`");
        }
        assert!(component("com1.𝄞").is_err(), "accepted a device name");
        assert!("𝄞".parse::<NodeId>().is_ok());
        assert!("src/𝄞.rs".parse::<ProjectPath>().is_ok());
    }

    #[test]
    fn identifiers_and_project_paths_are_validated_and_normalized() {
        assert!("node-good".parse::<NodeId>().is_ok());
        assert!("../secret".parse::<NodeId>().is_err());
        assert!("candidate-1".parse::<CandidateId>().is_ok());
        assert!("cand-1".parse::<CandidateId>().is_err());
        assert!(CandidateId::new().as_str().starts_with("candidate-"));

        for invalid in [
            "",
            "..",
            "../secret",
            "/absolute",
            r"..\secret",
            ".git/config",
            "src/.git/config",
            "C:/windows",
            "bad\npath",
            "src//file.rs",
        ] {
            assert!(
                invalid.parse::<ProjectPath>().is_err(),
                "accepted project path {invalid:?}"
            );
        }
        assert_eq!(
            r"src\nested\file.rs".parse::<ProjectPath>().unwrap(),
            "src/nested/file.rs"
        );
    }

    #[test]
    fn outcome_families_decide_which_nodes_may_record_what() {
        assert!(Outcome::Done.suits(false));
        assert!(!Outcome::Done.suits(true));
        assert!(Outcome::Abandoned.suits(true));
        assert!(!Outcome::Rejected.suits(false));
        assert!(Outcome::Accepted.satisfies_dependency());
        assert!(!Outcome::Rejected.satisfies_dependency());
        assert!(!Outcome::Failed.requires_full_pins());
    }

    #[test]
    fn workability_follows_the_truth_table() {
        let known = |outcome, currency, integration, blockers: Vec<Blocker>| NodeState::Known {
            outcome,
            currency,
            integration,
            staleness: Vec::new(),
            blockers,
        };
        use IntegrationStatus as I;
        use RecordedOutcome as O;
        let cases = [
            (
                O::Open,
                Currency::Current,
                I::NotRequired,
                Workability::Ready,
            ),
            (
                O::Failed,
                Currency::Current,
                I::NotRequired,
                Workability::Ready,
            ),
            (
                O::Failed,
                Currency::Stale,
                I::NotRequired,
                Workability::Ready,
            ),
            (
                O::Succeeded,
                Currency::Current,
                I::NotRequired,
                Workability::Complete,
            ),
            (
                O::Succeeded,
                Currency::Current,
                I::Published,
                Workability::Complete,
            ),
            (
                O::Succeeded,
                Currency::Current,
                I::Pending,
                Workability::AwaitingIntegration,
            ),
            (
                O::Succeeded,
                Currency::Current,
                I::Accepted,
                Workability::AwaitingIntegration,
            ),
            (
                O::Succeeded,
                Currency::Current,
                I::Rejected,
                Workability::Ready,
            ),
            (
                O::Succeeded,
                Currency::Stale,
                I::Pending,
                Workability::Ready,
            ),
            (
                O::Accepted,
                Currency::Current,
                I::NotRequired,
                Workability::Complete,
            ),
            (
                O::Rejected,
                Currency::Stale,
                I::NotRequired,
                Workability::Ready,
            ),
        ];
        for (outcome, currency, integration, expected) in cases {
            let state = known(outcome, currency, integration, Vec::new());
            assert_eq!(
                state.workability(),
                expected,
                "{outcome:?}/{currency:?}/{integration:?}"
            );
            // A blocker only changes work that would otherwise be ready:
            // completion and integration are facts about this node alone.
            let blocked = known(
                outcome,
                currency,
                integration,
                vec![Blocker {
                    id: "node-x".parse().unwrap(),
                    reason: BlockerReason::Open,
                }],
            );
            assert_eq!(
                blocked.workability(),
                if expected == Workability::Ready {
                    Workability::Blocked
                } else {
                    expected
                }
            );
        }
        assert_eq!(
            NodeState::Error {
                message: "bad".into()
            }
            .workability(),
            Workability::Error
        );
    }
}
