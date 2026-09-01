//! Durable linked worktrees owned by a Styra Workspace.
//!
//! The containing repository is discovered from the Workspace's host path,
//! while checkouts themselves live below the Workspace's state directory. The
//! whole parent is mounted into an interaction once, so a worktree created by a
//! host-side tool call appears immediately without changing Driva's mount table.

use crate::agent::MountSpec;
use crate::git::{self, Repository};
use anyhow::{Context, Result};
use genta::appserver::DynamicTool;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const CREATE_WORKTREE_TOOL: &str = "create_worktree";

/// Host and sandbox locations needed to create and expose linked worktrees.
#[derive(Clone, Debug)]
pub struct Worktrees {
    repository: Repository,
    host_root: PathBuf,
    sandbox_root: PathBuf,
}

impl Worktrees {
    /// Prepare one Workspace's durable worktree parent.
    pub fn prepare(
        repository: Repository,
        host_root: PathBuf,
        sandbox_root: PathBuf,
    ) -> Result<Self> {
        std::fs::create_dir_all(&host_root).with_context(|| {
            format!(
                "creating Workspace worktree directory {}",
                host_root.display()
            )
        })?;
        Ok(Self {
            repository,
            host_root,
            sandbox_root,
        })
    }

    /// Automatic mounts added to every interaction in this Workspace.
    ///
    /// The common directory is writable because linked-worktree indexes,
    /// branch refs, and commits all update it. It is mounted at its host path:
    /// that is the absolute location Git writes into each worktree's `.git`
    /// pointer file.
    pub fn mounts(&self) -> Vec<MountSpec> {
        vec![
            MountSpec {
                source: self.host_root.clone(),
                destination: self.sandbox_root.clone(),
                writable: true,
            },
            MountSpec {
                source: self.repository.common_dir.clone(),
                destination: self.repository.common_dir.clone(),
                writable: true,
            },
        ]
    }

    /// The Codex app-server function backed by [`Self::create`].
    pub fn tool(&self) -> DynamicTool {
        let worktrees = self.clone();
        DynamicTool::new(
            CREATE_WORKTREE_TOOL,
            "Create a new Git branch and linked worktree for independent work. Returns the path to use inside the sandbox.",
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The new branch name, for example feature/search-index."
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            move |arguments| {
                let name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "name must be a string".to_owned())?;
                worktrees
                    .create(name)
                    .map(|created| {
                        format!(
                            "Created branch {:?} in worktree {}",
                            created.branch,
                            created.sandbox_path.display()
                        )
                    })
                    .map_err(|error| format!("{error:#}"))
            },
        )
    }

    /// Create `name` as both a new branch and a linked checkout.
    pub fn create(&self, name: &str) -> Result<CreatedWorktree> {
        validate_branch_name(&self.repository.root, name)?;
        let directory = encoded_directory_name(name);
        let host_path = self.host_root.join(&directory);
        if std::fs::symlink_metadata(&host_path).is_ok() {
            anyhow::bail!("a worktree location already exists for branch {name:?}");
        }

        git::create_worktree(&self.repository.root, name, &host_path)?;

        Ok(CreatedWorktree {
            branch: name.to_owned(),
            host_path,
            sandbox_path: self.sandbox_root.join(directory),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedWorktree {
    pub branch: String,
    pub host_path: PathBuf,
    pub sandbox_path: PathBuf,
}

fn validate_branch_name(repository: &Path, name: &str) -> Result<()> {
    if name.is_empty() || name.trim() != name {
        anyhow::bail!(
            "worktree name must be a non-empty Git branch name without surrounding whitespace"
        );
    }
    if !git::branch_name_is_valid(repository, name)? {
        anyhow::bail!("invalid Git branch name {name:?}");
    }
    Ok(())
}

/// Encode a ref as one flat directory component. Keeping `/` encoded prevents
/// a branch such as `feature/ui` from creating agent-writable intermediate
/// directories that could later be replaced with symlinks before another host
/// call.
fn encoded_directory_name(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    fn temporary_directory(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "styra-worktrees-{tag}-{}-{}",
            std::process::id(),
            crate::journal::now_ms()
        ))
    }

    fn repository(tag: &str) -> (PathBuf, Repository) {
        let root = temporary_directory(tag);
        let checkout = root.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        git::fixture::init(&checkout);
        git::fixture::commit_empty(&checkout, "initial");
        let repository = git::discover(&checkout).unwrap().unwrap();
        (root, repository)
    }

    #[test]
    fn creates_a_branch_in_the_workspace_specific_location() {
        let (root, repository) = repository("create");
        let host_root = root.join("state/worktrees");
        let worktrees = Worktrees::prepare(
            repository.clone(),
            host_root.clone(),
            "/tmp/styra/worktrees".into(),
        )
        .unwrap();

        let created = worktrees.create("feature/tool").unwrap();
        assert_eq!(created.host_path, host_root.join("feature%2Ftool"));
        assert_eq!(
            created.sandbox_path,
            PathBuf::from("/tmp/styra/worktrees/feature%2Ftool")
        );
        assert_eq!(
            git::current_branch(&created.host_path).unwrap().as_deref(),
            Some("feature/tool")
        );
        assert!(created.host_path.join(".git").is_file());

        let mounts = worktrees.mounts();
        assert_eq!(mounts[0].source, host_root);
        assert_eq!(mounts[0].destination, Path::new("/tmp/styra/worktrees"));
        assert_eq!(mounts[1].source, repository.common_dir);
        assert!(mounts.iter().all(|mount| mount.writable));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_and_duplicate_names() {
        let (root, repository) = repository("reject");
        let worktrees = Worktrees::prepare(
            repository,
            root.join("state/worktrees"),
            "/tmp/styra/worktrees".into(),
        )
        .unwrap();

        assert!(worktrees.create("../escape").is_err());
        worktrees.create("topic").unwrap();
        assert!(worktrees.create("topic").is_err());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_dynamic_tool_calls_create_the_host_worktree() {
        let (root, repository) = repository("tool");
        let host_root = root.join("state/worktrees");
        let worktrees =
            Worktrees::prepare(repository, host_root.clone(), "/tmp/styra/worktrees".into())
                .unwrap();
        let client = genta::appserver::AppServer::new("/tmp/styra/workspace".into())
            .with_dynamic_tools(vec![worktrees.tool()]);

        let actions = client.handle_line(
            r#"{"id":"tool-1","method":"item/tool/call","params":{"tool":"create_worktree","arguments":{"name":"agent/topic"}}}"#,
        );
        let response: Value = actions
            .iter()
            .find_map(|action| match action {
                genta::appserver::Action::Send(line) => serde_json::from_str(line).ok(),
                _ => None,
            })
            .unwrap();

        assert_eq!(response["result"]["success"], true);
        assert!(response["result"]["contentItems"][0]["text"]
            .as_str()
            .unwrap()
            .contains("/tmp/styra/worktrees/agent%2Ftopic"));
        assert!(host_root.join("agent%2Ftopic/.git").is_file());

        std::fs::remove_dir_all(root).unwrap();
    }
}
