//! On-disk data types.
//!
//! There are exactly three kinds of records in the store:
//!
//!   * [`Meta`]        — the immutable, content-addressed definition of a node version.
//!   * [`StatusEvent`] — an immutable entry in a node's append-only status log.
//!   * a *ref*         — a one-line text file mapping a logical id to its current
//!                       version hash. It has no struct: it is just a hash string.
//!
//! `Meta` is the only thing that gets hashed. Status and refs deliberately live
//! outside the hash so that a status change or a "what is current" pointer move
//! never alters a node's identity (and therefore never invalidates its dependents).

use serde::{Deserialize, Serialize};

/// What kind of node this is. Mirrors the llaundry node taxonomy
/// (description -> task -> implementation -> build/verification).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Description,
    Task,
    Implementation,
    Build,
    Verification,
    Info,
}

impl NodeType {
    /// Short, human-scannable prefix used in logical ids, e.g. `task-01J8...`.
    pub fn prefix(self) -> &'static str {
        match self {
            NodeType::Description => "desc",
            NodeType::Task => "task",
            NodeType::Implementation => "impl",
            NodeType::Build => "build",
            NodeType::Verification => "verify",
            NodeType::Info => "info",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            NodeType::Description => "description",
            NodeType::Task => "task",
            NodeType::Implementation => "implementation",
            NodeType::Build => "build",
            NodeType::Verification => "verification",
            NodeType::Info => "info",
        }
    }
}

/// Who authored a version or status event.
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

/// A typed, directed edge to another node.
///
/// `to` is the *logical id* of the target (a stable handle that survives edits),
/// while `pin` records the target's *version hash* at the moment the edge was
/// created. The split is what makes invalidation possible: if the target later
/// gets a new version, `pin` no longer equals the target's current ref, so this
/// node is "stale" — it was built against a definition that has since moved.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    /// Logical id of the target node.
    pub to: String,
    /// Relationship kind: `depends_on`, `derived_from`, `verifies`, `builds`, ...
    pub rel: String,
    /// Target's version hash at link time.
    pub pin: String,
}

/// The immutable definition of a single node version.
///
/// Everything in here is part of the content hash. Editing any field produces a
/// new version with a new hash; the old version is never mutated. `parent` chains
/// versions together into a history (the "prompt history is the story" property).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    /// On-disk schema version, for forward compatibility.
    pub schema: u32,
    /// Stable identity of the logical node across all its versions.
    pub logical_id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub title: String,
    pub author: Author,
    /// Hash of the previous version, or `None` for the first version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

/// One immutable entry in a node's status log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusEvent {
    /// Unix milliseconds when the event was recorded.
    pub at: i64,
    /// Free-form status: `open`, `in_progress`, `done`, `failed`, `blocked`, ...
    pub status: String,
    pub author: Author,
    /// The version hash this status was asserted against.
    pub version: String,
}

/// The full status history of a node: an append-only list of [`StatusEvent`]s.
/// The current status is simply the last entry.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct StatusLog {
    #[serde(default, rename = "event")]
    pub events: Vec<StatusEvent>,
}
