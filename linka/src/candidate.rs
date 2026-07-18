//! First-class candidate outputs and target-branch publication.
//!
//! Candidates are attached to an exact node result and immutable artifact.
//! Producer metadata is opaque, keeping execution drivers outside Linka's domain.

use crate::model::{ArtifactRef, Author, CandidateId, IntegrationStatus, NodeId, ResultVersion};
use crate::{Store, Vcs};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

mod operations;
mod storage;
#[cfg(test)]
mod tests;

pub const CANDIDATE_SCHEMA: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIdentity {
    pub namespace: String,
    pub id: String,
}

/// One proposed project output and its review state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub schema: u32,
    pub id: CandidateId,
    pub node: NodeId,
    pub result: ResultVersion,
    pub artifact: ArtifactRef,
    /// Candidate branch name, without `refs/heads/`.
    pub branch: String,
    /// Intended target branch name, without `refs/heads/`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalIdentity>,
    pub state: CandidateState,
}

pub struct NewCandidate {
    pub node: NodeId,
    pub branch: String,
    pub target: String,
    pub external: Option<ExternalIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CandidateState {
    Pending,
    Accepted {
        decided_at_ms: i64,
        author: Author,
        notes: String,
        target_previous: String,
    },
    Rejected {
        decided_at_ms: i64,
        author: Author,
        notes: String,
    },
}

impl CandidateRecord {
    /// Derive publication from the accepted intent and the target's Git history.
    pub fn integration(&self, vcs: &dyn Vcs) -> Result<IntegrationStatus> {
        let CandidateState::Accepted {
            target_previous, ..
        } = &self.state
        else {
            return Ok(match self.state {
                CandidateState::Pending => IntegrationStatus::Pending,
                CandidateState::Rejected { .. } => IntegrationStatus::Rejected,
                CandidateState::Accepted { .. } => unreachable!(),
            });
        };
        let target_ref = branch_ref(&self.target);
        let target = vcs
            .ref_commit(&target_ref)?
            .with_context(|| format!("accepted candidate target `{target_ref}` is missing"))?;
        if vcs.is_ancestor(&self.artifact.id, &target)? {
            Ok(IntegrationStatus::Published)
        } else if target == *target_previous {
            Ok(IntegrationStatus::Accepted)
        } else {
            bail!(
                "candidate `{}` target moved from {} to {} without containing {}",
                self.id,
                target_previous,
                target,
                self.artifact.id
            )
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
