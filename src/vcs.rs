//! The version-control seam.
//!
//! Committing outputs/store changes and checking output drift are the only parts
//! of llaundry that need a git repository (versions and pins are blob ids
//! computed locally). Routing them through this trait keeps that dependency
//! injectable: the real implementation ([`crate::git::GitVcs`]) shells out to
//! `git`, while tests use an in-memory [`FakeVcs`] so the rest of the project can
//! be unit-tested with no git binary, no repository, and no configured identity.

use anyhow::Result;

pub trait Vcs {
    /// Capture (commit) exactly `paths`, returning an opaque output id — for git,
    /// the commit hash, which is itself a content hash of the change.
    fn capture(&self, paths: &[String], message: &str) -> Result<String>;

    /// Persist a store change (commit the given path, e.g. the store directory).
    fn commit_store(&self, path: &str, message: &str) -> Result<()>;

    /// If the outputs captured under `id` have changed since, return a short,
    /// human-readable reason (for git, a `diff --name-status`); else `None`.
    fn drift(&self, id: &str) -> Result<Option<String>>;

    /// The paths captured under `id` (for git, the files the commit touches).
    fn files_in(&self, id: &str) -> Result<Vec<String>>;

    /// Paths with uncommitted changes (empty means a clean working tree).
    fn dirty_paths(&self) -> Result<Vec<String>>;
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
}

#[cfg(test)]
impl Vcs for FakeVcs {
    fn capture(&self, paths: &[String], _message: &str) -> Result<String> {
        self.captured.borrow_mut().push(paths.to_vec());
        self.files_for
            .borrow_mut()
            .insert(self.next_id.clone(), paths.to_vec());
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
}
