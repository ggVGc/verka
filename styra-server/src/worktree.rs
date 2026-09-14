//! The linked Git worktree an interaction works in.
//!
//! A Workspace with worktrees enabled does not hand an agent the operator's
//! checkout. Styra creates one branch and one linked checkout per interaction,
//! before the agent starts, and mounts that checkout as the sandbox workspace.
//! Nothing else about the launch changes: the agent is given a directory and
//! works in it, unaware that it is a worktree, and the operator's own checkout
//! — its index, its branch, its uncommitted files — is never mounted writable.
//!
//! The checkout is named after the interaction, which makes it durable in the
//! same sense the Session is: resuming that interaction returns to the same
//! branch, with whatever it had not committed still there.

use crate::agent::MountSpec;
use crate::git::{self, Repository};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Branches Styra creates live under this prefix, so a checkout it owns is
/// recognisable among the operator's own in `git branch`.
const BRANCH_PREFIX: &str = "styra";

/// One Workspace's durable worktree parent, and the repository its checkouts
/// are made from.
pub struct Worktrees {
    repository: Repository,
    host_root: PathBuf,
}

impl Worktrees {
    /// Prepare one Workspace's durable worktree parent.
    pub fn prepare(repository: Repository, host_root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&host_root).with_context(|| {
            format!(
                "creating Workspace worktree directory {}",
                host_root.display()
            )
        })?;
        Ok(Self {
            repository,
            host_root,
        })
    }

    /// The checkout interaction `id` works in, created with its branch the
    /// first time it is asked for.
    ///
    /// A resumed interaction asks for the same id and so returns to the
    /// checkout it left, which is the point: a provider can restore a
    /// conversation but nothing restores uncommitted files.
    pub fn checkout(&self, id: &str) -> Result<PathBuf> {
        let path = self.path(id);
        if !path.exists() {
            git::create_worktree(&self.repository.root, &format!("{BRANCH_PREFIX}/{id}"), &path)?;
        }
        Ok(path)
    }

    /// Where interaction `id` would work, without creating anything. Planning
    /// describes a launch before there is an interaction to create a checkout
    /// for.
    pub fn path(&self, id: &str) -> PathBuf {
        self.host_root.join(id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    fn temporary_directory(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "styra-worktrees-{tag}-{}-{}",
            std::process::id(),
            crate::journal::now_ms()
        ))
    }

    fn repository(tag: &str) -> (PathBuf, Repository) {
        let root = temporary_directory(tag);
        let checkout = root.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        git::fixture::init(&checkout);
        git::fixture::commit_empty(&checkout, "initial");
        let repository = git::discover(&checkout).unwrap().unwrap();
        (root, repository)
    }

    #[test]
    fn an_interaction_gets_a_branch_and_checkout_of_its_own() {
        let (root, repository) = repository("create");
        let host_root = root.join("state/worktrees");
        let worktrees = Worktrees::prepare(repository.clone(), host_root.clone()).unwrap();

        let checkout = worktrees.checkout("1757000000000-1-0").unwrap();

        assert_eq!(checkout, host_root.join("1757000000000-1-0"));
        assert_eq!(
            git::current_branch(&checkout).unwrap().as_deref(),
            Some("styra/1757000000000-1-0")
        );
        assert!(checkout.join(".git").is_file());
        let metadata = worktrees.metadata_mount();
        assert_eq!(metadata.source, repository.common_dir);
        assert_eq!(metadata.destination, repository.common_dir);
        assert!(metadata.writable);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Resuming an interaction asks for its checkout again. The uncommitted
    /// file stands in for the work a resumed agent expects to find.
    #[test]
    fn asking_twice_returns_the_same_checkout() {
        let (root, repository) = repository("resume");
        let worktrees = Worktrees::prepare(repository, root.join("state/worktrees")).unwrap();

        let first = worktrees.checkout("1757000000000-1-1").unwrap();
        std::fs::write(first.join("in-progress.txt"), "half-done").unwrap();
        let second = worktrees.checkout("1757000000000-1-1").unwrap();

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
        let (root, repository) = repository("parallel");
        let worktrees = Worktrees::prepare(repository, root.join("state/worktrees")).unwrap();

        let one = worktrees.checkout("1757000000000-1-2").unwrap();
        let two = worktrees.checkout("1757000000000-1-3").unwrap();

        assert_ne!(one, two);
        assert_ne!(
            git::current_branch(&one).unwrap(),
            git::current_branch(&two).unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_planned_checkout_is_named_without_being_created() {
        let (root, repository) = repository("planned");
        let host_root = root.join("state/worktrees");
        let worktrees = Worktrees::prepare(repository, host_root.clone()).unwrap();

        assert_eq!(worktrees.path("<pending>"), host_root.join("<pending>"));
        assert!(!host_root.join("<pending>").exists());

        std::fs::remove_dir_all(root).unwrap();
    }
}
