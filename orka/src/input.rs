//! The durable input to one attempt.
//!
//! An attempt is built against Linka's [`linka::WorkSnapshot`] — the
//! authoritative, version-checked freeze of the node's definition, dependency
//! and lineage pins, explicit context, project revision, and previous result.
//! Orka persists that snapshot verbatim and submits against it unchanged.
//!
//! Alongside it, Orka keeps the prose it hands the agent: the node description
//! and the completed related work presented as context. This prose is frozen
//! audit material — what the agent was actually told — and is deliberately
//! distinct from the snapshot, which alone is authoritative for submission.

use linka::{NodeId, WorkSnapshot};
use serde::{Deserialize, Serialize};

/// Everything one attempt is built against: Linka's authoritative snapshot plus
/// the prompt prose Orka owns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttemptInput {
    /// Linka's frozen, version-checked work input. Authoritative for submission.
    pub snapshot: WorkSnapshot,
    /// Named project branch this attempt proposes to integrate into.
    pub target_branch: String,
    /// The node's definition prose, as shown to the agent.
    pub description: String,
    /// Completed dependencies (`depends_on`), presented as prompt context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_context: Vec<DependencyContext>,
    /// Lineage the work derives from (`derived_from`), presented separately so
    /// the agent can tell required inputs from prior related work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lineage_context: Vec<DependencyContext>,
}

impl AttemptInput {
    /// The project commit the work starts from — the snapshot's frozen revision.
    pub fn input_commit(&self) -> &str {
        &self.snapshot.project.revision
    }

    /// The node this attempt works.
    pub fn node(&self) -> &NodeId {
        &self.snapshot.node
    }
}

/// One completed related node, as prose context for the agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DependencyContext {
    pub node: NodeId,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_notes: String,
}

/// A minimal, self-consistent attempt input for unit tests that do not need a
/// real Linka store.
#[cfg(test)]
pub(crate) fn sample_input(node: &str) -> AttemptInput {
    use linka::{DefinitionVersion, ProjectSnapshot};
    AttemptInput {
        snapshot: WorkSnapshot {
            schema: linka::SNAPSHOT_SCHEMA,
            node: node.parse().unwrap(),
            definition: DefinitionVersion {
                metadata: "m".into(),
                description: "d".into(),
            },
            dependencies: vec![],
            lineage: vec![],
            context: vec![],
            project: ProjectSnapshot {
                scheme: "git".into(),
                repository: "r".into(),
                revision: "c0ffee".into(),
                tree: "t".into(),
            },
            previous_result: None,
        },
        target_branch: "main".into(),
        description: "Do the thing".into(),
        dependency_context: vec![],
        lineage_context: vec![],
    }
}

#[cfg(test)]
mod tests;
