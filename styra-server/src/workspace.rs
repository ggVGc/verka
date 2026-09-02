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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    notes: String,
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

/// Create a durable Workspace for `host_path`.
///
/// Workspace identity is deliberately separate from the host path: two
/// independent bodies of work may use the same checkout.
pub fn create(
    store_root: &Path,
    host_path: &Path,
    name: Option<String>,
) -> Result<WorkspaceSummary> {
    create_with_repository(store_root, host_path, name, None)
}

pub fn create_with_repository(
    store_root: &Path,
    host_path: &Path,
    name: Option<String>,
    git_repository: Option<&Path>,
) -> Result<WorkspaceSummary> {
    let host_path = host_path
        .canonicalize()
        .with_context(|| format!("workspace directory {} must exist", host_path.display()))?;
    let git_repository = git_repository.map(validate_git_repository).transpose()?;
    let created_at_ms = now_ms();
    let id = new_id(created_at_ms);
    let path = workspace_dir(store_root, &id);
    std::fs::create_dir_all(path.join("sessions"))
        .with_context(|| format!("creating Workspace directory {}", path.display()))?;
    let meta = WorkspaceMeta {
        id: id.clone(),
        name,
        notes: String::new(),
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
        notes: meta.notes,
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
            workspaces.push(summary_from_dir(&entry.path(), now)?);
        }
    }
    workspaces.sort_by(|a, b| {
        b.last_accessed_at_ms
            .cmp(&a.last_accessed_at_ms)
            .then_with(|| b.created_at_ms.cmp(&a.created_at_ms))
    });
    Ok(workspaces)
}

pub fn get(store_root: &Path, id: &str) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    summary_from_dir(&path, now_ms())
}

/// Record an explicit operator access and return the updated Workspace.
pub fn access(store_root: &Path, id: &str) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    let mut meta = read_meta(&path)?;
    let previous = meta.last_accessed_at_ms.unwrap_or(meta.created_at_ms);
    meta.last_accessed_at_ms = Some(now_ms().max(previous.saturating_add(1)));
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

fn summary_from_dir(path: &Path, now: u64) -> Result<WorkspaceSummary> {
    let meta = read_meta(path)?;
    summary_from_meta(path, meta, now)
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
        notes: meta.notes,
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

/// Replace a Workspace's notes. Notes are plain UTF-8 text and may be empty.
pub fn store_notes(store_root: &Path, id: &str, notes: String) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    let mut meta = read_meta(&path)?;
    meta.notes = notes;
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

/// Replace a Workspace's durable Git checkout association.
pub fn set_git_repository(
    store_root: &Path,
    id: &str,
    git_repository: Option<&Path>,
) -> Result<WorkspaceSummary> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    let mut meta = read_meta(&path)?;
    meta.git_repository = git_repository.map(validate_git_repository).transpose()?;
    write_meta(&path, &meta)?;
    summary_from_meta(&path, meta, now_ms())
}

/// Resolve every path needed at launch before making the association durable.
/// This prevents malformed linked-worktree metadata from poisoning all future
/// launches in the Workspace.
fn validate_git_repository(path: &Path) -> Result<PathBuf> {
    let root = crate::git::repository_root(path)?;
    crate::git::mounts(&root)?;
    Ok(root)
}

/// Read a Workspace's standing sandbox policy without marking it accessed.
pub fn launch(store_root: &Path, id: &str) -> Result<LaunchPolicy> {
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    Ok(read_meta(&path)?.launch)
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
    let mut meta = read_meta(&path)?;
    match change {
        WorkspaceLaunchChange::SetNetwork(network) => meta.launch.network = network,
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
    }
    // `standalone` says "ignore the layer below me", and a Workspace policy has
    // no layer below it.
    meta.launch.standalone = false;
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
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("styra-workspace-{tag}-{}", new_id(now_ms())));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn git_init(directory: &Path) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["init", "--quiet"])
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn creates_and_lists_distinct_workspaces_for_the_same_host_path() {
        let store = temp_dir("store");
        let host = temp_dir("host");
        let first = create(&store, &host, Some("first".into())).unwrap();
        let second = create(&store, &host, Some("second".into())).unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!(first.host_path, host.canonicalize().unwrap());
        assert!(first.path.join("workspace.json").is_file());
        assert!(first.path.join("sessions").is_dir());

        let listed = list(&store).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id);
        assert_eq!(
            get(&store, &first.id).unwrap().name.as_deref(),
            Some("first")
        );
    }

    /// A stored policy is the Workspace's, so it has to survive everything
    /// else that rewrites the metadata around it.
    #[test]
    fn a_stored_launch_policy_outlives_other_metadata_edits() {
        let store = temp_dir("launch-store");
        let host = temp_dir("launch-host");
        let workspace = create(&store, &host, None).unwrap();
        assert!(workspace.launch.is_empty());

        let launch = LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![crate::protocol::LaunchMount {
                source: PathBuf::from("/srv/corpus"),
                destination: Some(PathBuf::from("/mnt/corpus")),
                writable: false,
            }],
            // A Workspace policy has no layer below it to ignore, so this is
            // dropped rather than stored as a contradiction.
            standalone: true,
        };
        let stored = change_launch(
            &store,
            &workspace.id,
            WorkspaceLaunchChange::Replace(launch.clone()),
        )
        .unwrap();
        assert!(!stored.standalone);
        assert_eq!(stored.templates, launch.templates);

        // Notes, and the access bump the picker relies on, both rewrite
        // `workspace.json`; neither is allowed to drop the policy.
        store_notes(&store, &workspace.id, "shared notes".into()).unwrap();
        access(&store, &workspace.id).unwrap();
        let reread = get(&store, &workspace.id).unwrap();
        assert_eq!(reread.notes, "shared notes");
        assert_eq!(reread.launch.mounts, launch.mounts);
        assert_eq!(reread.launch.network, Some(true));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// Metadata written before Workspaces carried a policy must still read, as
    /// the empty policy: those launches ran on their own inputs alone.
    #[test]
    fn metadata_without_a_launch_policy_reads_as_the_empty_one() {
        let store = temp_dir("legacy-store");
        let host = temp_dir("legacy-host");
        let workspace = create(&store, &host, None).unwrap();
        let meta_path = workspace.path.join(WORKSPACE_META_FILE);
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("launch");
        std::fs::write(&meta_path, serde_json::to_string(&json).unwrap()).unwrap();

        assert!(get(&store, &workspace.id).unwrap().launch.is_empty());

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    #[test]
    fn a_git_repository_association_is_canonical_and_durable() {
        let store = temp_dir("git-store");
        let host = temp_dir("git-host");
        let repository = temp_dir("git-repository");
        git_init(&repository);
        std::fs::create_dir(repository.join("subdir")).unwrap();

        let workspace =
            create_with_repository(&store, &host, None, Some(&repository.join("subdir"))).unwrap();
        let expected = repository.canonicalize().unwrap();
        assert_eq!(
            workspace.git_repository.as_deref(),
            Some(expected.as_path())
        );
        assert_eq!(
            get(&store, &workspace.id).unwrap().git_repository,
            Some(expected.clone())
        );
        let metadata = std::fs::read_to_string(workspace.path.join(WORKSPACE_META_FILE)).unwrap();
        assert!(metadata.contains("git_repository"));

        let cleared = set_git_repository(&store, &workspace.id, None).unwrap();
        assert_eq!(cleared.git_repository, None);
        let restored = set_git_repository(&store, &workspace.id, Some(&repository)).unwrap();
        assert_eq!(restored.git_repository.as_deref(), Some(expected.as_path()));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
        std::fs::remove_dir_all(repository).ok();
    }

    #[test]
    fn malformed_git_metadata_is_rejected_before_it_is_stored() {
        let store = temp_dir("invalid-git-store");
        let host = temp_dir("invalid-git-host");
        let repository = temp_dir("invalid-git-repository");
        std::fs::write(repository.join(".git"), "not a gitdir pointer\n").unwrap();
        let workspace = create(&store, &host, None).unwrap();

        let error = set_git_repository(&store, &workspace.id, Some(&repository)).unwrap_err();
        assert!(error.to_string().contains("not inside a Git repository"));
        assert_eq!(get(&store, &workspace.id).unwrap().git_repository, None);

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
        std::fs::remove_dir_all(repository).ok();
    }

    #[test]
    fn accessing_an_older_workspace_moves_it_to_the_front() {
        let store = temp_dir("recent-store");
        let host = temp_dir("recent-host");
        let first = create(&store, &host, Some("first".into())).unwrap();
        let second = create(&store, &host, Some("second".into())).unwrap();

        access(&store, &first.id).unwrap();

        let listed = list(&store).unwrap();
        assert_eq!(listed[0].id, first.id);
        assert_eq!(listed[1].id, second.id);
    }
}
