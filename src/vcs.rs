//! The version-control seam.
//!
//! `complete` and staleness checks are the only parts of llaundry that touch git.
//! Routing them through this trait keeps that dependency injectable: the real
//! implementation ([`crate::git::GitVcs`]) shells out to `git`, while tests use an
//! in-memory [`FakeVcs`] so the rest of the project can be unit-tested with no git
//! binary, no repository, and no configured identity.

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
}

/// In-memory [`Vcs`] for tests. `capture` records the paths and returns `next_id`;
/// `drift` looks the id up in `drift_for`.
#[cfg(test)]
#[derive(Default)]
pub struct FakeVcs {
    pub next_id: String,
    pub drift_for: std::collections::HashMap<String, String>,
    pub captured: std::cell::RefCell<Vec<Vec<String>>>,
    pub store_commits: std::cell::RefCell<usize>,
}

#[cfg(test)]
impl Vcs for FakeVcs {
    fn capture(&self, paths: &[String], _message: &str) -> Result<String> {
        self.captured.borrow_mut().push(paths.to_vec());
        Ok(self.next_id.clone())
    }

    fn commit_store(&self, _path: &str, _message: &str) -> Result<()> {
        *self.store_commits.borrow_mut() += 1;
        Ok(())
    }

    fn drift(&self, id: &str) -> Result<Option<String>> {
        Ok(self.drift_for.get(id).cloned())
    }
}
