//! The execution- and review-agnostic llaundry domain.
//!
//! This crate deliberately has no Git, workspace, agent, review, or publication
//! dependency. Storage and artifact systems implement the ports below.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionVersion {
    pub metadata: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultVersion {
    pub metadata: String,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub scheme: String,
    pub repository: String,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumedNode {
    pub id: String,
    pub definition: DefinitionVersion,
    pub result: Option<ResultVersion>,
    pub output: Option<ArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPin {
    pub path: String,
    pub identity: String,
    #[serde(default)]
    pub observed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProducerEvidence {
    pub namespace: String,
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultRecord {
    pub definition: DefinitionVersion,
    pub outcome: Outcome,
    pub consumed: Vec<ConsumedNode>,
    pub context: Vec<ContextPin>,
    pub output: Option<ArtifactRef>,
    pub producer: Option<ProducerEvidence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    Done,
    Failed,
}

pub fn status(current: &DefinitionVersion, result: Option<&ResultRecord>) -> Status {
    match result {
        None => Status::Open,
        Some(result) if result.outcome == Outcome::Failed => Status::Failed,
        Some(result) if &result.definition == current => Status::Done,
        Some(_) => Status::Open,
    }
}

pub trait WorkGraph {
    type Error;
    fn definition_version(&self, id: &str) -> Result<DefinitionVersion, Self::Error>;
    fn result(&self, id: &str) -> Result<Option<ResultRecord>, Self::Error>;
    fn submit(
        &self,
        id: &str,
        result: &ResultRecord,
        notes: &str,
    ) -> Result<ResultVersion, Self::Error>;
}

pub trait ArtifactResolver {
    type Error;
    fn exists(&self, artifact: &ArtifactRef) -> Result<bool, Self::Error>;
    fn files(&self, artifact: &ArtifactRef) -> Result<Vec<String>, Self::Error>;
    fn drift(&self, artifact: &ArtifactRef) -> Result<Option<String>, Self::Error>;
    fn context_identity(&self, path: &str) -> Result<Option<String>, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_cannot_affect_core_status() {
        let version = DefinitionVersion {
            metadata: "m".into(),
            description: "d".into(),
        };
        let result = ResultRecord {
            definition: version.clone(),
            outcome: Outcome::Done,
            consumed: vec![],
            context: vec![],
            output: None,
            producer: None,
        };
        assert_eq!(status(&version, Some(&result)), Status::Done);
    }
}
