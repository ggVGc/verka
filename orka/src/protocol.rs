//! Versioned request/response protocol for out-of-process graph adapters,
//! extracted from the former linka-core crate.
//!
//! PARKED: not expected to compile yet. `FsGraphStore` no longer exists — the
//! graph store folded back into the linka library. This protocol is the
//! embryo of the planned task-store trait: orka should define the
//! trait and implement it with an adapter for linka (in-process or over
//! this wire format).

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

/// The write seam an execution driver needs into a task graph.
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
