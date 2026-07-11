//! Review and publication policy over immutable artifacts.

use llaundry_core::{ArtifactRef, ResultVersion};
use serde::{Deserialize, Serialize};

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

pub trait CandidateStore {
    type Error;
    fn candidate(&self, id: &str) -> Result<Candidate, Self::Error>;
    fn record_decision(&self, decision: &Decision) -> Result<(), Self::Error>;
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
