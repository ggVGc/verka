//! The version-control seam.
//!
//! Committing outputs/store changes and checking output drift are the only parts
//! of linka that need a git repository (versions and pins are blob ids
//! computed locally). Routing them through this trait keeps that dependency
//! injectable: the real implementation ([`crate::git::GitVcs`]) shells out to
//! `git`, while tests use an in-memory `FakeVcs` so the rest of the project can
//! be unit-tested with no git binary, no repository, and no configured identity.

use anyhow::Result;
pub trait StoreHistory {
    fn commit_store(&self, path: &str, message: &str) -> Result<()>;
}

pub trait ArtifactStore {
    /// Capture (commit) exactly `paths` in the project repository, returning an
    /// opaque output id — for git, the commit hash, which is itself a content
    /// hash of the change.
    fn capture(&self, paths: &[String], message: &str) -> Result<String>;

    /// Keep a completed node output reachable independently of a worktree.
    fn retain_output(&self, node: &str, commit: &str) -> Result<()>;

    /// If the outputs captured under `id` have changed since, return a short,
    /// human-readable reason (for git, a `diff --name-status`); else `None`.
    fn drift(&self, id: &str) -> Result<Option<String>>;

    /// The paths captured under `id` (for git, the files the commit touches).
    fn files_in(&self, id: &str) -> Result<Vec<String>>;

    /// Project paths with uncommitted changes (empty means a clean project
    /// working tree).
    fn dirty_paths(&self) -> Result<Vec<String>>;

    /// Whether `hash` names a commit that exists in the project repository.
    /// Used by the deep pairing check to find recorded output commits that a
    /// history rewrite has orphaned.
    fn commit_exists(&self, hash: &str) -> Result<bool>;
}

pub trait ContextIdentity {
    fn head_commit(&self) -> Result<Option<String>>;
    fn tree_id(&self, commit: &str) -> Result<String>;
    fn file_blob(&self, path: &str) -> Result<Option<String>>;
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>>;
}

pub trait RepositoryIdentity {
    fn root_commit(&self) -> Result<Option<String>>;
    fn remote_url(&self) -> Result<Option<String>>;
}

pub trait Vcs: StoreHistory + ArtifactStore + ContextIdentity + RepositoryIdentity {}
impl<T: StoreHistory + ArtifactStore + ContextIdentity + RepositoryIdentity + ?Sized> Vcs for T {}

/// In-memory [`Vcs`] for tests. `capture` records the paths and returns `next_id`;
/// `drift` looks the id up in `drift_for`.
#[cfg(test)]
#[derive(Default)]
pub struct FakeVcs {
    pub next_id: String,
    pub dirty: Vec<String>,
    pub drift_for: std::collections::HashMap<String, String>,
    pub drift_error: Option<String>,
    pub revision_blobs: std::collections::HashMap<(String, String), String>,
    pub captured: std::cell::RefCell<Vec<Vec<String>>>,
    pub store_commits: std::cell::RefCell<usize>,
    pub files_for: std::cell::RefCell<std::collections::HashMap<String, Vec<String>>>,
    /// The project repo's root commit; `None` models an empty repository.
    pub root: Option<String>,
    /// The project repo's `origin` remote URL, if any.
    pub remote: Option<String>,
    /// Commits that exist in the project repo; `capture` adds `next_id`.
    pub commits: std::cell::RefCell<std::collections::HashSet<String>>,
}

#[cfg(test)]
impl ArtifactStore for FakeVcs {
    fn capture(&self, paths: &[String], _message: &str) -> Result<String> {
        self.captured.borrow_mut().push(paths.to_vec());
        self.files_for
            .borrow_mut()
            .insert(self.next_id.clone(), paths.to_vec());
        self.commits.borrow_mut().insert(self.next_id.clone());
        Ok(self.next_id.clone())
    }

    fn retain_output(&self, _node: &str, commit: &str) -> Result<()> {
        self.commits.borrow_mut().insert(commit.to_string());
        Ok(())
    }

    fn drift(&self, id: &str) -> Result<Option<String>> {
        if let Some(error) = &self.drift_error {
            anyhow::bail!("{error}");
        }
        Ok(self.drift_for.get(id).cloned())
    }

    fn dirty_paths(&self) -> Result<Vec<String>> {
        Ok(self.dirty.clone())
    }

    fn files_in(&self, id: &str) -> Result<Vec<String>> {
        Ok(self.files_for.borrow().get(id).cloned().unwrap_or_default())
    }

    fn commit_exists(&self, hash: &str) -> Result<bool> {
        Ok(self.commits.borrow().contains(hash))
    }
}

#[cfg(test)]
impl StoreHistory for FakeVcs {
    fn commit_store(&self, _path: &str, _message: &str) -> Result<()> {
        *self.store_commits.borrow_mut() += 1;
        Ok(())
    }
}

#[cfg(test)]
impl ContextIdentity for FakeVcs {
    fn head_commit(&self) -> Result<Option<String>> {
        Ok(self.root.clone())
    }
    fn tree_id(&self, commit: &str) -> Result<String> {
        Ok(format!("tree-{commit}"))
    }
    fn file_blob(&self, _path: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>> {
        Ok(self
            .revision_blobs
            .get(&(revision.into(), path.into()))
            .cloned())
    }
}

#[cfg(test)]
impl RepositoryIdentity for FakeVcs {
    fn root_commit(&self) -> Result<Option<String>> {
        Ok(self.root.clone())
    }
    fn remote_url(&self) -> Result<Option<String>> {
        Ok(self.remote.clone())
    }
}

#[cfg(test)]
mod architecture_tests {
    #[test]
    fn graph_capability_surface_has_no_project_lifecycle_concepts() {
        let source = include_str!("vcs.rs");
        let public_surface = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "current_branch",
            "resolve_revision",
            "create_worktree",
            "publish_fast_forward",
            "remove_worktree",
            "ref_commit",
        ] {
            assert!(
                !public_surface.contains(forbidden),
                "public VCS surface contains {forbidden}"
            );
        }
        let manifest = include_str!("../Cargo.toml");
        assert!(!manifest.contains("orka"));
    }
}
