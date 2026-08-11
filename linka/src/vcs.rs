//! The version-control seam.
//!
//! Committing outputs and store changes, checking drift, and reading refs are
//! the only parts of Linka that need a git repository — versions and pins are
//! blob ids computed locally. Routing them through one trait keeps that
//! dependency injectable: [`crate::git::GitVcs`] shells out to `git`, while
//! tests use an in-memory fake with no git binary, repository, or identity.

use anyhow::Result;
use std::cell::RefCell;
use std::collections::HashMap;

pub trait Vcs {
    // --- store history ---------------------------------------------------------

    /// Require the Linka store path to have no uncommitted changes. The error
    /// identifies the dirty content so callers can report what to resolve.
    fn require_clean_store(&self, path: &str) -> Result<()>;
    fn commit_store(&self, path: &str, message: &str) -> Result<()>;
    /// Whether store history ever recorded `commit` as `node`'s output.
    fn output_was_recorded(&self, path: &str, node: &str, commit: &str) -> Result<bool>;

    // --- artifacts -------------------------------------------------------------

    /// Capture (commit) exactly `paths` in the project repository, returning an
    /// opaque output id — for git, the commit hash.
    fn capture(&self, paths: &[String], message: &str) -> Result<String>;
    /// Capture the entire final state of an isolated execution worktree as one
    /// output relative to `parent`, the frozen input commit the work started
    /// from. `None` when the worktree is identical to `parent`.
    fn capture_worktree(&self, parent: &str, message: &str) -> Result<Option<String>>;
    /// Keep a completed node output reachable independently of a worktree.
    fn retain_output(&self, node: &str, commit: &str) -> Result<()>;
    /// A short, human-readable reason the content captured under `id` has
    /// changed since, or `None`. `against` names the revision to compare with;
    /// `None` compares with the currently checked-out project tree.
    fn drift(&self, id: &str, against: Option<&str>) -> Result<Option<String>>;
    /// The paths captured under `id`.
    fn files_in(&self, id: &str) -> Result<Vec<String>>;
    /// Project paths with uncommitted changes.
    fn dirty_paths(&self) -> Result<Vec<String>>;
    fn commit_exists(&self, hash: &str) -> Result<bool>;

    // --- identity and refs -------------------------------------------------------

    fn head_commit(&self) -> Result<Option<String>>;
    /// The node named by a `Linka-Node` trailer on `commit`, if present.
    fn linka_node(&self, commit: &str) -> Result<Option<String>>;
    fn tree_id(&self, commit: &str) -> Result<String>;
    fn file_blob(&self, path: &str) -> Result<Option<String>>;
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>>;
    fn root_commit(&self) -> Result<Option<String>>;
    fn remote_url(&self) -> Result<Option<String>>;
    fn current_branch(&self) -> Result<Option<String>>;
    fn ref_commit(&self, reference: &str) -> Result<Option<String>>;
    /// Whether `ancestor` is contained in `descendant`'s history.
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool>;
    /// Move `target` from exactly `expected` to `new`, only by fast-forward.
    /// Returns false for a race or a non-fast-forward.
    fn publish_fast_forward(&self, target: &str, expected: &str, new: &str) -> Result<bool>;
}

/// A [`Vcs`] with no repository behind it: every read answers "nothing known"
/// and every write refuses.
///
/// This is what makes `check` genuinely git-free. A read-only integrity check
/// must be able to evaluate the graph without a repository, and "no repository
/// says otherwise" is the honest answer to drift and ancestry there — not a
/// failure, and not a claim that content has changed.
pub struct OfflineVcs;

impl Vcs for OfflineVcs {
    fn require_clean_store(&self, _path: &str) -> Result<()> {
        Ok(())
    }
    fn commit_store(&self, _path: &str, _message: &str) -> Result<()> {
        anyhow::bail!("no repository is available to commit the store")
    }
    fn output_was_recorded(&self, _path: &str, _node: &str, _commit: &str) -> Result<bool> {
        Ok(false)
    }
    fn capture(&self, _paths: &[String], _message: &str) -> Result<String> {
        anyhow::bail!("no repository is available to capture output")
    }
    fn capture_worktree(&self, _parent: &str, _message: &str) -> Result<Option<String>> {
        anyhow::bail!("no repository is available to capture output")
    }
    fn retain_output(&self, _node: &str, _commit: &str) -> Result<()> {
        anyhow::bail!("no repository is available to retain output")
    }
    fn drift(&self, _id: &str, _against: Option<&str>) -> Result<Option<String>> {
        Ok(None)
    }
    fn files_in(&self, _id: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn dirty_paths(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn commit_exists(&self, _hash: &str) -> Result<bool> {
        Ok(false)
    }
    fn head_commit(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn linka_node(&self, _commit: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn tree_id(&self, commit: &str) -> Result<String> {
        anyhow::bail!("no repository is available to read the tree of {commit}")
    }
    fn file_blob(&self, _path: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn file_blob_at(&self, _revision: &str, _path: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn root_commit(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn remote_url(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn current_branch(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn ref_commit(&self, _reference: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn is_ancestor(&self, _ancestor: &str, _descendant: &str) -> Result<bool> {
        Ok(false)
    }
    fn publish_fast_forward(&self, _target: &str, _expected: &str, _new: &str) -> Result<bool> {
        anyhow::bail!("no repository is available to publish into")
    }
}

/// One drift question: the captured output, and what to compare it against.
type DriftQuestion = (String, Option<String>);

/// A [`Vcs`] that answers each distinct read once.
///
/// Within one evaluation pass `ref_commit`, `is_ancestor`, `tree_id` and
/// `drift` are pure, so a graph with a hundred nodes sharing one target branch
/// costs one subprocess rather than a hundred. Mutating calls pass straight
/// through and are not cached — a pass that writes is not a pass that reads.
pub struct MemoizingVcs<'a> {
    inner: &'a dyn Vcs,
    ref_commit: RefCell<HashMap<String, Option<String>>>,
    is_ancestor: RefCell<HashMap<(String, String), bool>>,
    tree_id: RefCell<HashMap<String, String>>,
    drift: RefCell<HashMap<DriftQuestion, Option<String>>>,
}

impl<'a> MemoizingVcs<'a> {
    pub fn new(inner: &'a dyn Vcs) -> Self {
        Self {
            inner,
            ref_commit: RefCell::default(),
            is_ancestor: RefCell::default(),
            tree_id: RefCell::default(),
            drift: RefCell::default(),
        }
    }
}

/// Look `key` up in `cache`, computing and storing it on a miss. The closure
/// runs outside the borrow, so a nested lookup cannot panic on a double borrow.
fn memoized<K, V>(
    cache: &RefCell<HashMap<K, V>>,
    key: K,
    compute: impl FnOnce() -> Result<V>,
) -> Result<V>
where
    K: std::hash::Hash + Eq + Clone,
    V: Clone,
{
    if let Some(value) = cache.borrow().get(&key) {
        return Ok(value.clone());
    }
    let value = compute()?;
    cache.borrow_mut().insert(key, value.clone());
    Ok(value)
}

impl Vcs for MemoizingVcs<'_> {
    fn require_clean_store(&self, path: &str) -> Result<()> {
        self.inner.require_clean_store(path)
    }
    fn commit_store(&self, path: &str, message: &str) -> Result<()> {
        self.inner.commit_store(path, message)
    }
    fn output_was_recorded(&self, path: &str, node: &str, commit: &str) -> Result<bool> {
        self.inner.output_was_recorded(path, node, commit)
    }
    fn capture(&self, paths: &[String], message: &str) -> Result<String> {
        self.inner.capture(paths, message)
    }
    fn capture_worktree(&self, parent: &str, message: &str) -> Result<Option<String>> {
        self.inner.capture_worktree(parent, message)
    }
    fn retain_output(&self, node: &str, commit: &str) -> Result<()> {
        self.inner.retain_output(node, commit)
    }
    fn drift(&self, id: &str, against: Option<&str>) -> Result<Option<String>> {
        memoized(
            &self.drift,
            (id.to_string(), against.map(str::to_string)),
            || self.inner.drift(id, against),
        )
    }
    fn files_in(&self, id: &str) -> Result<Vec<String>> {
        self.inner.files_in(id)
    }
    fn dirty_paths(&self) -> Result<Vec<String>> {
        self.inner.dirty_paths()
    }
    fn commit_exists(&self, hash: &str) -> Result<bool> {
        self.inner.commit_exists(hash)
    }
    fn head_commit(&self) -> Result<Option<String>> {
        self.inner.head_commit()
    }
    fn linka_node(&self, commit: &str) -> Result<Option<String>> {
        self.inner.linka_node(commit)
    }
    fn tree_id(&self, commit: &str) -> Result<String> {
        memoized(&self.tree_id, commit.to_string(), || {
            self.inner.tree_id(commit)
        })
    }
    fn file_blob(&self, path: &str) -> Result<Option<String>> {
        self.inner.file_blob(path)
    }
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>> {
        self.inner.file_blob_at(revision, path)
    }
    fn root_commit(&self) -> Result<Option<String>> {
        self.inner.root_commit()
    }
    fn remote_url(&self) -> Result<Option<String>> {
        self.inner.remote_url()
    }
    fn current_branch(&self) -> Result<Option<String>> {
        self.inner.current_branch()
    }
    fn ref_commit(&self, reference: &str) -> Result<Option<String>> {
        memoized(&self.ref_commit, reference.to_string(), || {
            self.inner.ref_commit(reference)
        })
    }
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        memoized(
            &self.is_ancestor,
            (ancestor.to_string(), descendant.to_string()),
            || self.inner.is_ancestor(ancestor, descendant),
        )
    }
    fn publish_fast_forward(&self, target: &str, expected: &str, new: &str) -> Result<bool> {
        self.inner.publish_fast_forward(target, expected, new)
    }
}

#[cfg(test)]
pub use fake::FakeVcs;

/// An in-memory [`Vcs`] that models commit parentage, so `is_ancestor`,
/// `publish_fast_forward`, and drift are genuinely exercised: publication is
/// *defined* by ancestry, and a fake answering `ancestor == descendant` makes
/// the ordinary post-merge case untestable.
#[cfg(test)]
pub mod fake {
    use super::*;
    use anyhow::bail;
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    pub struct FakeVcs {
        /// The id `capture` mints next.
        pub next_id: String,
        /// Project paths reported as uncommitted.
        pub dirty: Vec<String>,
        pub drift_for: HashMap<String, String>,
        pub drift_error: Option<String>,
        pub revision_blobs: HashMap<(String, String), String>,
        pub captured: RefCell<Vec<Vec<String>>>,
        pub store_commits: RefCell<usize>,
        pub dirty_store: RefCell<Vec<String>>,
        pub linka_nodes: HashMap<String, String>,
        pub recorded_outputs: HashSet<(String, String)>,
        pub files_for: RefCell<HashMap<String, Vec<String>>>,
        /// commit -> its parent, the whole history model.
        pub parents: RefCell<HashMap<String, Option<String>>>,
        pub refs: RefCell<HashMap<String, String>>,
        pub head: Option<String>,
        pub branch: Option<String>,
        pub remote: Option<String>,
        /// How many times `ref_commit` was actually asked, so a memoizing
        /// caller can be shown to collapse repeated questions.
        pub ref_reads: RefCell<usize>,
    }

    impl FakeVcs {
        /// Record `commit` as a child of `parent` (`None` for a root commit).
        pub fn commit(&self, commit: &str, parent: Option<&str>) {
            self.parents
                .borrow_mut()
                .insert(commit.to_string(), parent.map(str::to_string));
        }

        pub fn set_ref(&self, reference: &str, commit: &str) {
            self.refs
                .borrow_mut()
                .insert(reference.to_string(), commit.to_string());
        }
    }

    impl Vcs for FakeVcs {
        fn require_clean_store(&self, _path: &str) -> Result<()> {
            let dirty = self.dirty_store.borrow();
            if !dirty.is_empty() {
                bail!("uncommitted store changes:\n  {}", dirty.join("\n  "));
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

        fn capture(&self, paths: &[String], _message: &str) -> Result<String> {
            self.captured.borrow_mut().push(paths.to_vec());
            self.files_for
                .borrow_mut()
                .insert(self.next_id.clone(), paths.to_vec());
            self.commit(&self.next_id, self.head.as_deref());
            Ok(self.next_id.clone())
        }

        fn capture_worktree(&self, parent: &str, _message: &str) -> Result<Option<String>> {
            // Produced files are modeled by `dirty`; nothing dirty means a tree
            // equal to the parent, so nothing was produced.
            if self.dirty.is_empty() {
                return Ok(None);
            }
            self.captured.borrow_mut().push(self.dirty.clone());
            self.files_for
                .borrow_mut()
                .insert(self.next_id.clone(), self.dirty.clone());
            self.commit(&self.next_id, (!parent.is_empty()).then_some(parent));
            Ok(Some(self.next_id.clone()))
        }

        fn retain_output(&self, node: &str, commit: &str) -> Result<()> {
            self.set_ref(&format!("refs/linka/outputs/{node}"), commit);
            Ok(())
        }

        fn drift(&self, id: &str, against: Option<&str>) -> Result<Option<String>> {
            if against == Some(id) {
                return Ok(None);
            }
            if let Some(error) = &self.drift_error {
                bail!("{error}");
            }
            Ok(self.drift_for.get(id).cloned())
        }

        fn files_in(&self, id: &str) -> Result<Vec<String>> {
            Ok(self.files_for.borrow().get(id).cloned().unwrap_or_default())
        }

        fn dirty_paths(&self) -> Result<Vec<String>> {
            Ok(self.dirty.clone())
        }

        fn commit_exists(&self, hash: &str) -> Result<bool> {
            Ok(self.parents.borrow().contains_key(hash))
        }

        fn head_commit(&self) -> Result<Option<String>> {
            Ok(self.head.clone())
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

        fn root_commit(&self) -> Result<Option<String>> {
            let parents = self.parents.borrow();
            let Some(mut commit) = self.head.clone() else {
                return Ok(None);
            };
            while let Some(Some(parent)) = parents.get(&commit).cloned() {
                commit = parent;
            }
            Ok(Some(commit))
        }

        fn remote_url(&self) -> Result<Option<String>> {
            Ok(self.remote.clone())
        }

        fn current_branch(&self) -> Result<Option<String>> {
            Ok(self.branch.clone())
        }

        fn ref_commit(&self, reference: &str) -> Result<Option<String>> {
            *self.ref_reads.borrow_mut() += 1;
            Ok(self.refs.borrow().get(reference).cloned())
        }

        fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
            let parents = self.parents.borrow();
            let mut current = Some(descendant.to_string());
            while let Some(commit) = current {
                if commit == ancestor {
                    return Ok(true);
                }
                current = parents.get(&commit).cloned().flatten();
            }
            Ok(false)
        }

        fn publish_fast_forward(&self, target: &str, expected: &str, new: &str) -> Result<bool> {
            if self.ref_commit(target)?.as_deref() != Some(expected) {
                return Ok(false);
            }
            if !self.is_ancestor(expected, new)? {
                return Ok(false);
            }
            self.set_ref(target, new);
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fake::FakeVcs;

    #[test]
    fn the_fake_answers_ancestry_from_modelled_parentage() {
        let vcs = FakeVcs::default();
        vcs.commit("root", None);
        vcs.commit("a", Some("root"));
        vcs.commit("b", Some("a"));
        vcs.commit("side", Some("root"));

        assert!(vcs.is_ancestor("root", "b").unwrap());
        assert!(vcs.is_ancestor("b", "b").unwrap());
        assert!(!vcs.is_ancestor("b", "a").unwrap());
        assert!(!vcs.is_ancestor("side", "b").unwrap());

        // Publication is a compare-and-swap that must also be a fast-forward.
        vcs.set_ref("refs/heads/main", "a");
        assert!(!vcs
            .publish_fast_forward("refs/heads/main", "root", "b")
            .unwrap());
        assert!(!vcs
            .publish_fast_forward("refs/heads/main", "a", "side")
            .unwrap());
        assert!(vcs
            .publish_fast_forward("refs/heads/main", "a", "b")
            .unwrap());
        assert_eq!(
            vcs.ref_commit("refs/heads/main").unwrap().as_deref(),
            Some("b")
        );
    }

    #[test]
    fn the_memo_asks_each_distinct_question_once() {
        let vcs = FakeVcs::default();
        vcs.set_ref("refs/heads/main", "a");
        let memo = MemoizingVcs::new(&vcs);
        for _ in 0..5 {
            assert_eq!(
                memo.ref_commit("refs/heads/main").unwrap().as_deref(),
                Some("a")
            );
        }
        // A proven-absent ref is an answer too, and is not asked for twice.
        assert_eq!(memo.ref_commit("refs/heads/other").unwrap(), None);
        assert_eq!(memo.ref_commit("refs/heads/other").unwrap(), None);
        assert_eq!(*vcs.ref_reads.borrow(), 2);
    }
}
