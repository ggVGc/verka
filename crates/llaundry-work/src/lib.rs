//! Execution is an application consuming a work graph, not graph state.

pub mod backend;
pub mod config;

pub use config::{Config, CONFIG_FILE};

use llaundry_core::{ArtifactRef, Author, DefinitionVersion, ResultRecord, ResultVersion};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkedBy {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ExecutionIdentity {
    pub node_id: String,
    pub attempt_id: String,
    pub candidate_branch: String,
    pub force: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttemptFinal {
    pub at: i64,
    pub backend_succeeded: bool,
}

/// Compatibility form of the original durable attempt record. It remains
/// application-owned even while old stores and frontends use its field layout.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttemptMeta {
    pub schema: u32,
    pub id: String,
    pub node: String,
    pub worker: Author,
    pub force: bool,
    pub definition: DefinitionVersion,
    pub input_commit: String,
    pub input_tree: String,
    pub candidate_branch: String,
    pub worktree: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prepared: bool,
}

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
    Get {
        id: String,
    },
    Prepare {
        attempt: Attempt,
    },
    Finish {
        id: String,
        final_record: AttemptFinished,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Response {
    Attempt(Attempt),
    Ok,
    Error(String),
}

pub fn handle_request(store: &FsAttemptStore, request: Request) -> Response {
    let result: anyhow::Result<Response> = (|| {
        Ok(match request {
            Request::Get { id } => Response::Attempt(store.read(&id)?),
            Request::Prepare { attempt } => {
                store.create(&attempt)?;
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
        let attempt = Attempt {
            id: "a".into(),
            work_item: "n".into(),
            definition: DefinitionVersion {
                metadata: "m".into(),
                description: "d".into(),
            },
            input: ArtifactRef {
                scheme: "git-commit".into(),
                repository: root.to_string_lossy().into(),
                id: commit.clone(),
            },
            executor: "test".into(),
            workspace_id: path.to_string_lossy().into(),
            created_at: 0,
        };
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
        let path = PathBuf::from(&attempt.workspace_id);
        let branch = format!("llaundry/candidates/{}", attempt.id);
        let path_arg = path.to_string_lossy();
        self.git(
            &self.repository,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
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
            branch,
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
