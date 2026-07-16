//! First-class candidate outputs and recoverable target-branch publication.
//!
//! Candidates are attached to an exact node result and immutable artifact.
//! Producer metadata is opaque, keeping execution drivers outside Linka's domain.

use crate::model::{
    ArtifactRef, Author, IntegrationStatus, NodeId, ProducerEvidence, ResultVersion,
};
use crate::{Store, Vcs};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

mod operations;
mod publication;
mod storage;
#[cfg(test)]
mod tests;

pub const CANDIDATE_SCHEMA: u32 = 1;
pub const DECISION_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CandidateId(pub String);

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

impl std::fmt::Display for CandidateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIdentity {
    pub namespace: String,
    pub id: String,
}

/// Immutable identity of one proposed project output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub schema: u32,
    pub id: CandidateId,
    pub created_at_ms: i64,
    pub node: NodeId,
    pub result: ResultVersion,
    pub artifact: ArtifactRef,
    /// Candidate branch name, without `refs/heads/`.
    pub branch: String,
    pub input_commit: String,
    /// Intended target branch name, without `refs/heads/`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerEvidence>,
}

pub struct NewCandidate {
    pub node: NodeId,
    pub branch: String,
    pub input_commit: String,
    pub target: String,
    pub external: Option<ExternalIdentity>,
    pub producer: Option<ProducerEvidence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDecision {
    pub schema: u32,
    pub decided_at_ms: i64,
    pub kind: DecisionKind,
    pub author: Author,
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_previous: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CandidateView {
    pub candidate: CandidateRecord,
    pub decision: Option<CandidateDecision>,
}

impl CandidateView {
    /// Derive publication from the accepted intent and the target's Git history.
    pub fn integration(&self, vcs: &dyn Vcs) -> Result<IntegrationStatus> {
        match self.decision.as_ref().map(|decision| decision.kind) {
            None => Ok(IntegrationStatus::Pending),
            Some(DecisionKind::Rejected) => Ok(IntegrationStatus::Rejected),
            Some(DecisionKind::Accepted) => {
                let decision = self.decision.as_ref().expect("accepted decision");
                let target_ref = decision
                    .target_ref
                    .as_deref()
                    .context("accepted candidate has no target")?;
                let previous = decision
                    .target_previous
                    .as_deref()
                    .context("accepted candidate has no target baseline")?;
                let target = vcs.ref_commit(target_ref)?.with_context(|| {
                    format!("accepted candidate target `{target_ref}` is missing")
                })?;
                if vcs.is_ancestor(&self.candidate.artifact.id, &target)? {
                    Ok(IntegrationStatus::Published)
                } else if target == previous {
                    Ok(IntegrationStatus::Accepted)
                } else {
                    bail!(
                        "candidate `{}` target moved from {} to {} without containing {}",
                        self.candidate.id,
                        previous,
                        target,
                        self.candidate.artifact.id
                    )
                }
            }
        }
    }
}

pub struct CandidateStore<'a> {
    store: &'a Store,
}

impl<'a> CandidateStore<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
}

fn branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
