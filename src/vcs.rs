//! The version-control seam.
//!
//! Committing outputs/store changes and checking output drift are the only parts
//! of llaundry that need a git repository (versions and pins are blob ids
//! computed locally). Routing them through this trait keeps that dependency
//! injectable: the real implementation ([`crate::git::GitVcs`]) shells out to
//! `git`, while tests use an in-memory [`FakeVcs`] so the rest of the project can
//! be unit-tested with no git binary, no repository, and no configured identity.

use anyhow::Result;

/// The trait's methods split along the workbench's two repositories:
/// [`commit_store`](Vcs::commit_store) persists graph state to the workbench
/// repo; everything else — capturing outputs, drift, file listing, cleanliness
/// — speaks about the project repo.
pub trait Vcs {
    /// Capture (commit) exactly `paths` in the project repository, returning an
    /// opaque output id — for git, the commit hash, which is itself a content
    /// hash of the change.
    fn capture(&self, paths: &[String], message: &str) -> Result<String>;

    /// Commit currently checked out in the project/execution tree.
    fn head_commit(&self) -> Result<Option<String>>;

    /// Name of the currently checked-out project branch, or `None` for a
    /// detached HEAD.
    fn current_branch(&self) -> Result<Option<String>>;

    /// Tree object for `commit`.
    fn tree_id(&self, commit: &str) -> Result<String>;

    /// Keep a completed node output reachable independently of a worktree.
    fn retain_output(&self, node: &str, commit: &str) -> Result<()>;

    /// Blob id of a path in the project/execution tree, if it is a file.
    fn file_blob(&self, path: &str) -> Result<Option<String>>;

    /// Persist a store change (commit the given path, e.g. the store directory)
    /// to the workbench repository.
    fn commit_store(&self, path: &str, message: &str) -> Result<()>;

    /// If the outputs captured under `id` have changed since, return a short,
    /// human-readable reason (for git, a `diff --name-status`); else `None`.
    fn drift(&self, id: &str) -> Result<Option<String>>;

    /// The paths captured under `id` (for git, the files the commit touches).
    fn files_in(&self, id: &str) -> Result<Vec<String>>;

    /// Project paths with uncommitted changes (empty means a clean project
    /// working tree).
    fn dirty_paths(&self) -> Result<Vec<String>>;

    /// The root commit of the project repository's mainline (the first-parent
    /// walk from HEAD), or `None` for a repository with no commits yet. The
    /// one hash that identifies the repository rather than a point in its
    /// history — the store↔project pairing is keyed on it.
    fn root_commit(&self) -> Result<Option<String>>;

    /// Whether `hash` names a commit that exists in the project repository.
    /// Used by the deep pairing check to find recorded output commits that a
    /// history rewrite has orphaned.
    fn commit_exists(&self, hash: &str) -> Result<bool>;

    /// The project repository's remote URL (git remote `origin`), or `None`
    /// if it has no such remote. Recorded on the pairing as information for
    /// human readers; never checked.
    fn remote_url(&self) -> Result<Option<String>>;

    /// Commit currently named by a project ref, or `None` when it is missing.
    fn ref_commit(&self, reference: &str) -> Result<Option<String>>;

    /// Move `target` from exactly `old` to `new`, updating a clean checked-out
    /// target worktree when necessary. Returns false if it cannot fast-forward.
    fn publish_fast_forward(&self, target: &str, old: &str, new: &str) -> Result<bool>;
}

/// In-memory [`Vcs`] for tests. `capture` records the paths and returns `next_id`;
/// `drift` looks the id up in `drift_for`.
#[cfg(test)]
#[derive(Default)]
pub struct FakeVcs {
    pub next_id: String,
    pub dirty: Vec<String>,
    pub drift_for: std::collections::HashMap<String, String>,
    pub captured: std::cell::RefCell<Vec<Vec<String>>>,
    pub store_commits: std::cell::RefCell<usize>,
    pub files_for: std::cell::RefCell<std::collections::HashMap<String, Vec<String>>>,
    /// The project repo's root commit; `None` models an empty repository.
    pub root: Option<String>,
    /// The project repo's `origin` remote URL, if any.
    pub remote: Option<String>,
    /// Commits that exist in the project repo; `capture` adds `next_id`.
    pub commits: std::cell::RefCell<std::collections::HashSet<String>>,
    pub refs: std::cell::RefCell<std::collections::HashMap<String, String>>,
    pub current_branch: Option<String>,
}

#[cfg(test)]
impl Vcs for FakeVcs {
    fn capture(&self, paths: &[String], _message: &str) -> Result<String> {
        self.captured.borrow_mut().push(paths.to_vec());
        self.files_for
            .borrow_mut()
            .insert(self.next_id.clone(), paths.to_vec());
        self.commits.borrow_mut().insert(self.next_id.clone());
        Ok(self.next_id.clone())
    }

    fn head_commit(&self) -> Result<Option<String>> {
        Ok(self.root.clone())
    }

    fn current_branch(&self) -> Result<Option<String>> {
        Ok(self.current_branch.clone())
    }

    fn tree_id(&self, commit: &str) -> Result<String> {
        Ok(format!("tree-{commit}"))
    }

    fn retain_output(&self, _node: &str, commit: &str) -> Result<()> {
        self.commits.borrow_mut().insert(commit.to_string());
        Ok(())
    }

    fn file_blob(&self, _path: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn commit_store(&self, _path: &str, _message: &str) -> Result<()> {
        *self.store_commits.borrow_mut() += 1;
        Ok(())
    }

    fn drift(&self, id: &str) -> Result<Option<String>> {
        Ok(self.drift_for.get(id).cloned())
    }

    fn dirty_paths(&self) -> Result<Vec<String>> {
        Ok(self.dirty.clone())
    }

    fn files_in(&self, id: &str) -> Result<Vec<String>> {
        Ok(self.files_for.borrow().get(id).cloned().unwrap_or_default())
    }

    fn root_commit(&self) -> Result<Option<String>> {
        Ok(self.root.clone())
    }

    fn commit_exists(&self, hash: &str) -> Result<bool> {
        Ok(self.commits.borrow().contains(hash))
    }

    fn remote_url(&self) -> Result<Option<String>> {
        Ok(self.remote.clone())
    }

    fn ref_commit(&self, reference: &str) -> Result<Option<String>> {
        Ok(self.refs.borrow().get(reference).cloned())
    }

    fn publish_fast_forward(&self, _target: &str, old: &str, new: &str) -> Result<bool> {
        if !self.commits.borrow().contains(old) || !self.commits.borrow().contains(new) {
            return Ok(false);
        }
        self.refs.borrow_mut().insert(format!("refs/heads/{_target}"), new.to_string());
        Ok(true)
    }
}
