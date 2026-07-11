//! Execution is an application consuming a work graph, not graph state.

pub mod backend;
pub mod config;

pub use config::{Config, CONFIG_FILE};

use llaundry_core::{ArtifactRef, DefinitionVersion, ResultRecord, ResultVersion};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Portable execution application messages for JSON-over-stdio adapters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    Prepare {
        attempt: Attempt,
    },
    Finish {
        id: String,
        final_record: AttemptFinished,
    },
}

/// File-backed execution state. New records live under `execution/`; the
/// legacy `attempts/` namespace remains readable during the schema transition.
pub struct FsAttemptStore {
    root: PathBuf,
}

impl FsAttemptStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, id: &str, file: &str) -> PathBuf {
        self.root.join("execution").join(id).join(file)
    }

    pub fn read(&self, id: &str) -> anyhow::Result<Attempt> {
        let current = self.path(id, "attempt.toml");
        if current.is_file() {
            return Ok(toml::from_str(&std::fs::read_to_string(current)?)?);
        }
        let legacy = self.root.join("attempts").join(id).join("attempt.toml");
        let old: LegacyAttempt = toml::from_str(&std::fs::read_to_string(&legacy)?)?;
        Ok(old.into_attempt())
    }

    pub fn transcript_path(&self, id: &str) -> PathBuf {
        let current = self.path(id, "work.jsonl");
        if current.exists() {
            current
        } else {
            self.root.join("attempts").join(id).join("work.jsonl")
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl AttemptStore for FsAttemptStore {
    type Error = anyhow::Error;

    fn create(&self, attempt: &Attempt) -> Result<(), Self::Error> {
        let path = self.path(&attempt.id, "attempt.toml");
        std::fs::create_dir_all(path.parent().expect("execution record has parent"))?;
        std::fs::write(path, toml::to_string_pretty(attempt)?)?;
        Ok(())
    }

    fn finish(&self, id: &str, final_record: &AttemptFinished) -> Result<(), Self::Error> {
        self.read(id)?;
        let path = self.path(id, "final.toml");
        std::fs::create_dir_all(path.parent().expect("execution record has parent"))?;
        std::fs::write(path, toml::to_string_pretty(final_record)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_attempts_and_writes_only_the_execution_namespace() {
        let root = std::env::temp_dir().join(format!("llaundry-work-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let legacy = root.join("attempts/a");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("attempt.toml"),
            r#"
id = "a"
node = "node-1"
worker = "machine"
force = false
input_commit = "abc"
input_tree = "tree"
candidate_branch = "llaundry/candidates/a"
worktree = "/tmp/a"
created_at = 1
prepared = true
[definition]
metadata = "m"
description = "d"
"#,
        )
        .unwrap();
        let store = FsAttemptStore::new(&root);
        let attempt = store.read("a").unwrap();
        assert_eq!(attempt.work_item, "node-1");
        assert_eq!(attempt.input.id, "abc");
        store
            .finish(
                "a",
                &AttemptFinished {
                    at: 2,
                    executor_succeeded: true,
                },
            )
            .unwrap();
        assert!(root.join("execution/a/final.toml").is_file());
        let _ = std::fs::remove_dir_all(root);
    }
}

#[derive(Deserialize)]
struct LegacyAttempt {
    id: String,
    node: String,
    definition: DefinitionVersion,
    input_commit: String,
    input_tree: String,
    candidate_branch: String,
    worktree: String,
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    model: Option<String>,
    created_at: i64,
}

impl LegacyAttempt {
    fn into_attempt(self) -> Attempt {
        Attempt {
            id: self.id,
            work_item: self.node,
            definition: self.definition,
            input: ArtifactRef {
                scheme: "git-commit".into(),
                repository: String::new(),
                id: self.input_commit,
            },
            executor: match (self.backend, self.model) {
                (Some(backend), Some(model)) => format!("{backend}:{model}"),
                (Some(backend), None) => backend,
                _ => "legacy".into(),
            },
            workspace_id: format!(
                "{}|{}|{}",
                self.worktree, self.candidate_branch, self.input_tree
            ),
            created_at: self.created_at,
        }
    }
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
