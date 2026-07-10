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
    /// Commits that exist in the project repo; `capture` adds `next_id`.
    pub commits: std::cell::RefCell<std::collections::HashSet<String>>,
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
}
