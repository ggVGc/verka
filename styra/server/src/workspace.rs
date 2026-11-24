//! Durable Workspace metadata and directory layout.
//!
//! A Workspace is Styra's top-level unit of related work. It names one host
//! directory and owns any number of durable provider Sessions beneath
//! `workspaces/<workspace-id>/sessions/`.

use crate::protocol::{LaunchPolicy, WorkspaceLaunchChange, WorkspaceSummary};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const WORKSPACE_META_FILE: &str = "workspace.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceMeta {
    id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    host_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_repository: Option<PathBuf>,
    created_at_ms: u64,
    #[serde(default)]
    last_accessed_at_ms: Option<u64>,
    /// The Workspace's standing sandbox policy. Absent in metadata written
    /// before Workspaces carried one, which reads as the empty policy: every
    /// launch there then runs on its own inputs alone, exactly as it did.
    #[serde(default, skip_serializing_if = "LaunchPolicy::is_empty")]
    launch: LaunchPolicy,
}

pub fn workspaces_dir(store_root: &Path) -> PathBuf {
    store_root.join("workspaces")
}

pub fn workspace_dir(store_root: &Path, id: &str) -> PathBuf {
    workspaces_dir(store_root).join(id)
}

pub fn sessions_dir(store_root: &Path, workspace_id: &str) -> PathBuf {
    workspace_dir(store_root, workspace_id).join("sessions")
}

/// Durable parent for linked Git worktrees created on behalf of this Workspace.
pub fn worktrees_dir(store_root: &Path, workspace_id: &str) -> PathBuf {
    workspace_dir(store_root, workspace_id).join(styra_protocol::WORKTREES_DIR)
}

/// Create a durable Workspace for `host_path`.
///
/// Workspace identity is deliberately separate from the host path: two
/// independent bodies of work may use the same checkout.
pub fn create(
    store_root: &Path,
    host_path: &Path,
    name: Option<String>,
) -> Result<WorkspaceSummary> {
    create_with_repository(&crate::git::SystemGit, store_root, host_path, name, None)
}

/// `git` answers the questions that validate the association; a Workspace
/// created without a repository never asks it anything.
pub fn create_with_repository(
    git: &dyn crate::git::Git,
    store_root: &Path,
    host_path: &Path,
    name: Option<String>,
    git_repository: Option<&Path>,
) -> Result<WorkspaceSummary> {
    let host_path = host_path
        .canonicalize()
        .with_context(|| format!("workspace directory {} must exist", host_path.display()))?;
    let git_repository = git_repository
        .map(|path| validate_git_repository(git, path))
        .transpose()?;
    let created_at_ms = now_ms();
    let id = new_id(created_at_ms);
    let path = workspace_dir(store_root, &id);
    std::fs::create_dir_all(path.join("sessions"))
        .with_context(|| format!("creating Workspace directory {}", path.display()))?;
    let meta = WorkspaceMeta {
        id: id.clone(),
        deleted: false,
        name,
        host_path: host_path.clone(),
        git_repository: git_repository.clone(),
        created_at_ms,
        last_accessed_at_ms: Some(created_at_ms),
        launch: LaunchPolicy::default(),
    };
    write_meta(&path, &meta)?;
    Ok(WorkspaceSummary {
        id,
        name: meta.name,
        host_path,
        git_repository,
        path,
        session_count: 0,
        age: "just now".into(),
        created_at_ms,
        last_accessed_at_ms: created_at_ms,
        launch: LaunchPolicy::default(),
    })
}

/// List durable Workspaces by most recent access, newest first.
pub fn list(store_root: &Path) -> Result<Vec<WorkspaceSummary>> {
    list_by_deleted(store_root, false)
}

/// List only deleted Workspaces, without making them available for use.
pub fn list_deleted(store_root: &Path) -> Result<Vec<WorkspaceSummary>> {
    list_by_deleted(store_root, true)
}

fn list_by_deleted(store_root: &Path, deleted: bool) -> Result<Vec<WorkspaceSummary>> {
    let root = workspaces_dir(store_root);
    let now = now_ms();
    let mut workspaces = Vec::new();
    if root.exists() {
        for entry in
            std::fs::read_dir(&root).with_context(|| format!("reading {}", root.display()))?
        {
            let entry = entry.with_context(|| format!("reading an entry in {}", root.display()))?;
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let meta = read_meta(&entry.path())?;
            if meta.deleted == deleted {
                workspaces.push(summary_from_meta(&entry.path(), meta, now)?);
            }
        }
    }
    workspaces.sort_by(|a, b| {
        b.last_accessed_at_ms
            .cmp(&a.last_accessed_at_ms)
            .then_with(|| b.created_at_ms.cmp(&a.created_at_ms))
    });
    Ok(workspaces)
}

/// The Workspace covering a host directory, or `None` if none does.
///
/// `path` may be the Workspace's own host directory or anywhere beneath it,
/// which is what makes this answerable from a working directory rather than
/// from a Workspace root a client would have to know already.
///
/// Two rules settle the cases where more than one Workspace answers. The
/// innermost wins, so a Workspace over a subdirectory of another is not
/// shadowed by its parent; and among Workspaces over the *same* directory —
/// which Styra allows deliberately, two bodies of work in one checkout — the
/// most recently accessed wins, the same order the pickers show them in.
///
/// The path is canonicalized, so symlinked and relative spellings of one
/// directory find the same Workspace. A path that does not exist cannot be
/// canonicalized and therefore cannot be inside anything: that is an error,
/// not an empty answer, because it is a mistake by the caller rather than a
/// directory Styra happens not to know.
/// The path must be absolute: the server resolves it in its own process, and
/// a relative path would quietly mean a directory under the daemon's working
/// directory rather than the caller's.
pub fn for_path(store_root: &Path, path: &Path) -> Result<Option<WorkspaceSummary>> {
    if !path.is_absolute() {
        anyhow::bail!(
            "{} must be an absolute path; the server resolves it, not the caller's shell",
            path.display()
        );
    }
    let path = path
        .canonicalize()
        .with_context(|| format!("directory {} must exist", path.display()))?;
    let mut best: Option<WorkspaceSummary> = None;
    for workspace in list(store_root)? {
        if !path.starts_with(&workspace.host_path) {
            continue;
        }
        let deeper = match &best {
            // `list` is already ordered by access, newest first, so the first
            // Workspace at a given depth is the one to keep.
            Some(current) => {
                workspace.host_path.components().count() > current.host_path.components().count()
            }
            None => true,
        };
        if deeper {
            best = Some(workspace);
        }
    }
    Ok(best)
}

pub fn get(store_root: &Path, id: &str) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    summary_from_meta(&path, read_active_meta(&path)?, now_ms())
}

/// Record an explicit operator access and return the updated Workspace.
pub fn access(store_root: &Path, id: &str) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    // An access has to leave this Workspace at the front of `list`, and the
    // wall clock alone cannot promise that: two Workspaces touched inside one
    // millisecond tie, and the tie is broken by creation time, which would
    // leave a just-accessed Workspace behind a newer one. So the new reading
    // is stepped past every other Workspace's as well as past its own.
    let latest = list(store_root)?
        .iter()
        .map(|workspace| workspace.last_accessed_at_ms)
        .max()
        .unwrap_or(0);
    let mut meta = read_active_meta(&path)?;
    let previous = meta
        .last_accessed_at_ms
        .unwrap_or(meta.created_at_ms)
        .max(latest);
    meta.last_accessed_at_ms = Some(now_ms().max(previous.saturating_add(1)));
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

/// Mark a Workspace unavailable, preserving all of its files.
pub fn delete(store_root: &Path, id: &str) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    let mut meta = read_active_meta(&path)?;
    meta.deleted = true;
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

/// Restore the same identity, Sessions, and policy.
pub fn restore(store_root: &Path, id: &str) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    let mut meta = read_meta(&path)?;
    anyhow::ensure!(meta.deleted, "Workspace {id:?} is not deleted");
    meta.deleted = false;
    write_meta(&path, &meta)?;
    access(store_root, id)
}

fn read_active_meta(path: &Path) -> Result<WorkspaceMeta> {
    let meta = read_meta(path)?;
    anyhow::ensure!(
        !meta.deleted,
        "Workspace {:?} is deleted; restore it before use",
        meta.id
    );
    Ok(meta)
}

fn summary_from_meta(path: &Path, meta: WorkspaceMeta, now: u64) -> Result<WorkspaceSummary> {
    let session_root = path.join("sessions");
    let session_count = if session_root.is_dir() {
        std::fs::read_dir(&session_root)
            .with_context(|| format!("reading {}", session_root.display()))?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .count()
    } else {
        0
    };
    let last_accessed_at_ms = meta.last_accessed_at_ms.unwrap_or(meta.created_at_ms);
    Ok(WorkspaceSummary {
        id: meta.id,
        name: meta.name,
        host_path: meta.host_path,
        git_repository: meta.git_repository,
        path: path.to_path_buf(),
        session_count,
        age: humanize_age(now, meta.created_at_ms),
        created_at_ms: meta.created_at_ms,
        last_accessed_at_ms,
        launch: meta.launch,
    })
}

/// Replace a Workspace's durable Git checkout association.
pub fn set_git_repository(
    git: &dyn crate::git::Git,
    store_root: &Path,
    id: &str,
    git_repository: Option<&Path>,
) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    let mut meta = read_active_meta(&path)?;
    meta.git_repository = git_repository
        .map(|path| validate_git_repository(git, path))
        .transpose()?;
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

/// Point a Workspace at a different host directory.
///
/// The Workspace keeps its identity, Sessions, worktrees and policy; only the
/// directory later launches run in changes. Interactions already running stay
/// in the directory they were spawned in. The path must be absolute for the
/// same reason as in [`for_path`]: the server resolves it, not the caller.
pub fn set_host_path(store_root: &Path, id: &str, host_path: &Path) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    if !host_path.is_absolute() {
        anyhow::bail!(
            "{} must be an absolute path; the server resolves it, not the caller's shell",
            host_path.display()
        );
    }
    let host_path = host_path
        .canonicalize()
        .with_context(|| format!("workspace directory {} must exist", host_path.display()))?;
    if !host_path.is_dir() {
        anyhow::bail!(
            "workspace directory {} is not a directory",
            host_path.display()
        );
    }
    let mut meta = read_active_meta(&path)?;
    meta.host_path = host_path;
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

/// Change a Workspace's optional operator-facing name.
pub fn rename(store_root: &Path, id: &str, name: Option<&str>) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    let mut meta = read_active_meta(&path)?;
    meta.name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

/// Resolve every path needed at launch before making the association durable.
/// This prevents malformed linked-worktree metadata from poisoning all future
/// launches in the Workspace.
fn validate_git_repository(git: &dyn crate::git::Git, path: &Path) -> Result<PathBuf> {
    let root = git.repository_root(path)?;
    git.mounts(&root)?;
    Ok(root)
}

/// Read a Workspace's standing sandbox policy without marking it accessed.
pub fn launch(store_root: &Path, id: &str) -> Result<LaunchPolicy> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    Ok(read_active_meta(&path)?.launch)
}

/// Apply an edit to a Workspace's standing sandbox policy.
///
/// Stored with the Workspace rather than with the client that set it, so every
/// launch in this Workspace — from any client, on any machine sharing the store
/// — starts from the same grants. Interactions already running keep the policy
/// they were spawned under; a policy is applied at launch, not enforced live.
pub fn change_launch(
    store_root: &Path,
    id: &str,
    change: WorkspaceLaunchChange,
) -> Result<LaunchPolicy> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    let mut meta = read_active_meta(&path)?;
    match change {
        WorkspaceLaunchChange::SetNetwork(network) => meta.launch.network = network,
        WorkspaceLaunchChange::SetWritableWorkspace(writable) => {
            meta.launch.writable_workspace = writable
        }
        WorkspaceLaunchChange::SetTemplates(templates) => meta.launch.templates = templates,
        WorkspaceLaunchChange::AddMounts(mounts) => {
            for mount in mounts {
                if !meta.launch.mounts.contains(&mount) {
                    meta.launch.mounts.push(mount);
                }
            }
        }
        WorkspaceLaunchChange::RemoveMount(mount) => {
            meta.launch.mounts.retain(|candidate| candidate != &mount);
        }
        WorkspaceLaunchChange::Replace(launch) => meta.launch = launch,
        WorkspaceLaunchChange::SetCommand { action, command } => {
            *meta.launch.command_mut(action) = command.filter(|command| !command.trim().is_empty())
        }
    }
    // `ignore_workspace` says "ignore the layer below me", and a Workspace
    // policy has no layer below it.
    meta.launch.ignore_workspace = false;
    write_meta(&path, &meta)?;
    Ok(meta.launch)
}

fn write_meta(directory: &Path, meta: &WorkspaceMeta) -> Result<()> {
    let path = directory.join(WORKSPACE_META_FILE);
    let json = serde_json::to_string_pretty(meta).context("serializing Workspace metadata")?;
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temporary = directory.join(format!(
        ".{WORKSPACE_META_FILE}.tmp-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, json).with_context(|| format!("writing {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, &path) {
        std::fs::remove_file(&temporary).ok();
        return Err(error).with_context(|| format!("publishing {}", path.display()));
    }
    Ok(())
}

fn read_meta(directory: &Path) -> Result<WorkspaceMeta> {
    let path = directory.join(WORKSPACE_META_FILE);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_id(created_at_ms: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{created_at_ms:013}-{}-{sequence}", std::process::id())
}

fn humanize_age(now_ms: u64, created_at_ms: u64) -> String {
    let elapsed_secs = now_ms.saturating_sub(created_at_ms) / 1000;
    if elapsed_secs < 60 {
        "just now".into()
    } else if elapsed_secs < 3_600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3_600)
    } else {
        format!("{}d ago", elapsed_secs / 86_400)
    }
}

#[cfg(test)]
mod tests;
