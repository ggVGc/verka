//! Review and publication policy over immutable artifacts.

use llaundry_core::{ArtifactRef, ResultVersion};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub subject: String,
    pub result: ResultVersion,
    pub artifact: ArtifactRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub candidate: String,
    pub kind: DecisionKind,
    pub notes: String,
    pub suggestion: Option<ArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationIntent {
    pub candidate: String,
    pub target: String,
    pub expected_previous: ArtifactRef,
    pub completed: bool,
}

/// Portable review application messages for JSON-over-stdio adapters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    AddCandidate { candidate: Candidate },
    Decide { decision: Decision },
    Show { id: String },
}

pub trait CandidateStore {
    type Error;
    fn candidate(&self, id: &str) -> Result<Candidate, Self::Error>;
    fn record_decision(&self, decision: &Decision) -> Result<(), Self::Error>;
}

/// File-backed review state. New decisions are kept outside core node results;
/// legacy review nodes remain a read-compatible candidate source.
pub struct FsCandidateStore {
    root: PathBuf,
}

impl FsCandidateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn dir(&self, id: &str) -> PathBuf {
        self.root.join("reviews").join(id)
    }

    fn read_current(&self, id: &str) -> anyhow::Result<Candidate> {
        Ok(toml::from_str(&std::fs::read_to_string(
            self.dir(id).join("candidate.toml"),
        )?)?)
    }

    fn read_legacy(&self, id: &str) -> anyhow::Result<Candidate> {
        let node: LegacyReviewNode = toml::from_str(&std::fs::read_to_string(
            self.root.join("nodes").join(id).join("node.toml"),
        )?)?;
        let review = node
            .review
            .ok_or_else(|| anyhow::anyhow!("node `{id}` is not a review"))?;
        Ok(Candidate {
            id: id.into(),
            subject: review.implementation,
            result: review.reviewed_result,
            artifact: ArtifactRef {
                scheme: "git-commit".into(),
                repository: String::new(),
                id: review.candidate_commit,
            },
        })
    }

    pub fn create_candidate(&self, candidate: &Candidate) -> anyhow::Result<()> {
        let dir = self.dir(&candidate.id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("candidate.toml"),
            toml::to_string_pretty(candidate)?,
        )?;
        Ok(())
    }

    pub fn decision(&self, id: &str) -> anyhow::Result<Option<Decision>> {
        let path = self.dir(id).join("decision.toml");
        match std::fs::read_to_string(path) {
            Ok(data) => Ok(Some(toml::from_str(&data)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl CandidateStore for FsCandidateStore {
    type Error = anyhow::Error;
    fn candidate(&self, id: &str) -> Result<Candidate, Self::Error> {
        self.read_current(id).or_else(|_| self.read_legacy(id))
    }
    fn record_decision(&self, decision: &Decision) -> Result<(), Self::Error> {
        let candidate = self.candidate(&decision.candidate)?;
        validate_decision(&candidate, decision).map_err(anyhow::Error::msg)?;
        let dir = self.dir(&decision.candidate);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("decision.toml"), toml::to_string_pretty(decision)?)?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct LegacyReviewNode {
    review: Option<LegacyReviewTarget>,
}
#[derive(Deserialize)]
struct LegacyReviewTarget {
    implementation: String,
    candidate_commit: String,
    reviewed_result: ResultVersion,
}

pub trait Publisher {
    type Error;
    fn publish(
        &self,
        candidate: &Candidate,
        target: &str,
        expected_previous: &ArtifactRef,
    ) -> Result<bool, Self::Error>;
}

pub fn validate_decision(candidate: &Candidate, decision: &Decision) -> Result<(), &'static str> {
    if candidate.id != decision.candidate {
        return Err("decision targets a different candidate");
    }
    if decision.kind == DecisionKind::Rejected && decision.notes.trim().is_empty() {
        return Err("rejection needs comments");
    }
    if decision.kind == DecisionKind::Accepted && decision.suggestion.is_some() {
        return Err("accepted candidates cannot carry suggestions");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CandidateStore;

    #[test]
    fn legacy_review_node_is_a_candidate_but_new_decision_is_separate() {
        let root =
            std::env::temp_dir().join(format!("llaundry-review-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let node = root.join("nodes/review-1");
        std::fs::create_dir_all(&node).unwrap();
        std::fs::write(
            node.join("node.toml"),
            r#"
schema = 1
author = "machine"
[review]
implementation = "node-1"
attempt_id = "a"
candidate_branch = "branch"
candidate_commit = "commit"
[review.reviewed_result]
metadata = "rm"
notes = "rn"
"#,
        )
        .unwrap();
        let store = FsCandidateStore::new(&root);
        let candidate = store.candidate("review-1").unwrap();
        assert_eq!(candidate.subject, "node-1");
        store
            .record_decision(&Decision {
                candidate: "review-1".into(),
                kind: DecisionKind::Rejected,
                notes: "revise".into(),
                suggestion: None,
            })
            .unwrap();
        assert!(root.join("reviews/review-1/decision.toml").is_file());
        let _ = std::fs::remove_dir_all(root);
    }
}
