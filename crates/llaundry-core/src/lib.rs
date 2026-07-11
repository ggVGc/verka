//! The execution- and review-agnostic llaundry domain.
//!
//! This crate deliberately has no Git, workspace, agent, review, or publication
//! dependency. Storage and artifact systems implement the ports below.

use serde::{Deserialize, Serialize};

pub mod store;
pub use store::{FsGraphStore, NodeDefinition, NodeRecord};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Human,
    Machine,
}
impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Machine => "machine",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    DependsOn,
    DerivedFrom,
}
impl DepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on",
            Self::DerivedFrom => "derived_from",
        }
    }
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum GraphRequest {
    Get {
        id: String,
    },
    List,
    Add {
        definition: NodeDefinition,
        description: String,
    },
    Link {
        from: String,
        to: String,
        blocking: bool,
    },
    Edit {
        id: String,
        description: String,
    },
    Submit {
        id: String,
        result: ResultRecord,
        notes: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum GraphResponse {
    Node(NodeRecord),
    Nodes(Vec<String>),
    Id(String),
    ResultVersion(ResultVersion),
    Ok,
    Error(String),
}

pub fn handle_request(store: &FsGraphStore, request: GraphRequest) -> GraphResponse {
    let response: anyhow::Result<GraphResponse> = (|| {
        Ok(match request {
            GraphRequest::Get { id } => GraphResponse::Node(store.read_node(&id)?),
            GraphRequest::List => GraphResponse::Nodes(store.list_ids()?),
            GraphRequest::Add {
                definition,
                description,
            } => GraphResponse::Id(store.add(definition, description)?),
            GraphRequest::Link { from, to, blocking } => {
                store.link(&from, &to, blocking)?;
                GraphResponse::Ok
            }
            GraphRequest::Edit { id, description } => {
                store.edit(&id, description)?;
                GraphResponse::Ok
            }
            GraphRequest::Submit { id, result, notes } => {
                GraphResponse::ResultVersion(store.write_result(&id, &result, &notes)?)
            }
        })
    })();
    response.unwrap_or_else(|error| GraphResponse::Error(format!("{error:#}")))
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
impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
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
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
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

pub trait DependencyView: GraphView {
    fn exists(&self, id: &str) -> bool;
    fn dependencies(&self, id: &str) -> Result<Vec<String>, Self::Error>;
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

pub fn blockers<G: DependencyView, A: ArtifactResolver>(
    graph: &G,
    artifacts: &A,
    id: &str,
) -> Vec<String> {
    let Ok(dependencies) = graph.dependencies(id) else {
        return Vec::new();
    };
    let mut blockers = Vec::new();
    for dependency in dependencies {
        if !graph.exists(&dependency) {
            blockers.push(format!("{dependency}: missing"));
            continue;
        }
        let Ok(version) = graph.definition_version(&dependency) else {
            blockers.push(format!("{dependency}: not done (open)"));
            continue;
        };
        let result = graph.result(&dependency).ok().flatten();
        match status(&version, result.as_ref()) {
            Status::Done if !staleness(graph, artifacts, &dependency).is_empty() => {
                blockers.push(format!("{dependency}: stale"))
            }
            Status::Done => {}
            Status::Open => blockers.push(format!("{dependency}: not done (open)")),
            Status::Failed => blockers.push(format!("{dependency}: not done (failed)")),
        }
    }
    blockers
}

pub fn is_ready<G: DependencyView, A: ArtifactResolver>(
    graph: &G,
    artifacts: &A,
    id: &str,
) -> bool {
    if !blockers(graph, artifacts, id).is_empty() {
        return false;
    }
    let Ok(version) = graph.definition_version(id) else {
        return false;
    };
    let result = graph.result(id).ok().flatten();
    status(&version, result.as_ref()) != Status::Done
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
