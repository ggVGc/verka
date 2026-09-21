//! The linked Git worktree an interaction works in.
//!
//! A Workspace with worktrees enabled does not hand an agent the operator's
//! checkout. Styra creates one branch and one linked checkout per interaction,
//! before the agent starts, and mounts that checkout as the sandbox workspace.
//! Nothing else about the launch changes: the agent is given a directory and
//! works in it, unaware that it is a worktree, and the operator's own checkout
//! — its index, its branch, its uncommitted files — is never mounted writable.
//!
//! The checkout carries the interaction's id, which makes it durable in the
//! same sense the Session is: resuming that interaction returns to the same
//! branch, with whatever it had not committed still there. In front of the id
//! it carries the interaction's topic, so that the branch an operator later
//! finds in their own `git branch` says what is on it; [`crate::naming`]
//! writes that half.

use crate::agent::MountSpec;
use crate::git::{Git, Repository};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Branches Styra creates live under this prefix, so a checkout it owns is
/// recognisable among the operator's own in `git branch`.
const BRANCH_PREFIX: &str = "styra";

/// One Workspace's durable worktree parent, and the repository its checkouts
/// are made from.
pub struct Worktrees {
    git: Arc<dyn Git>,
    repository: Repository,
    host_root: PathBuf,
}

impl Worktrees {
    /// Prepare one Workspace's durable worktree parent.
    pub fn prepare(git: Arc<dyn Git>, repository: Repository, host_root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&host_root).with_context(|| {
            format!(
                "creating Workspace worktree directory {}",
                host_root.display()
            )
        })?;
        Ok(Self {
            git,
            repository,
            host_root,
        })
    }

    /// The checkout interaction `id` works in, created with its branch the
    /// first time it is asked for.
    ///
    /// `topic` is what the work is about — see [`crate::naming`] — and is used
    /// only when the checkout is created; it is the readable half of the name,
    /// the id the unique half. A resumed interaction asks for the same id and
    /// so returns to the checkout it left, which is the point: a provider can
    /// restore a conversation but nothing restores uncommitted files. It
    /// passes no topic and needs none, because the name is on disk already.
    pub fn checkout(&self, id: &str, topic: Option<&str>) -> Result<PathBuf> {
        if let Some(existing) = self.existing(id) {
            return Ok(existing);
        }
        let path = self.host_root.join(named(id, topic));
        self.git.create_worktree(
            &self.repository.root,
            &format!("{BRANCH_PREFIX}/{}", named(id, topic)),
            &path,
        )?;
        Ok(path)
    }

    /// Where interaction `id` would work, without creating anything. Planning
    /// describes a launch before there is an interaction to create a checkout
    /// for — and so before there is a prompt to name one after, which is why
    /// this is the unnamed form.
    pub fn path(&self, id: &str) -> PathBuf {
        self.existing(id).unwrap_or_else(|| self.host_root.join(id))
    }

    /// The checkout already made for interaction `id`, whatever it ended up
    /// being called.
    fn existing(&self, id: &str) -> Option<PathBuf> {
        existing_checkout(&self.host_root, id)
    }

    /// The repository's shared Git metadata, writable at its host path.
    ///
    /// A linked checkout holds no history of its own: its `.git` file names
    /// this directory by absolute path, and every object, ref and index update
    /// lands there. Without it the checkout is a directory of files that Git
    /// cannot read, so it is mounted for every interaction that gets one.
    pub fn metadata_mount(&self) -> MountSpec {
        MountSpec {
            source: self.repository.common_dir.clone(),
            destination: self.repository.common_dir.clone(),
            writable: true,
        }
    }
}

/// The checkout already made for interaction `id` under `host_root`, whatever
/// it ended up being called, without preparing anything.
///
/// The id is the last component of every name this module writes, so a Session
/// finds its own checkout without anything having to store the mapping —
/// including a Session created before naming existed, whose checkout is the
/// bare id. A caller that must know whether a Session has a checkout *before*
/// it decides to make one asks here: [`Worktrees::prepare`] creates the parent
/// directory, so it cannot be the thing that answers the question.
pub fn existing_checkout(host_root: &Path, id: &str) -> Option<PathBuf> {
    let bare = host_root.join(id);
    if bare.exists() {
        return Some(bare);
    }
    let suffix = format!("-{id}");
    std::fs::read_dir(host_root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        })
}

/// What the branch and the checkout are both called: the topic, then the
/// interaction it belongs to. Either half alone would be worse — a topic
/// without the id could collide between two Sessions given the same task, and
/// an id without the topic is what `git branch` showed before.
fn named(id: &str, topic: Option<&str>) -> String {
    match topic {
        Some(topic) => format!("{topic}-{id}"),
        None => id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FakeGit;

    fn temporary_directory(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "styra-worktrees-{tag}-{}-{}",
            std::process::id(),
            crate::journal::now_ms()
        ))
    }

    /// A Workspace on a repository, with the Git that backs it. No process and
    /// no history: what these tests are about is which checkout an interaction
    /// gets and what it is called, and neither depends on there being commits.
    fn workspace(tag: &str) -> (PathBuf, Arc<FakeGit>, Worktrees) {
        let root = temporary_directory(tag);
        let git = Arc::new(FakeGit::new());
        let repository = git.init(&root.join("checkout"));
        let worktrees =
            Worktrees::prepare(git.clone(), repository, root.join("state/worktrees")).unwrap();
        (root, git, worktrees)
    }

    #[test]
    fn an_interaction_gets_a_branch_and_checkout_of_its_own() {
        let (root, git, worktrees) = workspace("create");
        let host_root = root.join("state/worktrees");

        let checkout = worktrees.checkout("1757000000000-1-0", None).unwrap();

        assert_eq!(checkout, host_root.join("1757000000000-1-0"));
        assert_eq!(
            git.current_branch(&checkout).unwrap().as_deref(),
            Some("styra/1757000000000-1-0")
        );
        assert!(checkout.join(".git").is_file());
        let metadata = worktrees.metadata_mount();
        let common_dir = root.join("checkout/.git").canonicalize().unwrap();
        assert_eq!(metadata.source, common_dir);
        assert_eq!(metadata.destination, common_dir);
        assert!(metadata.writable);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Resuming an interaction asks for its checkout again. The uncommitted
    /// file stands in for the work a resumed agent expects to find.
    #[test]
    fn asking_twice_returns_the_same_checkout() {
        let (root, _git, worktrees) = workspace("resume");

        let first = worktrees
            .checkout("1757000000000-1-1", Some("teach-the-picker-to-filter"))
            .unwrap();
        std::fs::write(first.join("in-progress.txt"), "half-done").unwrap();
        // A resume knows only the id, and the topic is not repeated to it.
        let second = worktrees.checkout("1757000000000-1-1", None).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            std::fs::read_to_string(second.join("in-progress.txt")).unwrap(),
            "half-done"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Two interactions in one Workspace share a repository and nothing else:
    /// separate branches, separate indexes, separate files.
    #[test]
    fn two_interactions_do_not_share_a_checkout() {
        let (root, git, worktrees) = workspace("parallel");

        // Two Sessions given the same task are named the same thing, and the
        // id each carries is what keeps their branches apart.
        let one = worktrees
            .checkout("1757000000000-1-2", Some("fix-the-flaky-test"))
            .unwrap();
        let two = worktrees
            .checkout("1757000000000-1-3", Some("fix-the-flaky-test"))
            .unwrap();

        assert_ne!(one, two);
        assert_ne!(
            git.current_branch(&one).unwrap(),
            git.current_branch(&two).unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// What the operator sees in `git branch` and in their worktree directory
    /// is the work, with the interaction it belongs to after it.
    #[test]
    fn a_named_interaction_gets_a_branch_that_says_what_it_is_for() {
        let (root, git, worktrees) = workspace("named");
        let host_root = root.join("state/worktrees");

        let checkout = worktrees
            .checkout("1757000000000-1-4", Some("fix-flaky-checkout-test"))
            .unwrap();

        assert_eq!(
            checkout,
            host_root.join("fix-flaky-checkout-test-1757000000000-1-4")
        );
        assert_eq!(
            git.current_branch(&checkout).unwrap().as_deref(),
            Some("styra/fix-flaky-checkout-test-1757000000000-1-4")
        );
        // And the Session finds it again from the id alone, which is all a
        // resume or a plan has.
        assert_eq!(worktrees.path("1757000000000-1-4"), checkout);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_planned_checkout_is_named_without_being_created() {
        let (root, _git, worktrees) = workspace("planned");
        let host_root = root.join("state/worktrees");

        assert_eq!(worktrees.path("<pending>"), host_root.join("<pending>"));
        assert!(!host_root.join("<pending>").exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    /// The branch name is the operator's view of the work, so the prefix that
    /// marks it as Styra's is part of the contract, not decoration.
    #[test]
    fn every_branch_styra_creates_is_under_its_own_prefix() {
        let (root, git, worktrees) = workspace("prefix");

        let checkout = worktrees
            .checkout("1757000000000-1-5", Some("rename-the-thing"))
            .unwrap();

        let branch = git.current_branch(&checkout).unwrap().unwrap();
        assert!(
            branch.starts_with(&format!("{BRANCH_PREFIX}/")),
            "{branch} is not recognisable as Styra's"
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
