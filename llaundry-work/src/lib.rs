//! Execution is an application consuming a work graph, not graph state.
//!
//! PARKED: this project collects everything execution- and review-related
//! from the old composed workspace (see README.md). It is not expected to
//! build yet — the plan is a `TaskStore` trait implemented by an adapter for
//! the llaundry library, replacing the direct `llaundry_core` paths below.

pub mod backend;
pub mod config;
pub mod harness;
pub mod protocol;
pub mod review;

pub use config::{Config, CONFIG_FILE};

use llaundry::store::blob_id;
use llaundry::{ArtifactRef, Author, DefinitionVersion, ProducerEvidence, ResultMeta as ResultRecord, ResultVersion};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Producer evidence namespace owned by this application.
pub const EVIDENCE_NAMESPACE: &str = "llaundry-work";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkedBy {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// The execution application's producer evidence carried on a submitted
/// result: which attempt produced it and, once known, on what backend/model.
/// Opaque namespaced data to the graph; only this application interprets it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkEvidence {
    /// The durable attempt that produced the result; absent for work stamped
    /// outside an isolated execution (e.g. an interactive session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl WorkEvidence {
    pub fn to_producer(&self) -> ProducerEvidence {
        ProducerEvidence {
            namespace: EVIDENCE_NAMESPACE.into(),
            data: serde_json::to_value(self).expect("work evidence serializes"),
        }
    }
    pub fn from_producer(producer: &ProducerEvidence) -> Option<Self> {
        (producer.namespace == EVIDENCE_NAMESPACE)
            .then(|| serde_json::from_value(producer.data.clone()).ok())
            .flatten()
    }
    pub fn worked_by(&self) -> Option<WorkedBy> {
        self.backend.clone().map(|backend| WorkedBy {
            backend,
            model: self.model.clone(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct ExecutionIdentity {
    pub node_id: String,
    pub attempt_id: String,
    pub candidate_branch: String,
    pub force: bool,
}

/// The durable execution attempt: identity, frozen inputs, and the local
/// workspace it runs in. Workspace paths and backend evidence are operational
/// records owned here; a submitted result carries only portable provenance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub schema: u32,
    pub id: String,
    pub work_item: String,
    pub worker: Author,
    pub force: bool,
    pub definition: DefinitionVersion,
    pub input: ArtifactRef,
    pub input_tree: String,
    pub branch: String,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prepared: bool,
}

/// Sealed backend-exit evidence for one attempt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttemptFinished {
    pub at: i64,
    pub executor_succeeded: bool,
}

/// Portable execution application messages for JSON-over-stdio adapters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    Get {
        id: String,
    },
    Prepare {
        attempt: Box<Attempt>,
    },
    Finish {
        id: String,
        final_record: AttemptFinished,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Response {
    Attempt(Box<Attempt>),
    Ok,
    Error(String),
}

pub fn handle_request(store: &FsAttemptStore, request: Request) -> Response {
    let result: anyhow::Result<Response> = (|| {
        Ok(match request {
            Request::Get { id } => Response::Attempt(Box::new(store.read(&id)?)),
            Request::Prepare { attempt } => {
                store.write(&attempt)?;
                Response::Ok
            }
            Request::Finish { id, final_record } => {
                store.finish(&id, &final_record)?;
                Response::Ok
            }
        })
    })();
    result.unwrap_or_else(|error| Response::Error(format!("{error:#}")))
}

/// File-backed execution state under the `execution/` namespace this
/// application owns: the attempt record, its transcript, its attempt-scoped
/// result, and the sealed final record.
pub struct FsAttemptStore {
    root: PathBuf,
}

impl FsAttemptStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join("execution").join(id)
    }

    fn path(&self, id: &str, file: &str) -> PathBuf {
        self.dir(id).join(file)
    }

    pub fn read(&self, id: &str) -> anyhow::Result<Attempt> {
        let data = std::fs::read_to_string(self.path(id, "attempt.toml"))
            .map_err(|_| anyhow::anyhow!("unknown attempt `{id}`"))?;
        Ok(toml::from_str(&data)?)
    }

    pub fn write(&self, attempt: &Attempt) -> anyhow::Result<()> {
        let path = self.path(&attempt.id, "attempt.toml");
        std::fs::create_dir_all(path.parent().expect("execution record has parent"))?;
        std::fs::write(path, toml::to_string_pretty(attempt)?)?;
        Ok(())
    }

    pub fn list_ids(&self) -> anyhow::Result<Vec<String>> {
        let dir = self.root.join("execution");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn write_result(
        &self,
        id: &str,
        result: &ResultRecord,
        notes: &str,
    ) -> anyhow::Result<()> {
        self.read(id)?;
        std::fs::write(self.path(id, "result.toml"), toml::to_string_pretty(result)?)?;
        if notes.is_empty() {
            let _ = std::fs::remove_file(self.path(id, "result.md"));
        } else {
            std::fs::write(self.path(id, "result.md"), notes)?;
        }
        Ok(())
    }

    pub fn read_result(&self, id: &str) -> anyhow::Result<Option<(ResultRecord, String)>> {
        let data = match std::fs::read_to_string(self.path(id, "result.toml")) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let result = toml::from_str(&data)?;
        let notes = std::fs::read_to_string(self.path(id, "result.md")).unwrap_or_default();
        Ok(Some((result, notes)))
    }

    pub fn result_version(&self, id: &str) -> anyhow::Result<ResultVersion> {
        let metadata = std::fs::read(self.path(id, "result.toml"))?;
        let notes = std::fs::read(self.path(id, "result.md"))
            .ok()
            .map(|bytes| blob_id(&bytes));
        Ok(ResultVersion {
            metadata: blob_id(&metadata),
            notes,
        })
    }

    pub fn read_final(&self, id: &str) -> anyhow::Result<Option<AttemptFinished>> {
        let data = match std::fs::read_to_string(self.path(id, "final.toml")) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(toml::from_str(&data)?))
    }

    pub fn transcript_path(&self, id: &str) -> PathBuf {
        self.path(id, "work.jsonl")
    }

    pub fn open_transcript(&self, id: &str, append: bool) -> anyhow::Result<std::fs::File> {
        self.read(id)?;
        Ok(std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(self.transcript_path(id))?)
    }

    pub fn read_transcript(&self, id: &str) -> anyhow::Result<Option<String>> {
        match std::fs::read_to_string(self.transcript_path(id)) {
            Ok(log) => Ok(Some(log)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl AttemptStore for FsAttemptStore {
    type Error = anyhow::Error;

    fn create(&self, attempt: &Attempt) -> Result<(), Self::Error> {
        self.write(attempt)
    }

    fn finish(&self, id: &str, final_record: &AttemptFinished) -> Result<(), Self::Error> {
        self.read(id)?;
        std::fs::write(
            self.path(id, "final.toml"),
            toml::to_string_pretty(final_record)?,
        )?;
        Ok(())
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

pub fn resolve_ready_work<P: WorkProvider>(
    provider: &P,
    id: &str,
) -> Result<DefinitionVersion, ResolveError<P::Error>> {
    if !provider.ready(id).map_err(ResolveError::Provider)? {
        return Err(ResolveError::NotReady);
    }
    provider
        .definition_version(id)
        .map_err(ResolveError::Provider)
}

#[derive(Debug)]
pub enum ResolveError<E> {
    NotReady,
    Provider(E),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitWorkspace {
    pub path: PathBuf,
    pub branch: String,
    pub input_commit: String,
    pub input_tree: String,
}

pub struct GitWorkspaceManager {
    repository: PathBuf,
}
impl GitWorkspaceManager {
    pub fn new(repository: impl Into<PathBuf>) -> Self {
        Self {
            repository: repository.into(),
        }
    }
    fn git(&self, cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().into())
    }
}
impl WorkspaceManager for GitWorkspaceManager {
    type Error = anyhow::Error;
    type Workspace = GitWorkspace;
    fn prepare(&self, attempt: &Attempt) -> Result<GitWorkspace, Self::Error> {
        if attempt.input.scheme != "git-commit" {
            anyhow::bail!("Git workspace requires a git-commit input");
        }
        let path = PathBuf::from(&attempt.workspace);
        let path_arg = path.to_string_lossy();
        self.git(
            &self.repository,
            &[
                "worktree",
                "add",
                "-b",
                &attempt.branch,
                &path_arg,
                &attempt.input.id,
            ],
        )?;
        let input_commit = self.git(&path, &["rev-parse", "HEAD"])?;
        let input_tree = self.git(&path, &["rev-parse", "HEAD^{tree}"])?;
        if input_commit != attempt.input.id {
            anyhow::bail!("prepared workspace resolved a different input");
        }
        Ok(GitWorkspace {
            path,
            branch: attempt.branch.clone(),
            input_commit,
            input_tree,
        })
    }
    fn clean(&self, workspace: &GitWorkspace) -> Result<bool, Self::Error> {
        Ok(self
            .git(&workspace.path, &["status", "--porcelain"])?
            .is_empty())
    }
    fn remove(&self, workspace: &GitWorkspace) -> Result<(), Self::Error> {
        let path = workspace.path.to_string_lossy();
        self.git(&self.repository, &["worktree", "remove", &path])?;
        Ok(())
    }
}

/// Standard post-execution workspace retention policy. Only successful,
/// clean, non-project executions are disposable without explicit consent.
pub fn should_remove_workspace(
    keep: bool,
    executor_succeeded: bool,
    produced_project_artifact: bool,
    workspace_clean: bool,
) -> bool {
    !keep && executor_succeeded && !produced_project_artifact && workspace_clean
}

#[cfg(test)]
mod tests {
    use super::*;
    use llaundry_core::Outcome;

    fn attempt(id: &str, root: &Path) -> Attempt {
        Attempt {
            schema: 1,
            id: id.into(),
            work_item: "node-1".into(),
            worker: Author::Machine,
            force: false,
            definition: DefinitionVersion {
                metadata: "m".into(),
                description: "d".into(),
            },
            input: ArtifactRef {
                scheme: "git-commit".into(),
                repository: root.to_string_lossy().into(),
                id: "abc".into(),
            },
            input_tree: "tree".into(),
            branch: "llaundry/candidates/a".into(),
            workspace: "/tmp/a".into(),
            backend: Some("test".into()),
            model: None,
            created_at: 1,
            prepared: true,
        }
    }

    #[test]
    fn attempt_records_live_only_in_the_execution_namespace() {
        let root = std::env::temp_dir().join(format!("llaundry-work-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = FsAttemptStore::new(&root);
        store.write(&attempt("a", &root)).unwrap();
        let read = store.read("a").unwrap();
        assert_eq!(read.work_item, "node-1");
        assert_eq!(read.input.id, "abc");
        assert_eq!(store.list_ids().unwrap(), vec!["a".to_string()]);
        let result = ResultRecord {
            at: 2,
            author: Author::Machine,
            definition: read.definition.clone(),
            outcome: Outcome::Done,
            consumed: vec![],
            context: vec![],
            output: None,
            producer: Some(
                WorkEvidence {
                    attempt: Some("a".into()),
                    backend: Some("test".into()),
                    model: None,
                }
                .to_producer(),
            ),
        };
        store.write_result("a", &result, "did it").unwrap();
        let (read_result, notes) = store.read_result("a").unwrap().unwrap();
        assert_eq!(notes, "did it");
        let evidence =
            WorkEvidence::from_producer(read_result.producer.as_ref().unwrap()).unwrap();
        assert_eq!(evidence.attempt.as_deref(), Some("a"));
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
        assert!(!root.join("attempts").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn git_workspace_manager_prepares_and_removes_isolated_branch() {
        let root = std::env::temp_dir().join(format!("llaundry-work-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| -> String {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().into()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("x"), "one").unwrap();
        git(&["add", "x"]);
        git(&["commit", "-m", "one"]);
        let commit = git(&["rev-parse", "HEAD"]);
        let path = root.with_extension("workspace");
        let _ = std::fs::remove_dir_all(&path);
        let mut attempt = attempt("a", &root);
        attempt.input.id = commit.clone();
        attempt.workspace = path.to_string_lossy().into();
        let manager = GitWorkspaceManager::new(&root);
        let workspace = manager.prepare(&attempt).unwrap();
        assert_eq!(workspace.input_commit, commit);
        assert!(manager.clean(&workspace).unwrap());
        manager.remove(&workspace).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    struct FakeProvider {
        ready: bool,
    }
    impl WorkProvider for FakeProvider {
        type Error = &'static str;
        fn definition_version(&self, _id: &str) -> Result<DefinitionVersion, Self::Error> {
            Ok(DefinitionVersion {
                metadata: "m".into(),
                description: "d".into(),
            })
        }
        fn ready(&self, _id: &str) -> Result<bool, Self::Error> {
            Ok(self.ready)
        }
        fn submit(
            &self,
            _id: &str,
            _result: &ResultRecord,
            _notes: &str,
        ) -> Result<ResultVersion, Self::Error> {
            Err("unused")
        }
    }

    #[test]
    fn runner_resolves_work_through_provider_interface() {
        assert!(resolve_ready_work(&FakeProvider { ready: true }, "external-id").is_ok());
        assert!(matches!(
            resolve_ready_work(&FakeProvider { ready: false }, "external-id"),
            Err(ResolveError::NotReady)
        ));
    }
}
