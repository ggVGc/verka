//! Execution is an application consuming a work graph, not graph state.

use llaundry_core::{ArtifactRef, DefinitionVersion, ResultRecord, ResultVersion};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub work_item: String,
    pub definition: DefinitionVersion,
    pub input: ArtifactRef,
    pub executor: String,
    pub workspace_id: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttemptFinished {
    pub at: i64,
    pub executor_succeeded: bool,
}

pub trait WorkProvider {
    type Error;
    fn definition_version(&self, id: &str) -> Result<DefinitionVersion, Self::Error>;
    fn ready(&self, id: &str) -> Result<bool, Self::Error>;
    fn submit(
        &self,
        id: &str,
        result: &ResultRecord,
        notes: &str,
    ) -> Result<ResultVersion, Self::Error>;
}

pub trait AttemptStore {
    type Error;
    fn create(&self, attempt: &Attempt) -> Result<(), Self::Error>;
    fn finish(&self, id: &str, final_record: &AttemptFinished) -> Result<(), Self::Error>;
}

pub trait WorkspaceManager {
    type Error;
    type Workspace;
    fn prepare(&self, attempt: &Attempt) -> Result<Self::Workspace, Self::Error>;
    fn clean(&self, workspace: &Self::Workspace) -> Result<bool, Self::Error>;
    fn remove(&self, workspace: &Self::Workspace) -> Result<(), Self::Error>;
}
