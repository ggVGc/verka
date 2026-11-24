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
pub use crate::protocol::BranchPoint;
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

    /// [`Self::fork`] from the local branch the operator chose as the base,
    /// refusing anything that is not one — a tag, a remote branch, or
    /// revision syntax such as `main~1` — so that what the new branch
    /// integrates into is a branch.
    pub fn fork_branch(&self, id: &str, topic: Option<&str>, branch: &str) -> Result<Checkout> {
        if let Some(existing) = self.existing(id) {
            return Ok(Checkout::at(existing));
        }
        self.git
            .local_branch_commit(&self.repository.root, branch)?
            .with_context(|| format!("base branch {branch} does not exist"))?;
        self.fork(id, topic, branch)
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
mod tests;
