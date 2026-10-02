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
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Branches Styra creates live under this prefix, so a checkout it owns is
/// recognisable among the operator's own in `git branch`.
const BRANCH_PREFIX: &str = "styra";

/// The checkout and branch one Session was given, as the Session records it.
///
/// Until this was stored, the pairing lived only in the name of a directory:
/// a Session found its checkout by scanning for the one whose name ends with
/// its id. That works until something renames it, and it makes every caller
/// that wants the branch reconstruct it from a path. Stored with the Session,
/// the pairing is a fact the Session states rather than one the filesystem
/// happens to still imply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    /// Where the Session works, on the host. `None` once the checkout has been
    /// cleaned up — see [`Worktrees::remove`] — which leaves the Session with
    /// its branch and no directory: the work is committed on the branch, and
    /// the next launch checks it out again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The branch checked out there, as `git branch` shows it.
    pub branch: String,
    /// Where the branch was made from, read when Styra made it. `None` for a
    /// checkout recorded before this was, and for one whose repository had no
    /// commit yet to start from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branched_from: Option<BranchPoint>,
}

/// The branch and commit a Session's branch was created from.
///
/// The commit is the one the branch was actually made at, not the one its
/// origin points to now: an origin branch moves on, and what the Session's
/// branch has that it did not is measured from here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPoint {
    /// The branch the new one was made from, as `git branch` shows it.
    /// `None` when the repository's head was detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The commit the new branch started at.
    pub commit: String,
}

impl Checkout {
    /// What the checkout at `path` is, for a path this module named.
    ///
    /// The branch is not read back from Git: the name on disk and the branch
    /// are written from the same string by [`Worktrees::checkout`], so the
    /// directory is the record. Deriving it is what lets a Session stored
    /// before this field existed be described without its checkout being
    /// touched.
    pub fn at(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            branch: format!("{BRANCH_PREFIX}/{name}"),
            path: Some(path),
            branched_from: None,
        }
    }

    /// The same checkout with its directory gone: what a Session records once
    /// its worktree has been cleaned up.
    pub fn without_worktree(&self) -> Self {
        Self {
            path: None,
            ..self.clone()
        }
    }

    /// The same checkout working in `path`: what a Session records once its
    /// branch has been checked out again, or once it shares this checkout.
    pub fn with_worktree(&self, path: PathBuf) -> Self {
        Self {
            path: Some(path),
            ..self.clone()
        }
    }

    /// How this checkout reads in a message: the directory it works in, or the
    /// branch alone once there is no directory left to name.
    pub fn describe(&self) -> String {
        match &self.path {
            Some(path) => path.display().to_string(),
            None => format!("branch {}", self.branch),
        }
    }
}

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
    ///
    /// A checkout made here records the branch and commit the repository had
    /// checked out, and the branch is made at that commit rather than at
    /// whatever the head is by the time Git gets to it, so the record is
    /// where the branch really starts. A checkout found already made says
    /// nothing about where it came from: that was for its creation to record.
    pub fn checkout(&self, id: &str, topic: Option<&str>) -> Result<Checkout> {
        if let Some(existing) = self.existing(id) {
            return Ok(Checkout::at(existing));
        }
        let root = &self.repository.root;
        let path = self.host_root.join(named(id, topic));
        let branch = format!("{BRANCH_PREFIX}/{}", named(id, topic));
        let origin = self.git.current_branch(root)?;
        let branched_from = match self.git.commit(root, "HEAD")? {
            Some(commit) => {
                self.git.fork_worktree(root, &branch, &path, &commit)?;
                Some(BranchPoint {
                    branch: origin,
                    commit,
                })
            }
            // Nothing committed yet: Git starts the branch with no history,
            // and there is no commit to say it came from.
            None => {
                self.git.create_worktree(root, &branch, &path)?;
                None
            }
        };
        Ok(Checkout {
            path: Some(path),
            branch,
            branched_from,
        })
    }

    /// The checkout interaction `id` works in, created on a branch of
    /// `start_point` the first time it is asked for.
    ///
    /// [`Self::checkout`] branches from the repository as it stands, which is
    /// where a Session started from nothing begins. A Session branched from
    /// another one continues that Session's conversation, so it continues its
    /// work too: the branch starts where the source Session's branch is, and
    /// the two diverge from there. Only committed work comes along —
    /// uncommitted files stay in the checkout that holds them, which is the
    /// source Session's, still working in it.
    pub fn fork(&self, id: &str, topic: Option<&str>, start_point: &str) -> Result<Checkout> {
        if let Some(existing) = self.existing(id) {
            return Ok(Checkout::at(existing));
        }
        let root = &self.repository.root;
        let commit = self.git.commit(root, start_point)?.with_context(|| {
            format!("branch {start_point} has no commit to start a new branch from")
        })?;
        let path = self.host_root.join(named(id, topic));
        let branch = format!("{BRANCH_PREFIX}/{}", named(id, topic));
        self.git.fork_worktree(root, &branch, &path, &commit)?;
        Ok(Checkout {
            path: Some(path),
            branch,
            branched_from: Some(BranchPoint {
                branch: Some(start_point.to_owned()),
                commit,
            }),
        })
    }

    /// Check `branch` out again in a checkout of its own, for a Session whose
    /// worktree was cleaned up after it while the branch stayed.
    ///
    /// The directory is named from the branch rather than from the id, because
    /// the two were written from one string when the checkout was made: a
    /// restored Session lands where it was, under the name the operator has
    /// already seen. A branch from somewhere else — one the agent switched to
    /// — is checked out under its own last segment, which is the only name
    /// there is to give it.
    pub fn restore(&self, branch: &str) -> Result<PathBuf> {
        let path = self.path_for_branch(branch);
        if path.is_dir() {
            return Ok(path);
        }
        self.git
            .add_worktree(&self.repository.root, branch, &path)?;
        Ok(path)
    }

    /// Where [`Self::restore`] would check `branch` out, without creating
    /// anything — what a plan has to be able to say about a Session that
    /// currently has a branch and no directory.
    pub fn path_for_branch(&self, branch: &str) -> PathBuf {
        self.host_root.join(directory_for(branch))
    }

    /// Remove the linked checkout at `path`, leaving its branch behind.
    pub fn remove(&self, path: &Path) -> Result<()> {
        self.git.remove_worktree(&self.repository.root, path)
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

/// The topic in the name of interaction `id`'s checkout at `path` — the
/// readable half [`named`] wrote in front of the id — or `None` for a checkout
/// named by id alone.
///
/// Read back rather than re-derived: a branched Session is about the same work
/// as the Session it came from, and the source's topic is already a Git-safe
/// fragment the operator has seen, so its sibling carries it rather than
/// paying a model to name the same work twice.
pub fn topic_of(path: &Path, id: &str) -> Option<String> {
    let name = path.file_name().and_then(|name| name.to_str())?;
    Some(name.strip_suffix(&format!("-{id}"))?.to_owned()).filter(|topic| !topic.is_empty())
}

/// The checkout directory a branch belongs in: the name [`named`] wrote, with
/// the prefix that marks the branch as Styra's taken back off.
fn directory_for(branch: &str) -> String {
    branch
        .strip_prefix(&format!("{BRANCH_PREFIX}/"))
        .unwrap_or(branch)
        .replace('/', "-")
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

        let checkout = worktrees
            .checkout("1757000000000-1-0", None)
            .unwrap()
            .path
            .unwrap();

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

    /// A new checkout says where its branch came from: the branch the
    /// repository had checked out, at the commit it was at then — which stays
    /// the answer after that branch moves on.
    #[test]
    fn a_new_checkout_records_the_branch_and_commit_it_was_made_from() {
        let (root, git, worktrees) = workspace("branched-from");
        let start = git.commit(&root.join("checkout"), "HEAD").unwrap().unwrap();

        let checkout = worktrees
            .checkout("1757000000000-1-8", Some("record-the-origin"))
            .unwrap();
        let moved_on = git.commit_on("main");

        assert_eq!(
            checkout.branched_from,
            Some(BranchPoint {
                branch: Some("main".to_owned()),
                commit: start.clone(),
            })
        );
        assert_ne!(moved_on, start);
        assert_eq!(
            git.commit(checkout.path.as_deref().unwrap(), "HEAD")
                .unwrap(),
            Some(start),
            "the branch starts where the record says it does"
        );
        // Asking again finds the checkout and does not invent its origin.
        assert_eq!(
            worktrees
                .checkout("1757000000000-1-8", None)
                .unwrap()
                .branched_from,
            None
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A Session branched from another records the source's branch, and the
    /// commit that branch was at when the two diverged.
    #[test]
    fn a_forked_checkout_records_the_branch_it_was_forked_from() {
        let (root, git, worktrees) = workspace("forked-from");
        let source = worktrees
            .checkout("1757000000000-1-9", Some("record-the-origin"))
            .unwrap();
        let source_commit = git.commit_on(&source.branch);

        let forked = worktrees
            .fork(
                "1757000000000-2-0",
                Some("record-the-origin"),
                &source.branch,
            )
            .unwrap();

        assert_eq!(
            forked.branched_from,
            Some(BranchPoint {
                branch: Some(source.branch.clone()),
                commit: source_commit,
            })
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A Session branched from another one gets a checkout of its own, on a
    /// branch of the source's rather than of the repository's own head — and
    /// under the source's topic, because it is about the same work.
    #[test]
    fn a_branched_interaction_forks_the_checkout_it_came_from() {
        let (root, git, worktrees) = workspace("fork");
        let host_root = root.join("state/worktrees");
        let source = worktrees
            .checkout("1757000000000-1-0", Some("teach-the-picker-to-filter"))
            .unwrap()
            .path
            .unwrap();
        let topic = topic_of(&source, "1757000000000-1-0");
        assert_eq!(topic.as_deref(), Some("teach-the-picker-to-filter"));

        let forked = worktrees
            .fork(
                "1757000000000-1-1",
                topic.as_deref(),
                "styra/teach-the-picker-to-filter-1757000000000-1-0",
            )
            .unwrap()
            .path
            .unwrap();

        assert_eq!(
            forked,
            host_root.join("teach-the-picker-to-filter-1757000000000-1-1")
        );
        assert_ne!(forked, source, "a branch works apart from its source");
        assert_eq!(
            git.current_branch(&forked).unwrap().as_deref(),
            Some("styra/teach-the-picker-to-filter-1757000000000-1-1")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A checkout named by id alone — an interaction that never had a topic —
    /// passes none on, rather than handing its sibling half an id as a name.
    #[test]
    fn a_checkout_named_by_id_alone_has_no_topic() {
        assert_eq!(
            topic_of(
                Path::new("/state/worktrees/1757000000000-1-0"),
                "1757000000000-1-0"
            ),
            None
        );
    }

    /// Resuming an interaction asks for its checkout again. The uncommitted
    /// file stands in for the work a resumed agent expects to find.
    #[test]
    fn asking_twice_returns_the_same_checkout() {
        let (root, _git, worktrees) = workspace("resume");

        let first = worktrees
            .checkout("1757000000000-1-1", Some("teach-the-picker-to-filter"))
            .unwrap()
            .path
            .unwrap();
        std::fs::write(first.join("in-progress.txt"), "half-done").unwrap();
        // A resume knows only the id, and the topic is not repeated to it.
        let second = worktrees
            .checkout("1757000000000-1-1", None)
            .unwrap()
            .path
            .unwrap();

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
            .unwrap()
            .path
            .unwrap();
        let two = worktrees
            .checkout("1757000000000-1-3", Some("fix-the-flaky-test"))
            .unwrap()
            .path
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
            .unwrap()
            .path
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

    /// Cleaning up after a finished interaction takes the directory and
    /// leaves the branch, and returning to it is the branch being checked out
    /// where it was — under the name the operator has already seen in their
    /// worktree directory and in `git branch`.
    #[test]
    fn a_removed_checkout_comes_back_on_the_branch_it_left_behind() {
        let (root, git, worktrees) = workspace("restore");

        let checkout = worktrees
            .checkout("1757000000000-1-6", Some("tidy-the-worktrees"))
            .unwrap()
            .path
            .unwrap();
        let branch = git.current_branch(&checkout).unwrap().unwrap();
        worktrees.remove(&checkout).unwrap();

        assert!(!checkout.exists());
        assert!(git.has_branch(&branch), "the branch went with the checkout");
        assert_eq!(worktrees.path_for_branch(&branch), checkout);

        let restored = worktrees.restore(&branch).unwrap();

        assert_eq!(restored, checkout);
        assert_eq!(git.current_branch(&restored).unwrap(), Some(branch));

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Asking for a checkout that is already there is what a second cleanup
    /// pass, or a resume racing one, has to be safe to do.
    #[test]
    fn restoring_a_checkout_that_is_still_there_returns_it() {
        let (root, git, worktrees) = workspace("restore-existing");

        let checkout = worktrees
            .checkout("1757000000000-1-7", None)
            .unwrap()
            .path
            .unwrap();
        let branch = git.current_branch(&checkout).unwrap().unwrap();
        std::fs::write(checkout.join("in-progress.txt"), "half-done").unwrap();

        assert_eq!(worktrees.restore(&branch).unwrap(), checkout);
        assert!(checkout.join("in-progress.txt").exists());

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
            .unwrap()
            .path
            .unwrap();

        let branch = git.current_branch(&checkout).unwrap().unwrap();
        assert!(
            branch.starts_with(&format!("{BRANCH_PREFIX}/")),
            "{branch} is not recognisable as Styra's"
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
