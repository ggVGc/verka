//! Per-attempt execution workspaces: linked git worktrees on candidate
//! branches.
//!
//! Orka owns workspace *policy* — where trees live, how branches are named,
//! when they may be removed. Each attempt gets a fresh worktree anchored to
//! its frozen input commit, so the user's checkout, branch, index, and
//! uncommitted changes are never touched, and concurrent attempts share no
//! writable state. The candidate branch (and the output ref completion
//! retains) outlives worktree cleanup, so recorded output commits stay
//! reachable.

use crate::ports::{CleanupOutcome, PreparedWorkspace, WorkspaceManager};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

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
    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace> {
        let branch = Self::branch_for(attempt);
        let path = self.path_for(attempt);
        // `create_worktree` refuses an existing path, refuses an existing
        // branch at a different commit, and reuses a branch already at the
        // input commit — exactly the recovery semantics we want when a crash
        // left the branch but not the tree.
        let worktree =
            linka::git::create_worktree(&self.project, path, &branch, input_commit)
                .with_context(|| format!("preparing workspace for {attempt}"))?;
        Ok(PreparedWorkspace {
            path: worktree.path,
            branch: worktree.branch,
            input_commit: worktree.input_commit,
        })
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
        if !linka::git::worktree_clean(&workspace.path)? {
            return Ok(CleanupOutcome::RetainedDirty);
        }
        linka::git::remove_worktree(&self.project, &workspace.path)?;
        Ok(CleanupOutcome::Removed)
    }
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
        assert_eq!(
            manager.cleanup(&ws).unwrap(),
            CleanupOutcome::AlreadyAbsent
        );
    }

    #[test]
    fn cleanup_never_discards_uncommitted_changes() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();
        std::fs::write(ws.path.join("scratch.txt"), "unsaved\n").unwrap();

        assert_eq!(
            manager.cleanup(&ws).unwrap(),
            CleanupOutcome::RetainedDirty
        );
        assert_eq!(
            std::fs::read_to_string(ws.path.join("scratch.txt")).unwrap(),
            "unsaved\n"
        );
    }

    #[test]
    fn a_crashed_preparation_can_be_recovered_or_reanchored() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        // Simulate a crash that lost the tree but kept the branch.
        std::fs::remove_dir_all(&ws.path).unwrap();
        assert_eq!(
            manager.cleanup(&ws).unwrap(),
            CleanupOutcome::AlreadyAbsent
        );

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
