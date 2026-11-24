//! Every Git operation Styra performs: the one place an assumption about
//! Git's on-disk layout is written down, and the one place a `git` process is
//! built.
//!
//! Git knowledge is here rather than beside each caller because the callers
//! are spread across both crates and both sides of the sandbox boundary, and
//! they have to agree: a client deciding which directories a launch must mount
//! and a server deciding what a checkout is have to answer from one model of
//! what Git keeps where.
//!
//! Which mechanism answers a question is part of that model. A question about
//! a repository that is *reachable* runs `git`, which is authoritative — and
//! runs it on the host, because a sandboxed process cannot reliably discover
//! an enclosing repository: the Workspace may be a bind mount of one directory
//! below the checkout root, and the repository metadata then lives outside the
//! sandbox, so discovery has to happen before Driva decides what to mount. A
//! question a launch has to answer *before* the history is reachable — which
//! directories carry it, so that they can be mounted at all — is answered by
//! reading Git's layout, since running `git` in that checkout is exactly what
//! fails.
//!
//! Running `git` is a seam, not a fact: [`Git`] names the questions that
//! need a process, [`SystemGit`] answers them by spawning one, and [`FakeGit`]
//! answers them from an in-memory model. Everything derived from those answers
//! — which root a path belongs to, which directories a launch must mount — is
//! a default method on the trait, so the interesting logic is written once and
//! tested without a `git` binary anywhere.

use crate::agent::MountSpec;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// The checkout and shared metadata Git associates with a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    /// Root of the checkout containing the directory passed to [`discover`].
    pub root: PathBuf,
    /// Git's common metadata directory. For a normal checkout this is `.git`;
    /// for a linked worktree it is the main checkout's shared `.git` directory.
    pub common_dir: PathBuf,
}

/// The two metadata directories a checkout uses: its own, and the one it
/// shares with every other checkout of the same repository.
///
/// They differ only for a linked worktree, whose `git_dir` is a per-worktree
/// directory under the main checkout while the objects and refs stay in the
/// `common_dir` beside it. A launch has to mount both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directories {
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

/// Every question about a repository that only a running `git` can answer.
///
/// Deliberately few methods. Each one is a single `git`
/// invocation whose output Styra parses; everything Styra *decides* from those
/// answers is a default method below, so a test that cares about the deciding
/// — which is nearly all of them — runs against [`FakeGit`] and never spawns a
/// process. [`SystemGit`] is the only implementation that does.
pub trait Git: Send + Sync {
    /// The common ancestor used when an older checkout has no recorded base.
    fn merge_base(&self, _checkout: &Path, _left: &str, _right: &str) -> Result<String> {
        anyhow::bail!("this Git backend cannot determine the checkout's base commit")
    }
    /// Whether two revisions have different committed contents. The default
    /// models the commit-only history used by fake backends.
    fn has_committed_changes(&self, checkout: &Path, base: &str, head: &str) -> Result<bool> {
        Ok(self.commit(checkout, base)? != self.commit(checkout, head)?)
    }
    /// Integration operations. Fakes which do not model history refuse them.
    fn rebase(&self, _checkout: &Path, _source: &str) -> Result<bool> {
        anyhow::bail!("this Git backend does not support rebasing")
    }
    /// Local branches ordered by their tip's committer date, newest first.
    fn diff_branches(&self, _checkout: &Path) -> Result<Vec<String>> {
        anyhow::bail!("this Git backend cannot list comparison branches")
    }
    fn local_branch_commit(&self, checkout: &Path, branch: &str) -> Result<Option<String>> {
        self.commit(checkout, &format!("refs/heads/{branch}"))
    }
    fn rebase_onto(&self, _checkout: &Path, _source: &str, _upstream: &str) -> Result<bool> {
        anyhow::bail!("this Git backend does not support retargeting")
    }
    /// Create `branch` at `start` without checking it out.
    fn create_branch(&self, _checkout: &Path, _branch: &str, _start: &str) -> Result<()> {
        anyhow::bail!("this Git backend does not support creating branches")
    }
    /// Move the checked-out branch, its index and working tree to `commit`.
    fn reset_hard(&self, _checkout: &Path, _commit: &str) -> Result<()> {
        anyhow::bail!("this Git backend does not support resetting branches")
    }
    fn rebase_in_progress(&self, _checkout: &Path) -> Result<bool> {
        anyhow::bail!("this Git backend does not support rebasing")
    }
    fn branch_worktree(&self, _checkout: &Path, _branch: &str) -> Result<Option<PathBuf>> {
        anyhow::bail!("this Git backend does not support integration")
    }
    fn squash_merge(&self, _checkout: &Path, _branch: &str, _message: &str) -> Result<String> {
        anyhow::bail!("this Git backend does not support integration")
    }
    fn integration_diff(&self, _checkout: &Path, _source: &str) -> Result<String> {
        anyhow::bail!("this Git backend does not support integration")
    }
    fn is_ancestor(&self, _checkout: &Path, _ancestor: &str, _descendant: &str) -> Result<bool> {
        anyhow::bail!("this Git backend does not support integration")
    }
    /// The messages of the commits `checkout` has made since `base`, newest
    /// first. Merges are left out: their messages say where work came from,
    /// not what it was for.
    fn commit_messages(&self, _checkout: &Path, _base: &str) -> Result<String> {
        anyhow::bail!("this Git backend does not model commit messages")
    }
    /// Discover the Git checkout containing `start`.
    ///
    /// `Ok(None)` means the path is not inside a working tree. Other failures
    /// — an unreadable path, a missing Git executable, or malformed successful
    /// output — remain errors so callers do not silently discard a repository
    /// they should have mounted.
    fn discover(&self, start: &Path) -> Result<Option<Repository>>;

    /// The metadata directories of the checkout rooted at `root`.
    fn directories(&self, root: &Path) -> Result<Directories>;

    /// Create `branch` as a new branch, checked out in a linked worktree at
    /// `path`.
    fn create_worktree(&self, repository: &Path, branch: &str, path: &Path) -> Result<()>;

    /// Create `branch` at `start_point` and check it out in a linked worktree
    /// at `path`.
    ///
    /// [`Self::create_worktree`] starts the branch from whatever the
    /// repository has checked out, which is where work begun from nothing
    /// belongs. Work that continues another Session's starts where that
    /// Session's own branch is instead, so the point is named.
    fn fork_worktree(
        &self,
        repository: &Path,
        branch: &str,
        path: &Path,
        start_point: &str,
    ) -> Result<()>;

    /// Check an existing `branch` out in a linked worktree at `path`.
    ///
    /// The counterpart of [`Self::create_worktree`] for a branch that is
    /// already there: a Session whose worktree was cleaned up keeps its
    /// branch, so returning to it is a checkout of that branch rather than the
    /// creation of another one.
    fn add_worktree(&self, repository: &Path, branch: &str, path: &Path) -> Result<()>;

    /// Remove the linked worktree at `path`, leaving the branch it had checked
    /// out in the repository.
    ///
    /// Git refuses a worktree with modified or untracked files, which is a
    /// second floor under the caller's own check rather than a substitute for
    /// it: the answer a cleanup pass reports has to be decided before anything
    /// is deleted.
    fn remove_worktree(&self, repository: &Path, path: &Path) -> Result<()>;

    /// The branch checked out in `checkout`, or `None` when its head is
    /// detached.
    fn current_branch(&self, checkout: &Path) -> Result<Option<String>>;

    /// The commit `revision` names, as seen from `checkout`, or `None` when it
    /// names none — which is what `HEAD` answers on a branch with no commits
    /// yet.
    fn commit(&self, checkout: &Path, revision: &str) -> Result<Option<String>>;

    /// Whether `checkout` has work that is not committed: anything `git
    /// status` would report, tracked or not.
    ///
    /// Asked of a checkout an agent has just stopped working in, so it is
    /// deliberately the same coarse question the operator would ask by
    /// running `git status` there — what is uncommitted, not what changed in
    /// this turn. Untracked files count: a turn that wrote a new file and
    /// never added it has left work behind just as surely as one that edited
    /// a tracked one.
    fn has_uncommitted_changes(&self, checkout: &Path) -> Result<bool>;

    /// Stage everything `git status` reports in `checkout` — tracked or not —
    /// and commit it with `message`, returning the new commit's abbreviated
    /// id.
    ///
    /// The same coarse scope as [`Self::has_uncommitted_changes`], and asked
    /// after it: a turn's commit is of what the turn left, which is what the
    /// operator would otherwise have been told was uncommitted.
    fn commit_all(&self, checkout: &Path, message: &str) -> Result<String>;

    /// The diff from `HEAD` to everything in `checkout`'s working tree —
    /// tracked or not, the same scope as [`Self::has_uncommitted_changes`] —
    /// in a form `git apply` takes back, binary files included. On a branch
    /// with no commits yet it is the whole tree, as additions.
    ///
    /// Read without touching the checkout's own index: what the operator has
    /// staged is theirs, and a snapshot must not change it.
    fn working_tree_diff(&self, checkout: &Path) -> Result<Vec<u8>>;

    /// The tree a snapshot recorded: `commit` — or nothing, for a branch with
    /// no commits yet — with `diff`, as [`Self::working_tree_diff`] wrote it,
    /// applied on top. Written to the repository `checkout` is in, without
    /// touching that checkout or its index, and returned by id.
    ///
    /// Rebuilt from what the snapshot stored rather than kept from when it was
    /// taken: a tree nothing refers to is Git's to collect, and the commit and
    /// diff are what the snapshot keeps durably.
    fn snapshot_tree(
        &self,
        checkout: &Path,
        commit: Option<&str>,
        diff: Option<&[u8]>,
    ) -> Result<String>;

    /// The diff from one tree to another, for reading.
    fn tree_diff(&self, checkout: &Path, from: &str, to: &str) -> Result<String>;

    /// Resolve `path` to the root of its nearest enclosing Git checkout.
    fn repository_root(&self, path: &Path) -> Result<PathBuf> {
        self.discover(path)?
            .map(|repository| repository.root)
            .with_context(|| format!("{} is not inside a Git repository", path.display()))
    }

    /// Mandatory mounts for a Workspace's associated repository.
    ///
    /// The checkout itself is read-only; the directories carrying history are
    /// writable, because an agent that commits writes objects and refs. A
    /// linked worktree contributes its own `git_dir` as well, since its index
    /// and HEAD live there rather than in the shared directory.
    fn mounts(&self, root: &Path) -> Result<Vec<MountSpec>> {
        let root = self.repository_root(root)?;
        let Directories {
            git_dir,
            common_dir,
        } = self.directories(&root)?;

        let mut mounts = Vec::new();
        push_mount(&mut mounts, root, false);
        if let Some(parent) = common_dir.parent() {
            push_mount(&mut mounts, parent.to_path_buf(), false);
        }
        push_mount(&mut mounts, common_dir.clone(), true);
        if !git_dir.starts_with(&common_dir) {
            if let Some(parent) = git_dir.parent() {
                push_mount(&mut mounts, parent.to_path_buf(), false);
            }
            push_mount(&mut mounts, git_dir, true);
        }
        Ok(mounts)
    }
}

/// The real thing: every method spawns `git`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGit;

impl SystemGit {
    /// A shareable handle, which is how [`crate::server::ServerState`] and
    /// [`crate::worktree::Worktrees`] hold their Git.
    pub fn shared() -> Arc<dyn Git> {
        Arc::new(Self)
    }
}

impl Git for SystemGit {
    fn merge_base(&self, checkout: &Path, left: &str, right: &str) -> Result<String> {
        let base = Invocation::new(checkout, "determine the checkout's base commit")
            .args(["merge-base", left, right])
            .output()?;
        let base = base.trim();
        anyhow::ensure!(!base.is_empty(), "the checkout has no common base commit");
        Ok(base.to_owned())
    }
    fn has_committed_changes(&self, checkout: &Path, base: &str, head: &str) -> Result<bool> {
        let output = Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args([
                "diff",
                "--quiet",
                "--no-ext-diff",
                "--no-textconv",
                base,
                head,
                "--",
            ])
            .output()?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => anyhow::bail!(
                "could not compare checkout contents: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }
    fn is_ancestor(&self, checkout: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
        let output = Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .output()?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => anyhow::bail!(
                "could not compare integration commits: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }
    fn rebase_in_progress(&self, checkout: &Path) -> Result<bool> {
        let directory = self.directories(checkout)?.git_dir;
        Ok(directory.join("rebase-merge").exists() || directory.join("rebase-apply").exists())
    }

    fn rebase(&self, checkout: &Path, source: &str) -> Result<bool> {
        if self.rebase_in_progress(checkout)? {
            return Ok(false);
        }
        anyhow::ensure!(
            !self.has_uncommitted_changes(checkout)?,
            "commit the interaction's changes before integrating"
        );
        match Invocation::new(checkout, "rebase onto the source branch")
            .args(["rebase", "--", source])
            .succeed()
        {
            Ok(()) => Ok(true),
            Err(_) if self.rebase_in_progress(checkout)? => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn diff_branches(&self, checkout: &Path) -> Result<Vec<String>> {
        Ok(Invocation::new(checkout, "list comparison branches")
            .args([
                "for-each-ref",
                "--sort=refname",
                "--sort=-committerdate",
                "--format=%(refname:strip=2)",
                "refs/heads/",
            ])
            .output()?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    fn local_branch_commit(&self, checkout: &Path, branch: &str) -> Result<Option<String>> {
        // check-ref-format rejects revision syntax such as main~1 or main^{commit}.
        Invocation::new(checkout, "validate the target branch")
            .args(["check-ref-format", &format!("refs/heads/{branch}")])
            .succeed()?;
        self.commit(checkout, &format!("refs/heads/{branch}"))
    }

    fn rebase_onto(&self, checkout: &Path, source: &str, upstream: &str) -> Result<bool> {
        if self.rebase_in_progress(checkout)? {
            return Ok(false);
        }
        anyhow::ensure!(
            !self.has_uncommitted_changes(checkout)?,
            "commit changes before retargeting"
        );
        match Invocation::new(checkout, "rebase onto the new target")
            .args(["rebase", "--onto", source, upstream])
            .succeed()
        {
            Ok(()) => Ok(true),
            Err(_) if self.rebase_in_progress(checkout)? => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn create_branch(&self, checkout: &Path, branch: &str, start: &str) -> Result<()> {
        Invocation::new(checkout, "save a copy of the interaction branch")
            .args(["branch", "--no-track", branch, start])
            .succeed()
    }

    fn reset_hard(&self, checkout: &Path, commit: &str) -> Result<()> {
        Invocation::new(checkout, "move the interaction branch")
            .args(["reset", "--hard", commit])
            .succeed()
    }

    fn branch_worktree(&self, checkout: &Path, branch: &str) -> Result<Option<PathBuf>> {
        let listing = Invocation::new(checkout, "find the source checkout")
            .args(["worktree", "list", "--porcelain", "-z"])
            .output()?;
        let mut path = None;
        for field in listing.split('\0') {
            if let Some(value) = field.strip_prefix("worktree ") {
                path = Some(PathBuf::from(value));
            } else if field.strip_prefix("branch ") == Some(&format!("refs/heads/{branch}")) {
                return Ok(path);
            }
        }
        Ok(None)
    }

    fn squash_merge(&self, checkout: &Path, branch: &str, message: &str) -> Result<String> {
        anyhow::ensure!(
            !self.has_uncommitted_changes(checkout)?,
            "the source checkout has uncommitted changes"
        );
        anyhow::ensure!(
            self.is_ancestor(checkout, "HEAD", branch)?,
            "the source advanced after rebasing; retry integration"
        );
        Invocation::new(checkout, "squash merge the interaction")
            .args(["merge", "--squash", "--", branch])
            .succeed()?;
        Invocation::new(checkout, "commit the integration")
            .args(["commit", "--quiet", "--cleanup=whitespace", "--file=-"])
            .stdin(message)
            .succeed()?;
        self.commit(checkout, "HEAD")?
            .context("the integration commit is missing")
    }

    fn integration_diff(&self, checkout: &Path, source: &str) -> Result<String> {
        Invocation::new(checkout, "read the integration diff")
            .args(["diff", "--no-ext-diff", "--no-textconv"])
            .arg(format!("{source}...HEAD"))
            .arg("--")
            .output()
    }
    fn commit_messages(&self, checkout: &Path, base: &str) -> Result<String> {
        Invocation::new(checkout, "read the branch's commit messages")
            .args(["log", "--no-merges", "--format=%B"])
            .arg(format!("{base}..HEAD"))
            .arg("--")
            .output()
    }
    fn discover(&self, start: &Path) -> Result<Option<Repository>> {
        let start = start
            .canonicalize()
            .with_context(|| format!("repository search path {} must exist", start.display()))?;
        let inside = Invocation::new(&start, "detect a repository")
            .args(["rev-parse", "--is-inside-work-tree"])
            .optional_output()?;
        if inside.as_deref().map(str::trim) != Some("true") {
            return Ok(None);
        }

        let described = Invocation::new(
            &start,
            format!("describe the repository containing {}", start.display()),
        )
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ])
        .output()?;
        let mut lines = described.lines();
        let root = lines
            .next()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .context("git did not report a checkout root")?;
        let common_dir = lines
            .next()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .context("git did not report a common directory")?;
        Ok(Some(Repository { root, common_dir }))
    }

    fn directories(&self, root: &Path) -> Result<Directories> {
        let git_dir = git_path(root, ["rev-parse", "--absolute-git-dir"])
            .context("resolving Git directory")?;
        let common_dir = git_path(
            root,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .context("resolving Git common directory")?;
        Ok(Directories {
            git_dir,
            common_dir,
        })
    }

    fn create_worktree(&self, repository: &Path, branch: &str, path: &Path) -> Result<()> {
        Invocation::new(repository, "create the branch and worktree")
            .args(["worktree", "add", "-b"])
            .arg(branch)
            .arg("--")
            .arg(path)
            .succeed()
    }

    fn fork_worktree(
        &self,
        repository: &Path,
        branch: &str,
        path: &Path,
        start_point: &str,
    ) -> Result<()> {
        Invocation::new(repository, "create the branch and worktree")
            .args(["worktree", "add", "-b"])
            .arg(branch)
            .arg("--")
            .arg(path)
            .arg(start_point)
            .succeed()
    }

    fn add_worktree(&self, repository: &Path, branch: &str, path: &Path) -> Result<()> {
        Invocation::new(repository, "check the branch out in a worktree")
            .args(["worktree", "add", "--"])
            .arg(path)
            .arg(branch)
            .succeed()
    }

    fn remove_worktree(&self, repository: &Path, path: &Path) -> Result<()> {
        Invocation::new(repository, "remove the worktree")
            .args(["worktree", "remove"])
            .arg(path)
            .succeed()
    }

    fn current_branch(&self, checkout: &Path) -> Result<Option<String>> {
        let branch = Invocation::new(checkout, "read the current branch")
            .args(["branch", "--show-current"])
            .output()?;
        Ok(Some(branch).filter(|branch| !branch.is_empty()))
    }

    fn commit(&self, checkout: &Path, revision: &str) -> Result<Option<String>> {
        Invocation::new(checkout, "resolve a commit")
            .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
            .arg(format!("{revision}^{{commit}}"))
            .optional_output()
    }

    fn has_uncommitted_changes(&self, checkout: &Path) -> Result<bool> {
        let status = Invocation::new(checkout, "read the working tree status")
            .args(["status", "--porcelain"])
            .output()?;
        Ok(!status.trim().is_empty())
    }

    fn commit_all(&self, checkout: &Path, message: &str) -> Result<String> {
        Invocation::new(checkout, "stage the working tree")
            .args(["add", "--all"])
            .succeed()?;
        // On stdin rather than as `-m`: a turn's messages can outgrow the
        // length the kernel allows a single argument. `whitespace` keeps
        // lines starting with `#`, which an agent's Markdown is full of.
        Invocation::new(checkout, "commit the turn")
            .args(["commit", "--quiet", "--cleanup=whitespace", "--file=-"])
            .stdin(message)
            .succeed()?;
        Invocation::new(checkout, "read the new commit")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
    }

    fn working_tree_diff(&self, checkout: &Path) -> Result<Vec<u8>> {
        // Untracked files only appear in a diff once they are in an index, so
        // they are added to a copy of the checkout's: a copy rather than a
        // fresh one read from `HEAD`, so its cached file stats spare `git add`
        // from rehashing every tracked file.
        // Not canonicalized: a repository nothing was ever staged in has no
        // index yet, and that is an empty one.
        let index = PathBuf::from(
            Invocation::new(checkout, "locate the index")
                .args(["rev-parse", "--path-format=absolute", "--git-path", "index"])
                .output()?,
        );
        let scratch = ScratchIndex::new()?;
        if index.exists() {
            std::fs::copy(&index, &scratch.0)
                .with_context(|| format!("copying the index {} for a snapshot", index.display()))?;
        }
        Invocation::new(checkout, "stage the working tree for a snapshot")
            .env("GIT_INDEX_FILE", &scratch.0)
            .args(["add", "--all"])
            .succeed()?;
        // Without a commit named, `--cached` compares with `HEAD`, or with
        // nothing at all on a branch that has no commits yet.
        Invocation::new(checkout, "diff the working tree for a snapshot")
            .env("GIT_INDEX_FILE", &scratch.0)
            .args([
                "diff",
                "--cached",
                "--binary",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
            ])
            .bytes()
    }

    fn snapshot_tree(
        &self,
        checkout: &Path,
        commit: Option<&str>,
        diff: Option<&[u8]>,
    ) -> Result<String> {
        // From the root, because `git apply` run in a subdirectory leaves out
        // the paths outside it, and a snapshot's diff names them from the root.
        let root = self.repository_root(checkout)?;
        let scratch = ScratchIndex::new()?;
        let read = Invocation::new(&root, "read the snapshot's commit")
            .env("GIT_INDEX_FILE", &scratch.0)
            .arg("read-tree");
        match commit {
            Some(commit) => read.arg(commit),
            None => read.arg("--empty"),
        }
        .succeed()?;
        if let Some(diff) = diff {
            Invocation::new(&root, "apply the snapshot's diff")
                .env("GIT_INDEX_FILE", &scratch.0)
                .args(["apply", "--cached", "--binary", "-"])
                .stdin(diff)
                .succeed()?;
        }
        Invocation::new(&root, "write the snapshot's tree")
            .env("GIT_INDEX_FILE", &scratch.0)
            .arg("write-tree")
            .output()
    }

    fn tree_diff(&self, checkout: &Path, from: &str, to: &str) -> Result<String> {
        let diff = Invocation::new(checkout, "diff two snapshots")
            .args([
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--end-of-options",
                from,
                to,
            ])
            .bytes()?;
        // Read, not applied: a file in another encoding shows as best it can.
        Ok(String::from_utf8_lossy(&diff).into_owned())
    }
}

/// An index file of [`Git::working_tree_diff`]'s own, removed when dropped.
struct ScratchIndex(PathBuf);

impl ScratchIndex {
    fn new() -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("styra-snapshot-{}-{n}.index", std::process::id()));
        // Left behind by an earlier process with this pid, and stale.
        let _ = std::fs::remove_file(&path);
        Ok(Self(path))
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("index.lock"));
    }
}

fn git_path<I, S>(directory: &Path, arguments: I) -> Result<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let value = Invocation::new(directory, "resolve a Git path")
        .args(arguments)
        .output()?;
    if value.is_empty() {
        anyhow::bail!("git returned an empty path");
    }
    Path::new(&value)
        .canonicalize()
        .with_context(|| format!("resolving Git path {value}"))
}

fn push_mount(mounts: &mut Vec<MountSpec>, path: PathBuf, writable: bool) {
    if let Some(existing) = mounts.iter_mut().find(|mount| mount.destination == path) {
        existing.writable |= writable;
        return;
    }
    mounts.push(MountSpec {
        source: path.clone(),
        destination: path,
        writable,
    });
}

/// An in-memory Git for tests: no process, no history, no objects.
///
/// Public rather than `cfg(test)` for the reason [`crate::server::ServerState`]
/// needs it — tests in other modules, and integration tests, build a server
/// around one.
///
/// It models Git's *layout* rather than Git, and it writes that layout to
/// disk: a registered checkout really has a `.git`, a created worktree really
/// has a `.git` file naming a real per-worktree directory with a real
/// `commondir` in it. That is deliberate. [`enclosing_root`] and
/// [`history_directories`] answer by reading those files and never consult
/// this trait at all, so a fake that only kept paths in a map would let the
/// two halves of this module disagree in a way the real thing cannot.
#[derive(Default)]
pub struct FakeGit {
    checkouts: Mutex<Vec<Checkout>>,
    /// Every branch the repository has, whether or not a checkout has it out.
    /// Kept apart from [`Self::checkouts`] because that is exactly what
    /// removing a worktree separates: the checkout goes and the branch stays,
    /// so a fake that read branches off its checkouts would let a cleaned-up
    /// Session's branch be created a second time.
    ///
    /// Each branch names the commit it is at. The fake has no history, so a
    /// commit is only a name — but a branch made from another starts at the
    /// same one, which is the part of history a caller can observe.
    branches: Mutex<BTreeMap<String, String>>,
    /// How many commits the fake has named, so each new one is distinct.
    named_commits: Mutex<u64>,
    /// Roots a test has declared to have uncommitted work. A fake with no
    /// history cannot derive this, and the callers only ever ask the
    /// question, so it is simply stated.
    dirty: Mutex<Vec<PathBuf>>,
    /// Every commit [`Git::commit_all`] made, as the checkout root it was made
    /// in and its message, oldest first.
    commits: Mutex<Vec<(PathBuf, String)>>,
}

/// One checkout the fake knows about: the main one, or a linked worktree.
#[derive(Clone, Debug)]
struct Checkout {
    root: PathBuf,
    directories: Directories,
    branch: Option<String>,
}

impl FakeGit {
    pub fn new() -> Self {
        Self::default()
    }

    /// A shareable handle, for the constructors that take one.
    pub fn shared() -> Arc<dyn Git> {
        Arc::new(Self::new())
    }

    /// Register `root` as a main checkout, creating its `.git` directory.
    ///
    /// The counterpart of `git init`, and the same starting point: the
    /// repository exists and has one checkout, whose branch is the initial
    /// one. Unlike `git init` it needs no commit before a worktree can be
    /// added, so tests say what they mean without a fixture commit in front.
    pub fn init(&self, root: &Path) -> Repository {
        std::fs::create_dir_all(root).expect("creating the fake checkout root");
        let root = root.canonicalize().expect("canonicalising the fake root");
        let common_dir = root.join(".git");
        std::fs::create_dir_all(&common_dir).expect("creating the fake .git directory");
        let repository = Repository {
            root: root.clone(),
            common_dir: common_dir.clone(),
        };
        self.checkouts.lock().unwrap().push(Checkout {
            root,
            directories: Directories {
                git_dir: common_dir.clone(),
                common_dir,
            },
            branch: Some("main".to_owned()),
        });
        let initial = self.next_commit();
        self.branches
            .lock()
            .unwrap()
            .insert("main".to_owned(), initial);
        repository
    }

    /// Whether the repository has `branch`, whether or not it is checked out
    /// anywhere. What `git branch --list` would answer.
    pub fn has_branch(&self, branch: &str) -> bool {
        self.branches.lock().unwrap().contains_key(branch)
    }

    /// Move `branch` on to a new commit, as committing on it would, and return
    /// that commit.
    pub fn commit_on(&self, branch: &str) -> String {
        let commit = self.next_commit();
        let mut branches = self.branches.lock().unwrap();
        let head = branches
            .get_mut(branch)
            .expect("committing on a branch the fake does not have");
        *head = commit.clone();
        commit
    }

    /// A commit id no other commit in this fake has, shaped like Git's.
    fn next_commit(&self) -> String {
        let mut named = self.named_commits.lock().unwrap();
        *named += 1;
        format!("{:040x}", *named)
    }

    /// The commit `revision` names: a branch's head, or a commit some branch
    /// is at. The fake knows no other commits, so it can name no others.
    fn resolve(&self, revision: &str) -> Option<String> {
        let branches = self.branches.lock().unwrap();
        let named = revision.strip_prefix("refs/heads/").unwrap_or(revision);
        branches.get(named).cloned().or_else(|| {
            branches
                .values()
                .find(|commit| *commit == revision)
                .cloned()
        })
    }

    /// Point an already-registered checkout at a `git_dir` outside its common
    /// directory, the shape `git init --separate-git-dir` leaves behind.
    pub fn relocate_git_dir(&self, root: &Path, git_dir: &Path) {
        let root = root.canonicalize().expect("canonicalising the fake root");
        let mut checkouts = self.checkouts.lock().unwrap();
        let checkout = checkouts
            .iter_mut()
            .find(|checkout| checkout.root == root)
            .expect("relocating an unregistered checkout");
        checkout.directories.git_dir = git_dir.to_path_buf();
    }

    /// Declare whether the checkout containing `root` has uncommitted work,
    /// the one thing about a working tree this fake cannot infer from layout.
    pub fn set_uncommitted_changes(&self, root: &Path, uncommitted: bool) {
        let root = root.canonicalize().expect("canonicalising the fake root");
        let mut dirty = self.dirty.lock().unwrap();
        let known = dirty.iter().position(|known| *known == root);
        match (uncommitted, known) {
            (true, None) => dirty.push(root),
            (false, Some(index)) => {
                dirty.remove(index);
            }
            _ => {}
        }
    }

    /// The messages of the commits made in the checkout containing `root`,
    /// oldest first.
    pub fn commits(&self, root: &Path) -> Vec<String> {
        let root = root.canonicalize().expect("canonicalising the fake root");
        self.commits
            .lock()
            .unwrap()
            .iter()
            .filter(|(made_in, _)| *made_in == root)
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// The checkout `repository` names, which every worktree operation is run
    /// from.
    fn main_checkout(&self, repository: &Path) -> Result<Checkout> {
        self.containing(
            &repository
                .canonicalize()
                .with_context(|| format!("repository {} must exist", repository.display()))?,
        )
        .with_context(|| format!("{} is not inside a Git repository", repository.display()))
    }

    /// Lay `branch` out as a linked worktree at `path`, without deciding
    /// whether the branch may be checked out there — that is the caller's,
    /// and it is the only thing `git worktree add` does differently with and
    /// without `-b`.
    fn check_out(&self, repository: &Path, branch: &str, path: &Path) -> Result<()> {
        let main = self.main_checkout(repository)?;

        // The layout `git worktree add` leaves behind, so that the readers
        // which parse it rather than ask us see what they would really see.
        let name = path
            .file_name()
            .context("a worktree path must name a directory")?;
        let git_dir = main.directories.common_dir.join("worktrees").join(name);
        std::fs::create_dir_all(&git_dir).context("creating the fake worktree metadata")?;
        std::fs::create_dir_all(path).context("creating the fake worktree checkout")?;
        std::fs::write(
            path.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .context("writing the fake worktree pointer")?;
        std::fs::write(git_dir.join("commondir"), "../..\n")
            .context("writing the fake worktree commondir")?;

        let root = path
            .canonicalize()
            .context("canonicalising the fake worktree")?;
        self.checkouts.lock().unwrap().push(Checkout {
            root,
            directories: Directories {
                git_dir,
                common_dir: main.directories.common_dir,
            },
            branch: Some(branch.to_owned()),
        });
        Ok(())
    }

    /// Create `branch` at `start` and lay it out as a linked worktree at
    /// `path`: what `git worktree add -b` does, with or without a start point.
    fn branch_out(
        &self,
        repository: &Path,
        branch: &str,
        path: &Path,
        start: String,
    ) -> Result<()> {
        if self.has_branch(branch) {
            anyhow::bail!(
                "git could not create the branch and worktree: branch {branch:?} already exists"
            );
        }
        self.check_out(repository, branch, path)?;
        self.branches
            .lock()
            .unwrap()
            .insert(branch.to_owned(), start);
        Ok(())
    }

    /// The checkout whose root is `path` or an ancestor of it, longest root
    /// first so a linked worktree nested under a checkout wins over it.
    fn containing(&self, path: &Path) -> Option<Checkout> {
        let checkouts = self.checkouts.lock().unwrap();
        checkouts
            .iter()
            .filter(|checkout| path.starts_with(&checkout.root))
            .max_by_key(|checkout| checkout.root.components().count())
            .cloned()
    }
}

impl Git for FakeGit {
    fn discover(&self, start: &Path) -> Result<Option<Repository>> {
        let start = start
            .canonicalize()
            .with_context(|| format!("repository search path {} must exist", start.display()))?;
        Ok(self.containing(&start).map(|checkout| Repository {
            root: checkout.root,
            common_dir: checkout.directories.common_dir,
        }))
    }

    fn directories(&self, root: &Path) -> Result<Directories> {
        let root = root
            .canonicalize()
            .with_context(|| format!("checkout {} must exist", root.display()))?;
        self.containing(&root)
            .map(|checkout| checkout.directories)
            .with_context(|| format!("{} is not inside a Git repository", root.display()))
    }

    fn create_worktree(&self, repository: &Path, branch: &str, path: &Path) -> Result<()> {
        let head = self
            .commit(repository, "HEAD")?
            .context("the fake repository's head has no commit to branch from")?;
        self.branch_out(repository, branch, path, head)
    }

    fn fork_worktree(
        &self,
        repository: &Path,
        branch: &str,
        path: &Path,
        start_point: &str,
    ) -> Result<()> {
        let Some(start) = self.resolve(start_point) else {
            anyhow::bail!(
                "git could not create the branch and worktree: invalid reference: {start_point:?}"
            );
        };
        self.branch_out(repository, branch, path, start)
    }

    fn add_worktree(&self, repository: &Path, branch: &str, path: &Path) -> Result<()> {
        if !self.has_branch(branch) {
            anyhow::bail!("git could not check the branch out: branch {branch:?} does not exist");
        }
        if self
            .checkouts
            .lock()
            .unwrap()
            .iter()
            .any(|checkout| checkout.branch.as_deref() == Some(branch))
        {
            anyhow::bail!(
                "git could not check the branch out: branch {branch:?} is already checked out"
            );
        }
        self.check_out(repository, branch, path)
    }

    fn remove_worktree(&self, repository: &Path, path: &Path) -> Result<()> {
        let path = path
            .canonicalize()
            .with_context(|| format!("worktree {} must exist", path.display()))?;
        let main = self.main_checkout(repository)?;
        anyhow::ensure!(
            path != main.root,
            "git could not remove the worktree: {} is a main working tree",
            path.display()
        );
        if self.has_uncommitted_changes(&path)? {
            anyhow::bail!(
                "git could not remove the worktree: {} contains modified or untracked files",
                path.display()
            );
        }
        let mut checkouts = self.checkouts.lock().unwrap();
        let index = checkouts
            .iter()
            .position(|checkout| checkout.root == path)
            .with_context(|| format!("{} is not a working tree", path.display()))?;
        // The branch deliberately stays in `branches`: removing a worktree
        // takes the checkout away and leaves the branch behind.
        let removed = checkouts.remove(index);
        drop(checkouts);
        std::fs::remove_dir_all(&path).context("removing the fake worktree checkout")?;
        std::fs::remove_dir_all(&removed.directories.git_dir)
            .context("removing the fake worktree metadata")?;
        Ok(())
    }

    fn current_branch(&self, checkout: &Path) -> Result<Option<String>> {
        let checkout = checkout
            .canonicalize()
            .with_context(|| format!("checkout {} must exist", checkout.display()))?;
        Ok(self.containing(&checkout).and_then(|found| found.branch))
    }

    fn commit(&self, checkout: &Path, revision: &str) -> Result<Option<String>> {
        if revision != "HEAD" {
            return Ok(self.resolve(revision));
        }
        Ok(self
            .current_branch(checkout)?
            .and_then(|branch| self.resolve(&branch)))
    }

    fn has_uncommitted_changes(&self, checkout: &Path) -> Result<bool> {
        let checkout = checkout
            .canonicalize()
            .with_context(|| format!("checkout {} must exist", checkout.display()))?;
        let Some(found) = self.containing(&checkout) else {
            anyhow::bail!("{} is not inside a Git repository", checkout.display());
        };
        Ok(self.dirty.lock().unwrap().contains(&found.root))
    }

    fn commit_all(&self, checkout: &Path, message: &str) -> Result<String> {
        if !self.has_uncommitted_changes(checkout)? {
            anyhow::bail!("git could not commit the turn: nothing to commit, working tree clean");
        }
        let checkout = checkout.canonicalize()?;
        let root = self
            .containing(&checkout)
            .expect("checked to be in a checkout above")
            .root;
        self.set_uncommitted_changes(&root, false);
        let mut commits = self.commits.lock().unwrap();
        commits.push((root, message.to_owned()));
        Ok(format!("{:07x}", commits.len()))
    }

    /// The fake keeps no file contents, so a dirty checkout's diff only names
    /// the checkout it was taken in.
    fn working_tree_diff(&self, checkout: &Path) -> Result<Vec<u8>> {
        if !self.has_uncommitted_changes(checkout)? {
            return Ok(Vec::new());
        }
        let checkout = checkout.canonicalize()?;
        Ok(format!("fake diff of {}\n", checkout.display()).into_bytes())
    }

    /// Named after what went into it, so two snapshots of the same state
    /// share a tree, as they would in Git.
    fn snapshot_tree(
        &self,
        _checkout: &Path,
        commit: Option<&str>,
        diff: Option<&[u8]>,
    ) -> Result<String> {
        Ok(format!(
            "tree({}+{})",
            commit.unwrap_or("empty"),
            diff.map(String::from_utf8_lossy)
                .unwrap_or_default()
                .trim_end()
        ))
    }

    fn tree_diff(&self, _checkout: &Path, from: &str, to: &str) -> Result<String> {
        Ok(if from == to {
            String::new()
        } else {
            format!("fake diff from {from} to {to}\n")
        })
    }
}

/// The nearest enclosing directory of `start` that holds a `.git`. Inside a
/// worktree `.git` is a file rather than a directory, so the test is existence
/// and not kind.
pub fn enclosing_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|directory| directory.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The directories holding the history of a checkout whose root is `root`, for
/// the case where the root alone does not hold it.
///
/// In an ordinary checkout `.git` is a directory inside the root and this is
/// empty. In a linked worktree `.git` is instead a file naming a directory
/// under the main checkout, which in turn names the common directory that
/// carries the objects and refs — both live outside the worktree, so both have
/// to be mounted for history to be readable at all.
pub fn history_directories(root: &Path) -> Vec<PathBuf> {
    let pointer = root.join(".git");
    if pointer.is_dir() {
        return Vec::new();
    }
    let Some(git_directory) = std::fs::read_to_string(&pointer)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("gitdir:")
                    .map(|target| target.trim().to_owned())
            })
        })
        .map(|target| resolve_against(root, Path::new(&target)))
    else {
        return Vec::new();
    };
    let common = std::fs::read_to_string(git_directory.join("commondir"))
        .ok()
        .map(|contents| resolve_against(&git_directory, Path::new(contents.trim())));
    let mut directories = vec![git_directory];
    if let Some(common) = common {
        if !directories.contains(&common) {
            directories.push(common);
        }
    }
    directories
}

/// Interpret a path a Git pointer file gave us, which may be relative to the
/// file that named it. Canonicalized when the target exists so that the `..`
/// segments Git writes do not reach a launch policy as-is.
fn resolve_against(base: &Path, target: &Path) -> PathBuf {
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        base.join(target)
    };
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// One `git` invocation, run with `-C directory`.
///
/// `action` names what the command is for, in the infinitive: it appears both
/// in the context of a spawn failure ("running git to `action`") and in the
/// error a non-zero exit produces ("git could not `action`"), so every failure
/// mode of every call reads the same way without each call spelling it out.
struct Invocation<'a> {
    directory: &'a Path,
    action: String,
    arguments: Vec<OsString>,
    stdin: Option<Vec<u8>>,
    env: Vec<(OsString, OsString)>,
}

impl<'a> Invocation<'a> {
    fn new(directory: &'a Path, action: impl Into<String>) -> Self {
        Self {
            directory,
            action: action.into(),
            arguments: Vec::new(),
            stdin: None,
            env: Vec::new(),
        }
    }

    /// Set an environment variable for the command.
    fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Feed `input` to the command's standard input.
    fn stdin(mut self, input: impl AsRef<[u8]>) -> Self {
        self.stdin = Some(input.as_ref().to_vec());
        self
    }

    fn arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.arguments.push(argument.as_ref().to_owned());
        self
    }

    fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_owned()),
        );
        self
    }

    /// Run the command, requiring success. Returns stdout without the trailing
    /// newline Git writes.
    fn output(self) -> Result<String> {
        // `run` only withholds stdout when a failure is allowed to pass, which
        // this call does not permit.
        Ok(self.run(true)?.unwrap_or_default())
    }

    /// Run the command, mapping a non-zero exit to `None`. Only for questions
    /// where failing is itself an answer — a directory that is not a
    /// repository, a name that is not a valid ref.
    fn optional_output(self) -> Result<Option<String>> {
        self.run(false)
    }

    /// Run the command for its effect, discarding stdout.
    fn succeed(self) -> Result<()> {
        self.run(true).map(|_| ())
    }

    /// Run the command, requiring success, and return stdout exactly as Git
    /// wrote it — for output that is file contents rather than an answer, and
    /// need not be UTF-8.
    fn bytes(self) -> Result<Vec<u8>> {
        Ok(self.run_raw(true)?.unwrap_or_default())
    }

    fn run(self, require_success: bool) -> Result<Option<String>> {
        let action = self.action.clone();
        let Some(stdout) = self.run_raw(require_success)? else {
            return Ok(None);
        };
        let stdout = String::from_utf8(stdout)
            .with_context(|| format!("git returned non-UTF-8 output when asked to {action}"))?;
        Ok(Some(stdout.trim_end_matches('\n').to_owned()))
    }

    fn run_raw(self, require_success: bool) -> Result<Option<Vec<u8>>> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(self.directory)
            .args(&self.arguments)
            .envs(self.env.iter().map(|(key, value)| (key, value)));
        let output = match &self.stdin {
            None => command.output(),
            Some(input) => feed(&mut command, input),
        }
        .with_context(|| format!("running git to {}", self.action))?;
        if output.status.success() {
            return Ok(Some(output.stdout));
        }
        if !require_success {
            return Ok(None);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        anyhow::bail!("git could not {}: {detail}", self.action)
    }
}

/// Run `command` with `input` on its standard input, collecting its output.
fn feed(command: &mut Command, input: &[u8]) -> std::io::Result<std::process::Output> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Dropped before waiting, so git sees the end of the message.
    let written = child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input);
    let output = child.wait_with_output()?;
    // A git that refused before reading all of it says why on stderr, which
    // is the better error than the broken pipe that refusal caused here.
    if output.status.success() {
        written?;
    }
    Ok(output)
}

/// Repository setup the real-Git conformance tests need. Here rather than in
/// each test module so that these invocations, too, are built in one place.
#[cfg(test)]
pub(crate) mod fixture {
    use super::Invocation;
    use std::path::Path;

    /// Whether a usable `git` is on this host, for the conformance tests that
    /// need one. They are `#[ignore]`d, so this only guards a deliberate run.
    pub fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Create an empty repository in `directory`, which must exist.
    pub fn init(directory: &Path) {
        Invocation::new(directory, "initialise a test repository")
            .args(["init", "--quiet"])
            .succeed()
            .unwrap();
    }

    /// Give the repository at `checkout` an identity of its own, for a test
    /// that commits through code which passes none.
    pub fn identify(checkout: &Path) {
        for (key, value) in [
            ("user.name", "Styra"),
            ("user.email", "styra@example.invalid"),
        ] {
            Invocation::new(checkout, "configure a test identity")
                .args(["config", key, value])
                .succeed()
                .unwrap();
        }
    }

    /// Commit with no changes, and without depending on whether whoever runs
    /// the tests has a Git identity configured.
    pub fn commit_empty(checkout: &Path, message: &str) {
        Invocation::new(checkout, "commit in a test repository")
            .args([
                "-c",
                "user.name=Styra",
                "-c",
                "user.email=styra@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                message,
            ])
            .succeed()
            .unwrap();
    }
}

#[cfg(test)]
pub(crate) fn temporary_directory(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "styra-git-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ))
}

/// What Styra decides from Git's answers, decided against [`FakeGit`].
///
/// Nothing here runs a process. The mount set a launch needs, and the rule
/// that a path belongs to its nearest enclosing checkout, are Styra's own
/// logic and are tested as such.
#[cfg(test)]
mod tests;

/// [`SystemGit`] against the real `git`, which is the only thing that can say
/// whether Styra's command lines and output parsing still match it.
///
/// `#[ignore]`d: the default suite must not need a `git` binary, and these
/// prove nothing about Styra's own logic — [`tests`] above does that against
/// [`FakeGit`]. Run them when the command lines in [`SystemGit`] change:
///
/// ```text
/// cargo test -p server --lib conformance -- --ignored
/// ```
#[cfg(test)]
mod conformance;
