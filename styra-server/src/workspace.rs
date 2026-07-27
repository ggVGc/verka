//! Durable Workspace metadata and directory layout.
//!
//! A Workspace is Styra's top-level unit of related work. It names one host
//! directory and owns any number of durable provider Sessions beneath
//! `workspaces/<workspace-id>/sessions/`.

use crate::types::WorkspaceSummary;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const WORKSPACE_META_FILE: &str = "workspace.json";
pub const LEGACY_WORKSPACE_ID: &str = "__legacy__";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceMeta {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    host_path: PathBuf,
    created_at_ms: u64,
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
    let host_path = host_path
        .canonicalize()
        .with_context(|| format!("workspace directory {} must exist", host_path.display()))?;
    let created_at_ms = now_ms();
    let id = new_id(created_at_ms);
    let path = workspace_dir(store_root, &id);
    std::fs::create_dir_all(path.join("sessions"))
        .with_context(|| format!("creating Workspace directory {}", path.display()))?;
    let meta = WorkspaceMeta {
        id: id.clone(),
        name,
        host_path: host_path.clone(),
        created_at_ms,
    };
    write_meta(&path, &meta)?;
    Ok(WorkspaceSummary {
        id,
        name: meta.name,
        host_path,
        path,
        session_count: 0,
        age: "just now".into(),
        created_at_ms,
    })
}

/// List durable Workspaces newest first.
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
    workspaces.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    if let Some(legacy) = legacy_summary(store_root)? {
        workspaces.push(legacy);
    }
    Ok(workspaces)
}

pub fn get(store_root: &Path, id: &str) -> Result<WorkspaceSummary> {
    if id == LEGACY_WORKSPACE_ID {
        return legacy_summary(store_root)?
            .with_context(|| "Legacy sessions Workspace was not found");
    }
    let path = workspace_dir(store_root, id);
    if !path.is_dir() {
        anyhow::bail!("Workspace {id:?} was not found");
    }
    summary_from_dir(&path, now_ms())
}

fn legacy_summary(store_root: &Path) -> Result<Option<WorkspaceSummary>> {
    let path = crate::journal::sessions_dir(store_root);
    if !path.is_dir() {
        return Ok(None);
    }
    let session_count = std::fs::read_dir(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .count();
    if session_count == 0 {
        return Ok(None);
    }
    Ok(Some(WorkspaceSummary {
        id: LEGACY_WORKSPACE_ID.into(),
        name: Some("Legacy sessions".into()),
        // Old Session metadata did not persist the mounted host directory.
        host_path: PathBuf::new(),
        path,
        session_count,
        age: "legacy".into(),
        created_at_ms: 0,
    }))
}

fn summary_from_dir(path: &Path, now: u64) -> Result<WorkspaceSummary> {
    let meta = read_meta(path)?;
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
    Ok(WorkspaceSummary {
        id: meta.id,
        name: meta.name,
        host_path: meta.host_path,
        path: path.to_path_buf(),
        session_count,
        age: humanize_age(now, meta.created_at_ms),
        created_at_ms: meta.created_at_ms,
    })
}

fn write_meta(directory: &Path, meta: &WorkspaceMeta) -> Result<()> {
    let path = directory.join(WORKSPACE_META_FILE);
    let json = serde_json::to_string_pretty(meta).context("serializing Workspace metadata")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
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

    #[test]
    fn flat_sessions_appear_as_one_synthetic_legacy_workspace() {
        let store = temp_dir("legacy-store");
        std::fs::create_dir_all(crate::journal::sessions_dir(&store).join("old-session")).unwrap();
        let workspaces = list(&store).unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, LEGACY_WORKSPACE_ID);
        assert_eq!(workspaces[0].session_count, 1);
        assert_eq!(workspaces[0].name.as_deref(), Some("Legacy sessions"));
    }
}
