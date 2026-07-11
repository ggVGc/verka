//! The execution- and review-agnostic llaundry domain.
//!
//! This crate deliberately has no Git, workspace, agent, review, or publication
//! dependency. Storage and artifact systems implement the ports below.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Versioned JSON envelope used by out-of-process graph adapters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub schema: u32,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(payload: T) -> Self {
        Self {
            schema: PROTOCOL_VERSION,
            payload,
        }
    }
    pub fn validate(self) -> Result<T, String> {
        if self.schema == PROTOCOL_VERSION {
            Ok(self.payload)
        } else {
            Err(format!("unsupported protocol schema {}", self.schema))
        }
    }
}

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

/// Read-only graph projection used by derived queries. Implementations may be
/// a file store, service client, database adapter, or test double.
pub trait GraphView {
    type Error: std::fmt::Display;
    fn definition_version(&self, id: &str) -> Result<DefinitionVersion, Self::Error>;
    fn result(&self, id: &str) -> Result<Option<ResultRecord>, Self::Error>;
    fn result_version(&self, id: &str) -> Result<Option<ResultVersion>, Self::Error>;
}

pub trait ArtifactResolver {
    type Error: std::fmt::Display;
    fn exists(&self, artifact: &ArtifactRef) -> Result<bool, Self::Error>;
    fn files(&self, artifact: &ArtifactRef) -> Result<Vec<String>, Self::Error>;
    fn drift(&self, artifact: &ArtifactRef) -> Result<Option<String>, Self::Error>;
    fn context_identity(&self, path: &str) -> Result<Option<String>, Self::Error>;
}

/// Derive every reason recorded work no longer matches its inputs or output.
/// Review and execution evidence are intentionally absent from this query.
pub fn staleness<G: GraphView, A: ArtifactResolver>(
    graph: &G,
    artifacts: &A,
    id: &str,
) -> Vec<String> {
    let Ok(Some(result)) = graph.result(id) else {
        return Vec::new();
    };
    let mut reasons = Vec::new();
    match graph.definition_version(id) {
        Ok(current) if current != result.definition => {
            let mut detail = Vec::new();
            if current.metadata != result.definition.metadata {
                detail.push("node.toml changed");
            }
            if current.description != result.definition.description {
                detail.push("description.md changed");
            }
            reasons.push(format!(
                "definition changed since the work ({})",
                detail.join(", ")
            ));
        }
        Err(_) => reasons.push("definition missing or unreadable since the work".into()),
        _ => {}
    }
    for consumed in &result.consumed {
        match graph.definition_version(&consumed.id) {
            Ok(current) if current != consumed.definition => {
                reasons.push(format!("dependency {}: definition moved", consumed.id))
            }
            Err(_) => reasons.push(format!("dependency {}: missing", consumed.id)),
            _ => {}
        }
        if graph.result_version(&consumed.id).ok().flatten() != consumed.result {
            reasons.push(format!(
                "dependency {}: result changed since it was consumed",
                consumed.id
            ));
        }
        let current_output = graph
            .result(&consumed.id)
            .ok()
            .flatten()
            .and_then(|r| r.output);
        if current_output != consumed.output {
            reasons.push(format!("dependency {}: output changed", consumed.id));
        }
    }
    for pin in &result.context {
        match artifacts.context_identity(&pin.path) {
            Ok(Some(now)) if now != pin.identity => {
                reasons.push(format!("context {}: content changed", pin.path))
            }
            Ok(None) => reasons.push(format!("context {}: missing", pin.path)),
            Err(error) => reasons.push(format!("context {}: check failed: {error}", pin.path)),
            _ => {}
        }
    }
    if let Some(output) = &result.output {
        match artifacts.drift(output) {
            Ok(Some(reason)) => reasons.push(format!(
                "output changed since {}:\n      {}",
                output.id,
                reason.replace('\n', "\n      ")
            )),
            Err(error) => reasons.push(format!("output check failed ({}): {error}", output.id)),
            _ => {}
        }
    }
    reasons
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
