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
    /// Require the Linka store path to have no uncommitted changes. The error
    /// identifies the dirty store content so callers can report what must be
    /// resolved.
    fn require_clean_store(&self, path: &str) -> Result<()>;

    fn commit_store(&self, path: &str, message: &str) -> Result<()>;

    /// Whether store history ever recorded `commit` as `node`'s output.
    fn output_was_recorded(&self, path: &str, node: &str, commit: &str) -> Result<bool>;
}

pub trait ArtifactStore {
    /// Capture (commit) exactly `paths` in the project repository, returning an
    /// opaque output id — for git, the commit hash, which is itself a content
    /// hash of the change.
    fn capture(&self, paths: &[String], message: &str) -> Result<String>;

    /// Capture the entire final state of an isolated execution worktree as a
    /// single output relative to `parent` (the frozen input commit the work
    /// started from), returning the output id. The complete change is whatever
    /// differs between `parent` and the worktree now — regardless of any
    /// commits the agent made along the way — so no output paths need be
    /// declared. The worktree's checked-out branch is left pointing at the
    /// captured commit and its tree clean, exactly as [`Self::capture`] leaves
    /// it. Returns `None` when the worktree is identical to `parent` (nothing
    /// was produced).
    fn capture_worktree(&self, parent: &str, message: &str) -> Result<Option<String>>;

    /// Keep a completed node output reachable independently of a worktree.
    fn retain_output(&self, node: &str, commit: &str) -> Result<()>;

    /// If the outputs captured under `id` have changed since, return a short,
    /// human-readable reason (for git, a `diff --name-status`); else `None`.
    fn drift(&self, id: &str) -> Result<Option<String>>;

    /// As `drift`, but compare the artifact with a named revision rather than
    /// the currently checked-out project tree.
    fn drift_at(&self, id: &str, revision: &str) -> Result<Option<String>>;

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
    /// The node named by a `Linka-Node` trailer on `commit`, if present.
    fn linka_node(&self, commit: &str) -> Result<Option<String>>;
    fn tree_id(&self, commit: &str) -> Result<String>;
    fn file_blob(&self, path: &str) -> Result<Option<String>>;
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>>;
}

pub trait RepositoryIdentity {
    fn root_commit(&self) -> Result<Option<String>>;
    fn remote_url(&self) -> Result<Option<String>>;
}

/// Named project-reference operations used by Linka candidates.
pub trait BranchStore {
    fn current_branch(&self) -> Result<Option<String>>;
    fn ref_commit(&self, reference: &str) -> Result<Option<String>>;
    /// Whether `ancestor` is contained in `descendant`'s history.
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool>;
    /// Move `target` from exactly `expected_previous` to `candidate`, only by
    /// fast-forward. Returns false for a race or non-fast-forward.
    fn publish_fast_forward(
        &self,
        target: &str,
        expected_previous: &str,
        candidate: &str,
    ) -> Result<bool>;
}

pub trait Vcs:
    StoreHistory + ArtifactStore + ContextIdentity + RepositoryIdentity + BranchStore
{
}
impl<
        T: StoreHistory + ArtifactStore + ContextIdentity + RepositoryIdentity + BranchStore + ?Sized,
    > Vcs for T
{
}

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
    pub dirty_store: std::cell::RefCell<Vec<String>>,
    pub linka_nodes: std::collections::HashMap<String, String>,
    pub recorded_outputs: std::collections::HashSet<(String, String)>,
    pub files_for: std::cell::RefCell<std::collections::HashMap<String, Vec<String>>>,
    /// The project repo's root commit; `None` models an empty repository.
    pub root: Option<String>,
    /// The project repo's `origin` remote URL, if any.
    pub remote: Option<String>,
    /// Commits that exist in the project repo; `capture` adds `next_id`.
    pub commits: std::cell::RefCell<std::collections::HashSet<String>>,
    pub refs: std::cell::RefCell<std::collections::HashMap<String, String>>,
    pub branch: Option<String>,
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

    fn capture_worktree(&self, _parent: &str, _message: &str) -> Result<Option<String>> {
        // The worktree's produced files are modeled by `dirty`; an empty set
        // means nothing was produced, mirroring a tree equal to the parent.
        if self.dirty.is_empty() {
            return Ok(None);
        }
        self.captured.borrow_mut().push(self.dirty.clone());
        self.files_for
            .borrow_mut()
            .insert(self.next_id.clone(), self.dirty.clone());
        self.commits.borrow_mut().insert(self.next_id.clone());
        Ok(Some(self.next_id.clone()))
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

    fn drift_at(&self, id: &str, revision: &str) -> Result<Option<String>> {
        if id == revision {
            Ok(None)
        } else {
            self.drift(id)
        }
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
    fn require_clean_store(&self, _path: &str) -> Result<()> {
        let dirty = self.dirty_store.borrow();
        if !dirty.is_empty() {
            anyhow::bail!("uncommitted store changes:\n  {}", dirty.join("\n  "));
        }
        Ok(())
    }

    fn commit_store(&self, _path: &str, _message: &str) -> Result<()> {
        *self.store_commits.borrow_mut() += 1;
        self.dirty_store.borrow_mut().clear();
        Ok(())
    }

    fn output_was_recorded(&self, _path: &str, node: &str, commit: &str) -> Result<bool> {
        Ok(self
            .recorded_outputs
            .contains(&(node.to_string(), commit.to_string())))
    }
}

#[cfg(test)]
impl ContextIdentity for FakeVcs {
    fn head_commit(&self) -> Result<Option<String>> {
        Ok(self.root.clone())
    }
    fn linka_node(&self, commit: &str) -> Result<Option<String>> {
        Ok(self.linka_nodes.get(commit).cloned())
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
impl BranchStore for FakeVcs {
    fn current_branch(&self) -> Result<Option<String>> {
        Ok(self.branch.clone())
    }

    fn ref_commit(&self, reference: &str) -> Result<Option<String>> {
        Ok(self.refs.borrow().get(reference).cloned())
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        Ok(ancestor == descendant)
    }

    fn publish_fast_forward(
        &self,
        target: &str,
        expected_previous: &str,
        candidate: &str,
    ) -> Result<bool> {
        let mut refs = self.refs.borrow_mut();
        if refs.get(target).map(String::as_str) != Some(expected_previous) {
            return Ok(false);
        }
        refs.insert(target.to_string(), candidate.to_string());
        Ok(true)
    }
}
