//! On-disk data types.
//!
//! A node separates structured data from prose: `node.toml` and
//! `description.md` form its definition, while `result.toml` and the optional
//! `result.md` form its completion record.
//!
//! Status is never stored. It is derived from whether `result.toml` exists,
//! what its `outcome` says, and whether its definition version still matches.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NodeId(String);

impl NodeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl AsRef<str> for NodeId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl std::ops::Deref for NodeId {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl From<NodeId> for String {
    fn from(value: NodeId) -> Self {
        value.0
    }
}
impl TryFrom<String> for NodeId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}
impl FromStr for NodeId {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value == "." || value == ".." {
            return Err("node id must be a non-empty name".into());
        }
        if value.contains(['/', '\\']) || value.chars().any(char::is_control) {
            return Err("node id must not contain separators or control characters".into());
        }
        if value.eq_ignore_ascii_case(".git") || (value.len() >= 2 && value.as_bytes()[1] == b':') {
            return Err("node id uses a forbidden platform name or prefix".into());
        }
        Ok(Self(value.into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectPath(String);

impl ProjectPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for ProjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl AsRef<str> for ProjectPath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
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
impl From<ProjectPath> for String {
    fn from(value: ProjectPath) -> Self {
        value.0
    }
}
impl TryFrom<String> for ProjectPath {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}
impl FromStr for ProjectPath {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.replace('\\', "/");
        if normalized.is_empty()
            || normalized.starts_with('/')
            || (normalized.len() >= 2 && normalized.as_bytes()[1] == b':')
            || normalized.chars().any(char::is_control)
        {
            return Err("project path must be a non-empty relative path".into());
        }
        for component in normalized.split('/') {
            if component.is_empty()
                || component == "."
                || component == ".."
                || component.eq_ignore_ascii_case(".git")
            {
                return Err("project path contains a forbidden component".into());
            }
        }
        Ok(Self(normalized))
    }
}

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

/// Contents of `node.toml`. Dependencies are *ids only*: which versions the
/// work was actually built against is a fact about the work, recorded in the
/// result's consumed pins at completion, so that updating a pin never counts
/// as a definition change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeMeta {
    pub schema: u32,
    pub author: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Author>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<NodeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<NodeId>,
    /// Namespaced application metadata (e.g. from an execution harness) is
    /// preserved but never interpreted here.
    #[serde(default, flatten)]
    pub extensions: std::collections::BTreeMap<String, toml::Value>,
}

/// A definition's version: the Git blob ids of `node.toml` and `description.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionVersion {
    pub metadata: String,
    pub description: String,
}

/// A result's version: the Git blob ids of `result.toml` and `result.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultVersion {
    pub metadata: String,
    pub notes: Option<String>,
}

/// A reference to content in an artifact system (for git: a commit).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub scheme: String,
    pub repository: String,
    pub id: String,
}

/// A dependency pinned at completion time: which definition and result of it
/// the work was built against.
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
    #[serde(default)]
    pub observed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextObservation {
    pub schema: u32,
    pub result: ResultVersion,
    pub context: Vec<ContextPin>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
}
impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Namespaced evidence about what produced a result. Written by external
/// harnesses (e.g. an execution driver); preserved but never interpreted here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProducerEvidence {
    pub namespace: String,
    pub data: serde_json::Value,
}

/// Contents of `result.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultMeta {
    pub schema: u32,
    /// Unix milliseconds when the result was recorded.
    pub at: i64,
    /// Who recorded the result.
    pub author: Author,
    pub definition: DefinitionVersion,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed: Vec<ConsumedNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerEvidence>,
}

/// A node's derived status — never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    Done,
    Failed,
}

/// The result evidence currently recorded for a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordedOutcome {
    Open,
    Succeeded,
    Failed,
}

/// Whether recorded evidence still covers the current graph and project facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Currency {
    Current,
    Stale,
}

/// A machine-readable reason that recorded evidence is stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StalenessReason {
    DefinitionChanged { metadata: bool, description: bool },
    ConsumedDefinitionChanged { id: String },
    ConsumedNodeMissing { id: String },
    ConsumedResultChanged { id: String },
    ConsumedOutputChanged { id: String },
    ContextChanged { path: String },
    ContextMissing { path: String },
    OutputDrifted { artifact: String, detail: String },
}

/// Why one required dependency does not satisfy a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockerReason {
    Missing,
    Open,
    Failed,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocker {
    pub id: String,
    pub reason: BlockerReason,
}

/// The complete derived state of one node at a point in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeState {
    pub outcome: RecordedOutcome,
    pub currency: Currency,
    pub staleness: Vec<StalenessReason>,
    pub blockers: Vec<Blocker>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub scheme: String,
    pub repository: String,
    pub revision: String,
    pub tree: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSnapshot {
    pub node: NodeId,
    pub definition: DefinitionVersion,
    pub dependencies: Vec<ConsumedNode>,
    pub lineage: Vec<ConsumedNode>,
    pub context: Vec<ContextPin>,
    pub project: ProjectSnapshot,
    pub previous_result: Option<ResultVersion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultSubmission {
    pub snapshot: WorkSnapshot,
    pub outcome: Outcome,
    pub output: Option<ArtifactRef>,
    pub notes: String,
    pub author: Author,
    pub producer: Option<ProducerEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmissionConflict {
    DefinitionChanged,
    DependenciesChanged,
    LineageChanged,
    ContextChanged { path: ProjectPath },
    ProjectChanged,
    ReadinessChanged,
    PreviousResultChanged,
}

impl NodeState {
    pub fn is_complete(&self) -> bool {
        self.outcome == RecordedOutcome::Succeeded && self.currency == Currency::Current
    }

    pub fn is_ready(&self) -> bool {
        !self.is_complete() && self.blockers.is_empty()
    }

    pub fn is_blocked(&self) -> bool {
        !self.is_complete() && !self.blockers.is_empty()
    }
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Derive a node's status from its current definition version and its result.
/// `done` holds only while the result's definition version still matches:
/// editing the definition after completion reopens the node.
pub fn status(current: &DefinitionVersion, result: Option<&ResultMeta>) -> Status {
    match result {
        None => Status::Open,
        Some(result) if result.outcome == Outcome::Failed => Status::Failed,
        Some(result) if &result.definition == current => Status::Done,
        Some(_) => Status::Open,
    }
}

/// A node's display title: the first non-empty line of its description. There
/// is no stored title — the description is the definition, and its opening
/// line names the node wherever a one-liner is needed.
pub fn title_of(description: &str) -> &str {
    description
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
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
    fn status_follows_outcome_and_definition_version() {
        let version = DefinitionVersion {
            metadata: "m".into(),
            description: "d".into(),
        };
        let result = ResultMeta {
            schema: 1,
            at: 0,
            author: Author::Human,
            definition: version.clone(),
            outcome: Outcome::Done,
            project: None,
            consumed: vec![],
            context: vec![],
            output: None,
            producer: None,
        };
        assert_eq!(status(&version, None), Status::Open);
        assert_eq!(status(&version, Some(&result)), Status::Done);
        let moved = DefinitionVersion {
            metadata: "m2".into(),
            description: "d".into(),
        };
        assert_eq!(status(&moved, Some(&result)), Status::Open);
        let failed = ResultMeta {
            outcome: Outcome::Failed,
            ..result
        };
        assert_eq!(status(&version, Some(&failed)), Status::Failed);
    }

    #[test]
    fn graph_identifiers_and_project_paths_are_validated_and_normalized() {
        for invalid in [
            "",
            ".",
            "..",
            "../secret",
            "/absolute",
            r"..\secret",
            ".git",
            "C:node",
            "bad\nnode",
        ] {
            assert!(
                invalid.parse::<NodeId>().is_err(),
                "accepted node id {invalid:?}"
            );
        }
        assert_eq!("node-good".parse::<NodeId>().unwrap().as_str(), "node-good");

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
        ] {
            assert!(
                invalid.parse::<ProjectPath>().is_err(),
                "accepted project path {invalid:?}"
            );
        }
        assert_eq!(
            r"src\nested\file.rs"
                .parse::<ProjectPath>()
                .unwrap()
                .as_str(),
            "src/nested/file.rs"
        );
    }
}
