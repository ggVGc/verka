//! Work snapshots and the checked submissions recorded against them.

use super::{
    ArtifactRef, Author, ConsumedNode, ContextPin, DefinitionVersion, NodeId, Outcome,
    ProducerEvidence, ProjectPath, ProjectSnapshot, ResultVersion, VerificationOutcome,
};
use serde::{Deserialize, Serialize};

pub const SNAPSHOT_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// Shared, producer-neutral fields of every checked submission. The payload
/// remains separate so verification submissions cannot carry work outputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmissionEnvelope {
    pub snapshot: WorkSnapshot,
    pub notes: String,
    pub author: Author,
    pub producer: Option<ProducerEvidence>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerificationSubmission {
    pub snapshot: WorkSnapshot,
    pub outcome: VerificationOutcome,
    pub notes: String,
    pub author: Author,
    pub producer: Option<ProducerEvidence>,
}

impl ResultSubmission {
    pub fn envelope(&self) -> SubmissionEnvelope {
        SubmissionEnvelope {
            snapshot: self.snapshot.clone(),
            notes: self.notes.clone(),
            author: self.author,
            producer: self.producer.clone(),
        }
    }
}

impl VerificationSubmission {
    pub fn envelope(&self) -> SubmissionEnvelope {
        SubmissionEnvelope {
            snapshot: self.snapshot.clone(),
            notes: self.notes.clone(),
            author: self.author,
            producer: self.producer.clone(),
        }
    }
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
