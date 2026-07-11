//! On-disk data types.
//!
//! A node separates structured data from prose: `node.toml` and
//! `description.md` form its definition, while `result.toml` and the optional
//! `result.md` form its completion record.
//!
//! Status is never stored. It is derived from whether `result.toml` exists,
//! what its `outcome` says, and whether its definition version still matches.

use serde::{Deserialize, Serialize};

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

/// Who authored a definition or did the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Human,
    Machine,
}

impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Author::Human => "human",
            Author::Machine => "machine",
        }
    }
}

/// A node's derived lifecycle status. Never stored anywhere: computed from the
/// presence and content of `result.toml` against the current definition files.
///
/// There is deliberately no `in_progress` (a node is worked once, and nothing
/// records "being worked") and no `blocked` (a fact about dependencies, derived
/// by the `blockers` query).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// No result yet — or the definition was edited after a `done`, which
    /// reopens the node (the completion no longer covers the current content).
    Open,
    Done,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }
}

/// The stored outcome of a node's unit of work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Failed => "failed",
        }
    }
}

/// Which dependency list of `node.toml` an id goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    DependsOn,
    DerivedFrom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Accepted,
    Rejected,
}

/// The exact implementation attempt a review node examines.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewTarget {
    pub implementation: String,
    pub attempt_id: String,
    pub candidate_branch: String,
    pub candidate_commit: String,
    pub reviewed_result: ResultVersion,
}

impl DepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DepKind::DependsOn => "depends_on",
            DepKind::DerivedFrom => "derived_from",
        }
    }
}

/// Contents of `node.toml`. Dependencies are *ids only*:
/// which versions the work was actually built against is a fact about the work,
/// recorded in [`ResultMeta::built_against`] at completion, so that updating a
/// pin never counts as a definition change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeMeta {
    /// On-disk schema version, for forward compatibility.
    pub schema: u32,
    pub author: Author,
    /// Who the work is *for*: a machine-authored question node is assigned to a
    /// human, whose answer (its result notes) unblocks the asker. Absent means
    /// anyone may work it. Distinct from `author` — who wrote the definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Author>,
    /// Ids of nodes this node needs finished before it can be worked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Ids of nodes this node was derived from (lineage, not a work blocker).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// Present only on automatically-created review nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewTarget>,
}

/// Git blob ids of the two files that constitute a node definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionVersion {
    pub metadata: String,
    pub description: String,
}

/// Git blob ids of the files that constitute a result. `notes` is absent when
/// there is no `result.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultVersion {
    pub metadata: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// The exact version of a related node the work was built against, captured at
/// completion time. Definition and result versions are recorded separately,
/// along with the target's output commit. Any consumed component moving later
/// makes this node stale.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuiltAgainst {
    pub id: String,
    pub definition: DefinitionVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// A file that was consumed during the work but is not any node's output —
/// pinned by its git blob id so a later change to it flags this node.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextPin {
    /// Path relative to the project root.
    pub path: String,
    /// The file's blob id when it was pinned.
    pub blob: String,
    /// How the pin got here: `false` for a pin the worker declared at
    /// completion, `true` for one derived afterwards from the recorded
    /// session (the worker was observed reading the file but did not
    /// declare it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub observed: bool,
}

/// The engine that produced a result: which backend ran the work, and which
/// model it was given. Stamped onto `result.toml` by the work driver after the
/// session (the worker itself does not reliably know what it runs on); absent
/// on results recorded by hand.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkedBy {
    /// The backend that ran the session (e.g. `claude-code`).
    pub backend: String,
    /// The model the backend was asked for. Absent means the backend's own
    /// default — whatever that was at the time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Contents of `result.toml` — the record of the node's one unit of work.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultMeta {
    /// Unix milliseconds when the result was recorded.
    pub at: i64,
    pub author: Author,
    /// Version of the definition this work fulfilled. A `done` certifies only
    /// these exact metadata and description blobs.
    pub definition: DefinitionVersion,
    pub outcome: Outcome,
    /// Machine completion awaiting automatic verification/publication.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub publication_pending: bool,
    /// Exact project commit and tree the work started from. Optional for
    /// results written by older versions and for hand-recorded answers/failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tree: Option<String>,
    /// Execution attempt and permanent project branch that produced this
    /// candidate. Optional for older and hand-recorded results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_branch: Option<String>,
    /// Present only on completed review nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<ReviewDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion_commit: Option<String>,
    /// The single git commit encompassing all files this node produced.
    /// Absent when the work produced no files (graph-only work) or failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_commit: Option<String>,
    /// Final verified commit published to the configured target branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_previous: Option<String>,
    /// The backend/model that did the work, stamped by the driver afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worked_by: Option<WorkedBy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built_against: Vec<BuiltAgainst>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextPin>,
}

#[cfg(test)]
mod tests {
    use super::title_of;

    #[test]
    fn title_is_the_first_non_empty_line_of_the_description() {
        assert_eq!(title_of("Parse config\n\nDetails follow."), "Parse config");
        assert_eq!(title_of("\n  \n  Leading blanks\nrest"), "Leading blanks");
        assert_eq!(title_of("one-liner"), "one-liner");
        assert_eq!(title_of(""), "(no description)");
        assert_eq!(title_of("  \n\t\n"), "(no description)");
    }
}
