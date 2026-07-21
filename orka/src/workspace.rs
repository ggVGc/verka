//! Per-attempt execution workspaces: linked git worktrees on candidate
//! branches.
//!
//! Orka owns workspace *policy* — where trees live, how branches are named,
//! when they may be removed — and the git mechanics that implement it. Each
//! attempt gets a fresh worktree anchored to its frozen input commit, so the
//! user's checkout, branch, index, and uncommitted changes are never touched,
//! and concurrent attempts share no writable state. The candidate branch (and
//! the output ref Linka retains on completion) outlives worktree cleanup, so
//! recorded output commits stay reachable.
//!
//! Substituting a different workspace mechanism is genuinely useful (a plain
//! copy, an overlay, a remote checkout), so this stays a narrow Orka-owned
//! trait with the git implementation as one concrete adapter.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// An isolated working tree prepared for one attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedWorkspace {
    pub path: PathBuf,
    /// The candidate branch the workspace is checked out on.
    pub branch: String,
    pub input_commit: String,
}

/// What cleanup observed. A dirty workspace is retained, never discarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupOutcome {
    Removed,
    RetainedDirty,
    AlreadyAbsent,
}

/// Whether an unexecuted workspace could be rolled back without losing work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscardOutcome {
    /// The worktree and its candidate branch were both removed.
    Discarded,
    /// The worktree is dirty or its branch has commits beyond the input.
    RetainedChanged,
}

/// Preparing and cleaning isolated per-attempt working trees.
pub trait WorkspaceManager {
    /// Where `prepare` would put the attempt's workspace — pure, so the plan
    /// can be durably recorded before anything is created.
    fn plan(&self, attempt: &str, input_commit: &str) -> PreparedWorkspace;

    /// Create a fresh working tree at `input_commit` on a candidate branch
    /// named for `attempt`. Fails if the workspace already exists.
    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace>;

    /// Remove a workspace whose attempt is sealed. Refuses to discard
    /// uncommitted changes, reporting `RetainedDirty` instead.
    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome>;

    /// Roll back an attempt that produced no exit evidence. Removes both the
    /// worktree and candidate branch only when they still exactly match the
    /// frozen input; otherwise retains them for inspection.
    fn discard_unchanged(&self, workspace: &PreparedWorkspace) -> Result<DiscardOutcome>;
}

pub struct GitWorkspaces {
    /// The project repository worktrees are linked to.
    project: PathBuf,
    /// Where attempt worktrees are created (e.g. `<workbench>/.orka/worktrees`).
    root: PathBuf,
}

impl GitWorkspaces {
    pub fn new(project: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        Self {
            project: project.into(),
            root: root.into(),
        }
    }

    pub fn branch_for(attempt: &str) -> String {
        format!("orka/attempts/{attempt}")
    }

    pub fn path_for(&self, attempt: &str) -> PathBuf {
        self.root.join(attempt)
    }
}

impl WorkspaceManager for GitWorkspaces {
    fn plan(&self, attempt: &str, input_commit: &str) -> PreparedWorkspace {
        PreparedWorkspace {
            path: self.path_for(attempt),
            branch: Self::branch_for(attempt),
            input_commit: input_commit.to_string(),
        }
    }

    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace> {
        let branch = Self::branch_for(attempt);
        let path = self.path_for(attempt);
        // `create_worktree` refuses an existing path, refuses an existing
        // branch at a different commit, and reuses a branch already at the
        // input commit — exactly the recovery semantics we want when a crash
        // left the branch but not the tree.
        create_worktree(&self.project, path, &branch, input_commit)
            .with_context(|| format!("preparing workspace for {attempt}"))
    }

    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome> {
        if !workspace.path.exists() {
            // The tree is gone (crash, manual removal): drop its stale
            // registration so the path can be reused.
            let _ = Command::new("git")
                .arg("-C")
                .arg(&self.project)
                .args(["worktree", "prune"])
                .status();
            return Ok(CleanupOutcome::AlreadyAbsent);
        }
        if !worktree_clean(&workspace.path)? {
            return Ok(CleanupOutcome::RetainedDirty);
        }
        // Remove only the checked-out worktree. The candidate branch is
        // durable attempt evidence for accepted, failed, and stale attempts;
        // ordinary cleanup never deletes it. Branch removal belongs to a
        // future explicit pruning policy coordinated with attempt retention.
        remove_worktree(&self.project, &workspace.path)?;
        Ok(CleanupOutcome::Removed)
    }

    fn discard_unchanged(&self, workspace: &PreparedWorkspace) -> Result<DiscardOutcome> {
        if workspace.path.exists() {
            if !worktree_clean(&workspace.path)?
                || checked(&workspace.path, &["rev-parse", "HEAD"])? != workspace.input_commit
            {
                return Ok(DiscardOutcome::RetainedChanged);
            }
            remove_worktree(&self.project, &workspace.path)?;
        } else {
            let _ = Command::new("git")
                .arg("-C")
                .arg(&self.project)
                .args(["worktree", "prune"])
                .status();
        }

        let branch_ref = format!("refs/heads/{}", workspace.branch);
        match resolve_ref_optional(&self.project, &branch_ref)? {
            Some(commit) if commit != workspace.input_commit => Ok(DiscardOutcome::RetainedChanged),
            Some(_) => {
                // Supply the expected old value so a concurrent ref move
                // cannot make rollback delete work produced elsewhere.
                checked(
                    &self.project,
                    &["update-ref", "-d", &branch_ref, &workspace.input_commit],
                )?;
                Ok(DiscardOutcome::Discarded)
            }
            None => Ok(DiscardOutcome::Discarded),
        }
    }
}

// --- git worktree mechanics --------------------------------------------------

/// Create a linked worktree at `path` on a candidate `branch` anchored to
/// `rev`. Reuses `branch` when it already sits at the same commit (crash
/// recovery); refuses it when it has moved, so an attempt never silently
/// re-anchors.
fn create_worktree(
    project: &Path,
    path: PathBuf,
    branch: &str,
    rev: &str,
) -> Result<PreparedWorkspace> {
    let input_commit = checked(
        project,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?;
    if path.exists() {
        bail!("execution worktree path already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating worktree directory {}", parent.display()))?;
    }
    let path_arg = path.to_string_lossy().into_owned();
    checked(project, &["check-ref-format", "--branch", branch])?;
    let branch_ref = format!("refs/heads/{branch}");
    if let Ok(existing) = checked(project, &["rev-parse", "--verify", &branch_ref]) {
        if existing != input_commit {
            bail!("candidate branch `{branch}` exists at {existing}, expected {input_commit}");
        }
        checked(project, &["worktree", "add", &path_arg, branch])?;
    } else {
        checked(
            project,
            &["worktree", "add", "-b", branch, &path_arg, &input_commit],
        )?;
    }
    Ok(PreparedWorkspace {
        path,
        branch: branch.to_string(),
        input_commit,
    })
}

/// Whether a worktree has no uncommitted changes.
fn worktree_clean(path: &Path) -> Result<bool> {
    Ok(checked(path, &["status", "--porcelain"])?.is_empty())
}

fn remove_worktree(project: &Path, path: &Path) -> Result<()> {
    let path_arg = path.to_string_lossy().into_owned();
    checked(project, &["worktree", "remove", &path_arg])?;
    Ok(())
}

fn resolve_ref_optional(project: &Path, reference: &str) -> Result<Option<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["rev-parse", "--verify", "--quiet", reference])
        .output()
        .with_context(|| format!("failed to resolve Git ref `{reference}`"))?;
    if out.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ));
    }
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    bail!(
        "resolving Git ref `{reference}` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

/// Run a git command, returning trimmed stdout or an error carrying stderr.
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &PathBuf, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("running git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn project() -> (TempDir, PathBuf, String) {
        let root = std::env::temp_dir().join(format!("orka-ws-git-test-{}", ulid::Ulid::new()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        git(&project, &["init", "-q"]);
        git(&project, &["config", "user.name", "orka test"]);
        git(&project, &["config", "user.email", "test@orka.invalid"]);
        std::fs::write(project.join("file.txt"), "base\n").unwrap();
        git(&project, &["add", "file.txt"]);
        git(&project, &["commit", "-q", "-m", "base"]);
        let head = git(&project, &["rev-parse", "HEAD"]);
        (TempDir(root), project, head)
    }

    fn workspaces(project: &PathBuf) -> GitWorkspaces {
        GitWorkspaces::new(project, project.parent().unwrap().join(".orka/worktrees"))
    }

    #[test]
    fn prepare_anchors_a_fresh_tree_without_touching_the_checkout() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);

        let ws = manager.prepare("attempt-1", &head).unwrap();
        assert_eq!(ws.input_commit, head);
        assert_eq!(ws.branch, "orka/attempts/attempt-1");
        assert_eq!(git(&ws.path, &["rev-parse", "HEAD"]), head);

        // Concurrent attempts get independent trees; the user's checkout and
        // branch never move.
        let other = manager.prepare("attempt-2", &head).unwrap();
        assert_ne!(ws.path, other.path);
        std::fs::write(ws.path.join("file.txt"), "one\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("file.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(git(&project, &["rev-parse", "HEAD"]), head);

        // A second preparation of the same attempt is refused.
        assert!(manager.prepare("attempt-1", &head).is_err());
    }

    #[test]
    fn cleanup_removes_clean_trees_and_keeps_the_candidate_branch() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        // Commit output in the workspace, as a completed attempt would.
        std::fs::write(ws.path.join("out.txt"), "output\n").unwrap();
        git(&ws.path, &["add", "out.txt"]);
        git(&ws.path, &["commit", "-q", "-m", "output"]);
        let output = git(&ws.path, &["rev-parse", "HEAD"]);

        assert_eq!(manager.cleanup(&ws).unwrap(), CleanupOutcome::Removed);
        assert!(!ws.path.exists());
        // The output stays reachable through the candidate branch.
        assert_eq!(git(&project, &["rev-parse", &ws.branch]), output);
        assert_eq!(manager.cleanup(&ws).unwrap(), CleanupOutcome::AlreadyAbsent);
    }

    #[test]
    fn cleanup_never_discards_uncommitted_changes() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();
        std::fs::write(ws.path.join("scratch.txt"), "unsaved\n").unwrap();

        assert_eq!(manager.cleanup(&ws).unwrap(), CleanupOutcome::RetainedDirty);
        assert_eq!(
            std::fs::read_to_string(ws.path.join("scratch.txt")).unwrap(),
            "unsaved\n"
        );
    }

    #[test]
    fn discard_removes_an_unchanged_tree_and_its_branch() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        assert_eq!(
            manager.discard_unchanged(&ws).unwrap(),
            DiscardOutcome::Discarded
        );
        assert!(!ws.path.exists());
        assert!(git(&project, &["branch", "--list", &ws.branch]).is_empty());
    }

    #[test]
    fn discard_retains_dirty_or_committed_work() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let dirty = manager.prepare("attempt-dirty", &head).unwrap();
        std::fs::write(dirty.path.join("scratch.txt"), "unsaved\n").unwrap();
        assert_eq!(
            manager.discard_unchanged(&dirty).unwrap(),
            DiscardOutcome::RetainedChanged
        );
        assert!(dirty.path.exists());

        let committed = manager.prepare("attempt-committed", &head).unwrap();
        std::fs::write(committed.path.join("file.txt"), "changed\n").unwrap();
        git(&committed.path, &["commit", "-q", "-am", "partial work"]);
        assert_eq!(
            manager.discard_unchanged(&committed).unwrap(),
            DiscardOutcome::RetainedChanged
        );
        assert!(committed.path.exists());
    }

    #[test]
    fn a_crashed_preparation_can_be_recovered_or_reanchored() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        // Simulate a crash that lost the tree but kept the branch.
        std::fs::remove_dir_all(&ws.path).unwrap();
        assert_eq!(manager.cleanup(&ws).unwrap(), CleanupOutcome::AlreadyAbsent);

        // Re-preparation reuses the branch because it still sits at the
        // frozen input commit.
        let again = manager.prepare("attempt-1", &head).unwrap();
        assert_eq!(again.input_commit, head);
        assert_eq!(git(&again.path, &["rev-parse", "HEAD"]), head);

        // But a branch that moved away from the frozen input is refused —
        // the attempt's identity must not silently re-anchor.
        std::fs::write(again.path.join("file.txt"), "moved\n").unwrap();
        git(&again.path, &["commit", "-q", "-am", "moved"]);
        manager.cleanup(&again).unwrap();
        let _ = Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(["worktree", "prune"])
            .status();
        assert!(manager.prepare("attempt-1", &head).is_err());
    }
}
