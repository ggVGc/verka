//! On-disk data types.
//!
//! A node is a directory `nodes/<id>/` holding at most two files:
//!
//!   * `node.md` — the definition: [`NodeMeta`] frontmatter plus a prose body.
//!     Its git blob id is the node's *version*; editing the file changes it.
//!   * `result.md` — the completion record: [`ResultMeta`] frontmatter plus a
//!     free-form narrative of what happened during the work. Written once when
//!     the node's single unit of work finishes; overwritten on rework (git
//!     history keeps every earlier attempt).
//!
//! Status is never stored. It is derived from whether `result.md` exists, what
//! its `outcome` says, and whether its `node_version` still matches `node.md`.

use serde::{Deserialize, Serialize};

/// What kind of node this is. Mirrors the llaundry node taxonomy
/// (task -> implementation -> build/verification).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Task,
    Implementation,
    Build,
    Verification,
    Info,
}

impl NodeType {
    /// Short, human-scannable prefix used in node ids, e.g. `task-01J8...`.
    pub fn prefix(self) -> &'static str {
        match self {
            NodeType::Task => "task",
            NodeType::Implementation => "impl",
            NodeType::Build => "build",
            NodeType::Verification => "verify",
            NodeType::Info => "info",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            NodeType::Task => "task",
            NodeType::Implementation => "implementation",
            NodeType::Build => "build",
            NodeType::Verification => "verification",
            NodeType::Info => "info",
        }
    }
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
/// presence and content of `result.md` against the current `node.md`.
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

/// Which dependency list of `node.md` an id goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    DependsOn,
    DerivedFrom,
}

impl DepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DepKind::DependsOn => "depends_on",
            DepKind::DerivedFrom => "derived_from",
        }
    }
}

/// Frontmatter of `node.md` — the definition. Dependencies are *ids only*:
/// which versions the work was actually built against is a fact about the work,
/// recorded in [`ResultMeta::built_against`] at completion, so that updating a
/// pin never counts as a definition change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeMeta {
    /// On-disk schema version, for forward compatibility.
    pub schema: u32,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub title: String,
    pub author: Author,
    /// Ids of nodes this node needs finished before it can be worked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Ids of nodes this node was derived from (lineage, not a work blocker).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
}

/// The exact version of a related node the work was built against, captured at
/// completion time. `pin` is the blob id of the target's `node.md`; `output` is
/// the target's output commit at that moment, if it had one. Either moving on
/// later makes this node stale.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuiltAgainst {
    pub id: String,
    pub pin: String,
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
}

/// Frontmatter of `result.md` — the record of the node's one unit of work.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultMeta {
    /// Unix milliseconds when the result was recorded.
    pub at: i64,
    pub author: Author,
    /// Blob id of the `node.md` this work fulfilled. A `done` certifies only
    /// that version: editing the definition afterwards reopens the node.
    pub node_version: String,
    pub outcome: Outcome,
    /// The single git commit encompassing all files this node produced.
    /// Absent when the work produced no files (graph-only work) or failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built_against: Vec<BuiltAgainst>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextPin>,
}
