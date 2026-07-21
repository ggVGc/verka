//! Managed worktrees for Nota reviews.
//!
//! Nota deliberately owns only the Git-native review record. Orka owns the
//! convenience and safety policy around where a reviewer checks that branch
//! out, how an existing checkout is validated, and when it may be removed.

use crate::review::ReviewRecord;
use anyhow::{bail, Context, Result};
use linka::NodeId;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewWorktree {
    pub verification: NodeId,
    pub path: PathBuf,
    pub branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewWorktreeInfo {
    pub verification: NodeId,
    pub path: PathBuf,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewCleanupOutcome {
    Removed,
    RetainedDirty,
    AlreadyAbsent,
}

/// Git-backed review worktrees below `.orka/review-worktrees/`.
pub struct GitReviewWorktrees {
    project: PathBuf,
    root: PathBuf,
}

impl GitReviewWorktrees {
    pub fn new(project: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        Self {
            project: absolute(project.into()),
            root: absolute(root.into()),
        }
    }

    pub fn path_for(&self, verification: &NodeId) -> PathBuf {
        self.root.join(verification.as_str())
    }

    /// Create the canonical checkout for a review, or safely reuse it when it
    /// is already registered on the recorded review branch.
    pub fn prepare(&self, record: &ReviewRecord) -> Result<ReviewWorktree> {
        let worktree = ReviewWorktree {
            verification: record.verification.clone(),
            path: self.path_for(&record.verification),
            branch: record.branch.clone(),
        };

        if worktree.path.exists() {
            self.require_registered_branch(&worktree)?;
            return Ok(worktree);
        }

        // Clear a stale registration left by an externally removed directory.
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.project)
            .args(["worktree", "prune"])
            .status();
        if let Some(parent) = worktree.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating review worktree directory {}", parent.display())
            })?;
        }
        let path = worktree.path.to_string_lossy().into_owned();
        checked(&self.project, &["worktree", "add", &path, &worktree.branch]).with_context(
            || {
                format!(
                    "preparing review worktree for {} on `{}`",
                    worktree.verification, worktree.branch
                )
            },
        )?;
        Ok(worktree)
    }

    /// Inspect every registered worktree in Orka's managed review directory.
    pub fn list(&self) -> Result<Vec<ReviewWorktreeInfo>> {
        let mut managed = Vec::new();
        for registration in registrations(&self.project)? {
            if !registration.path.starts_with(&self.root) {
                continue;
            }
            let Some(name) = registration.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Ok(verification) = name.parse::<NodeId>() else {
                continue;
            };
            let Some(branch) = registration.branch else {
                continue;
            };
            let dirty = !checked(&registration.path, &["status", "--porcelain"])?.is_empty();
            managed.push(ReviewWorktreeInfo {
                verification,
                path: registration.path,
                branch,
                dirty,
            });
        }
        managed.sort_by(|a, b| a.verification.as_str().cmp(b.verification.as_str()));
        Ok(managed)
    }

    /// Remove only a clean, correctly registered review worktree. The Nota
    /// branch remains as the durable review record.
    pub fn cleanup(&self, record: &ReviewRecord) -> Result<ReviewCleanupOutcome> {
        let worktree = ReviewWorktree {
            verification: record.verification.clone(),
            path: self.path_for(&record.verification),
            branch: record.branch.clone(),
        };
        if !worktree.path.exists() {
            let _ = Command::new("git")
                .arg("-C")
                .arg(&self.project)
                .args(["worktree", "prune"])
                .status();
            return Ok(ReviewCleanupOutcome::AlreadyAbsent);
        }
        self.require_registered_branch(&worktree)?;
        if !checked(&worktree.path, &["status", "--porcelain"])?.is_empty() {
            return Ok(ReviewCleanupOutcome::RetainedDirty);
        }
        let path = worktree.path.to_string_lossy().into_owned();
        checked(&self.project, &["worktree", "remove", &path])?;
        Ok(ReviewCleanupOutcome::Removed)
    }

    fn require_registered_branch(&self, worktree: &ReviewWorktree) -> Result<()> {
        let registration = registrations(&self.project)?
            .into_iter()
            .find(|registration| registration.path == worktree.path)
            .with_context(|| {
                format!(
                    "managed review path {} exists but is not a worktree of {}",
                    worktree.path.display(),
                    self.project.display()
                )
            })?;
        match registration.branch.as_deref() {
            Some(branch) if branch == worktree.branch => Ok(()),
            Some(branch) => bail!(
                "managed review worktree {} is on `{branch}`, expected `{}`",
                worktree.path.display(),
                worktree.branch
            ),
            None => bail!(
                "managed review worktree {} is detached, expected `{}`",
                worktree.path.display(),
                worktree.branch
            ),
        }
    }
}

#[derive(Debug)]
struct Registration {
    path: PathBuf,
    branch: Option<String>,
}

fn registrations(project: &Path) -> Result<Vec<Registration>> {
    let text = checked(project, &["worktree", "list", "--porcelain"])?;
    let mut registrations = Vec::new();
    let mut path = None;
    let mut branch = None;
    for line in text.lines().chain(std::iter::once("")) {
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
            branch = None;
        } else if let Some(value) = line.strip_prefix("branch refs/heads/") {
            branch = Some(value.to_string());
        } else if line.is_empty() {
            if let Some(path) = path.take() {
                registrations.push(Registration {
                    path: absolute(path),
                    branch: branch.take(),
                });
            }
        }
    }
    Ok(registrations)
}

fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|current| current.join(&path))
            .unwrap_or(path)
    }
}

fn checked(base: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
