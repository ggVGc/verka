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
//! Running `git` is a seam, not a fact: [`Git`] names the four questions that
//! need a process, [`SystemGit`] answers them by spawning one, and [`FakeGit`]
//! answers them from an in-memory model. Everything derived from those answers
//! — which root a path belongs to, which directories a launch must mount — is
//! a default method on the trait, so the interesting logic is written once and
//! tested without a `git` binary anywhere.

use crate::agent::MountSpec;
use anyhow::{Context, Result};
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
/// Deliberately four methods and no more. Each one is a single `git`
/// invocation whose output Styra parses; everything Styra *decides* from those
/// answers is a default method below, so a test that cares about the deciding
/// — which is nearly all of them — runs against [`FakeGit`] and never spawns a
/// process. [`SystemGit`] is the only implementation that does.
pub trait Git: Send + Sync {
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

    /// The branch checked out in `checkout`, or `None` when its head is
    /// detached.
    fn current_branch(&self, checkout: &Path) -> Result<Option<String>>;

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

    fn current_branch(&self, checkout: &Path) -> Result<Option<String>> {
        let branch = Invocation::new(checkout, "read the current branch")
            .args(["branch", "--show-current"])
            .output()?;
        Ok(Some(branch).filter(|branch| !branch.is_empty()))
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
        repository
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
        let main = self
            .containing(
                &repository
                    .canonicalize()
                    .with_context(|| format!("repository {} must exist", repository.display()))?,
            )
            .with_context(|| format!("{} is not inside a Git repository", repository.display()))?;
        if self
            .checkouts
            .lock()
            .unwrap()
            .iter()
            .any(|checkout| checkout.branch.as_deref() == Some(branch))
        {
            anyhow::bail!(
                "git could not create the branch and worktree: branch {branch:?} already exists"
            );
        }

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

    fn current_branch(&self, checkout: &Path) -> Result<Option<String>> {
        let checkout = checkout
            .canonicalize()
            .with_context(|| format!("checkout {} must exist", checkout.display()))?;
        Ok(self.containing(&checkout).and_then(|found| found.branch))
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
}

impl<'a> Invocation<'a> {
    fn new(directory: &'a Path, action: impl Into<String>) -> Self {
        Self {
            directory,
            action: action.into(),
            arguments: Vec::new(),
        }
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

    fn run(self, require_success: bool) -> Result<Option<String>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.directory)
            .args(&self.arguments)
            .output()
            .with_context(|| format!("running git to {}", self.action))?;
        if output.status.success() {
            let stdout = String::from_utf8(output.stdout).with_context(|| {
                format!(
                    "git returned non-UTF-8 output when asked to {}",
                    self.action
                )
            })?;
            return Ok(Some(stdout.trim_end_matches('\n').to_owned()));
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
        crate::journal::now_ms()
    ))
}

/// What Styra decides from Git's answers, decided against [`FakeGit`].
///
/// Nothing here runs a process. The mount set a launch needs, and the rule
/// that a path belongs to its nearest enclosing checkout, are Styra's own
/// logic and are tested as such.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_checkout_from_a_nested_directory() {
        let root = temporary_directory("nested");
        let nested = root.join("one/two");
        std::fs::create_dir_all(&nested).unwrap();
        let git = FakeGit::new();
        git.init(&root);

        let repository = git.discover(&nested).unwrap().unwrap();
        assert_eq!(repository.root, root.canonicalize().unwrap());
        assert_eq!(
            repository.common_dir,
            root.join(".git").canonicalize().unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_directory_outside_git_is_not_a_repository() {
        let root = temporary_directory("plain");
        std::fs::create_dir_all(&root).unwrap();
        let git = FakeGit::new();

        assert_eq!(git.discover(&root).unwrap(), None);
        let error = git.repository_root(&root).unwrap_err().to_string();
        assert!(error.contains("not inside a Git repository"), "{error}");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linked_worktrees_report_the_shared_common_directory() {
        let root = temporary_directory("linked");
        let checkout = root.join("checkout");
        let linked = root.join("linked");
        let git = FakeGit::new();
        git.init(&checkout);
        git.create_worktree(&checkout, "feature", &linked).unwrap();

        let repository = git.discover(&linked).unwrap().unwrap();
        assert_eq!(repository.root, linked.canonicalize().unwrap());
        assert_eq!(
            repository.common_dir,
            checkout.join(".git").canonicalize().unwrap()
        );
        // The two mechanisms answer the same question for different callers,
        // so what the layout reader reports has to be what the Git seam
        // reports — which is why the fake writes the layout it describes.
        assert_eq!(
            enclosing_root(&linked.join("nested")),
            Some(linked.canonicalize().unwrap())
        );
        assert!(history_directories(&linked).contains(&repository.common_dir));

        std::fs::remove_dir_all(root).unwrap();
    }

    /// An ordinary checkout mounts its root read-only and the one directory
    /// holding its history writable.
    #[test]
    fn a_plain_checkout_mounts_its_root_and_its_history() {
        let root = temporary_directory("mounts-plain");
        let git = FakeGit::new();
        let repository = git.init(&root);

        let mounts = git.mounts(&root).unwrap();

        let writable: Vec<_> = mounts
            .iter()
            .filter(|mount| mount.writable)
            .map(|mount| mount.destination.clone())
            .collect();
        assert_eq!(writable, vec![repository.common_dir.clone()]);
        assert!(mounts
            .iter()
            .any(|mount| mount.destination == repository.root && !mount.writable));
        // Every mount is its own source: these are host paths bound in place.
        assert!(mounts.iter().all(|mount| mount.source == mount.destination));

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A linked worktree keeps its index and HEAD in a per-worktree directory
    /// and its objects in the shared one. Both have to arrive writable, or the
    /// agent gets a checkout it cannot commit in — but they arrive as *one*
    /// mount, because Git nests the first inside the second. What the test
    /// pins is the reachability, not the number of mounts that deliver it.
    #[test]
    fn a_linked_worktree_can_write_both_its_own_metadata_and_the_shared_history() {
        let base = temporary_directory("mounts-linked");
        let checkout = base.join("checkout");
        let linked = base.join("linked");
        let git = FakeGit::new();
        let main = git.init(&checkout);
        git.create_worktree(&checkout, "feature", &linked).unwrap();
        let worktree_git = git.directories(&linked).unwrap().git_dir;

        let mounts = git.mounts(&linked).unwrap();

        let writable_cover = |path: &Path| {
            mounts
                .iter()
                .any(|mount| mount.writable && path.starts_with(&mount.destination))
        };
        assert!(
            writable_cover(&main.common_dir),
            "the shared history must be writable: {mounts:?}"
        );
        assert!(
            writable_cover(&worktree_git),
            "the worktree's own metadata must be writable: {mounts:?}"
        );
        // The checkout itself is not: the agent edits files through the
        // Workspace mount, and nothing should be able to rewrite it here.
        assert!(mounts
            .iter()
            .any(|mount| mount.destination == linked.canonicalize().unwrap() && !mount.writable));

        std::fs::remove_dir_all(base).unwrap();
    }

    /// The other shape: a `git_dir` that is not inside the common directory,
    /// which is the case the mount set has an explicit branch for. Nesting is
    /// what makes the linked-worktree case one mount; without it, both
    /// directories have to be named.
    #[test]
    fn a_git_dir_outside_the_common_directory_is_mounted_in_its_own_right() {
        let base = temporary_directory("mounts-separate");
        let root = base.join("checkout");
        let git_dir = base.join("elsewhere/git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let git = FakeGit::new();
        git.init(&root);
        let common_dir = root.join(".git").canonicalize().unwrap();
        git.relocate_git_dir(&root, &git_dir.canonicalize().unwrap());

        let mounts = git.mounts(&root).unwrap();

        let writable: Vec<_> = mounts
            .iter()
            .filter(|mount| mount.writable)
            .map(|mount| mount.destination.clone())
            .collect();
        assert!(writable.contains(&common_dir), "{writable:?}");
        assert!(
            writable.contains(&git_dir.canonicalize().unwrap()),
            "{writable:?}"
        );

        std::fs::remove_dir_all(base).unwrap();
    }

    /// One directory, one mount. A path reached twice — the common directory's
    /// parent is also the checkout root — must not be bound twice, and the
    /// writable claim must win.
    #[test]
    fn a_directory_reached_twice_is_mounted_once_and_writably() {
        let root = temporary_directory("mounts-once");
        let git = FakeGit::new();
        let repository = git.init(&root);

        let mounts = git.mounts(&root).unwrap();

        let mut destinations: Vec<_> = mounts.iter().map(|mount| &mount.destination).collect();
        let total = destinations.len();
        destinations.sort();
        destinations.dedup();
        assert_eq!(destinations.len(), total, "duplicate mount: {mounts:?}");
        assert!(mounts
            .iter()
            .any(|mount| mount.destination == repository.common_dir && mount.writable));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_enclosing_root_is_the_nearest_directory_holding_a_git() {
        let base = temporary_directory("enclosing");
        let root = base.join("checkout");
        let nested = root.join("one/two");
        std::fs::create_dir_all(nested.join(".git-not-this-one")).unwrap();
        let git = FakeGit::new();
        git.init(&root);

        assert_eq!(enclosing_root(&nested), Some(root));
        // A sibling outside the checkout does not borrow its root. Whether
        // anything above the temporary directory is itself a checkout is not
        // this test's business, so the claim is only about the tree it built.
        let outside = base.join("plain/deeper");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(!matches!(enclosing_root(&outside), Some(found) if found.starts_with(&base)));

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_worktree_contributes_the_directories_holding_its_history() {
        let base = temporary_directory("history");
        let main = base.join("main");
        let worktree_git = main.join(".git/worktrees/feature");
        let worktree = base.join("feature");
        std::fs::create_dir_all(&worktree_git).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../main/.git/worktrees/feature\n",
        )
        .unwrap();
        std::fs::write(worktree_git.join("commondir"), "../..\n").unwrap();

        let directories = history_directories(&worktree);

        assert_eq!(
            directories,
            vec![
                std::fs::canonicalize(&worktree_git).unwrap(),
                std::fs::canonicalize(main.join(".git")).unwrap(),
            ]
        );
        assert!(history_directories(&main).is_empty());
        std::fs::remove_dir_all(base).unwrap();
    }

    /// The fake refuses what Git refuses, so a caller cannot pass a test by
    /// doing something the real thing would reject.
    #[test]
    fn the_fake_refuses_a_branch_that_already_exists() {
        let base = temporary_directory("duplicate-branch");
        let checkout = base.join("checkout");
        let git = FakeGit::new();
        git.init(&checkout);
        git.create_worktree(&checkout, "feature", &base.join("one"))
            .unwrap();

        let error = git
            .create_worktree(&checkout, "feature", &base.join("two"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("already exists"), "{error}");
        std::fs::remove_dir_all(base).unwrap();
    }
}

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
mod conformance {
    use super::*;

    /// Both implementations must answer a nested lookup the same way.
    #[test]
    #[ignore = "requires a real git binary"]
    fn real_git_describes_a_plain_checkout_as_the_fake_does() {
        if !fixture::git_available() {
            eprintln!("skipping: no usable git");
            return;
        }
        let root = temporary_directory("conformance-plain");
        let nested = root.join("one/two");
        std::fs::create_dir_all(&nested).unwrap();
        fixture::init(&root);

        let real = SystemGit.discover(&nested).unwrap().unwrap();
        assert_eq!(real.root, root.canonicalize().unwrap());
        assert_eq!(real.common_dir, root.join(".git").canonicalize().unwrap());
        assert_eq!(
            SystemGit.directories(&real.root).unwrap(),
            Directories {
                git_dir: real.common_dir.clone(),
                common_dir: real.common_dir.clone(),
            }
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires a real git binary"]
    fn real_git_is_silent_about_a_directory_outside_a_repository() {
        if !fixture::git_available() {
            eprintln!("skipping: no usable git");
            return;
        }
        let root = temporary_directory("conformance-outside");
        std::fs::create_dir_all(&root).unwrap();

        assert_eq!(SystemGit.discover(&root).unwrap(), None);

        std::fs::remove_dir_all(root).unwrap();
    }

    /// The shape [`FakeGit::create_worktree`] writes is the shape `git
    /// worktree add` writes: a `.git` file, a per-worktree metadata directory
    /// under the main checkout, and a shared common directory. If this drifts,
    /// every test built on the fake is testing the wrong layout.
    #[test]
    #[ignore = "requires a real git binary"]
    fn real_git_lays_a_worktree_out_the_way_the_fake_does() {
        if !fixture::git_available() {
            eprintln!("skipping: no usable git");
            return;
        }
        let base = temporary_directory("conformance-worktree");
        let checkout = base.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        fixture::init(&checkout);
        fixture::commit_empty(&checkout, "initial");
        let real_linked = base.join("real");
        SystemGit
            .create_worktree(&checkout, "styra/conformance", &real_linked)
            .unwrap();

        let fake = FakeGit::new();
        let fake_checkout = base.join("fake-checkout");
        fake.init(&fake_checkout);
        let fake_linked = base.join("fake");
        fake.create_worktree(&fake_checkout, "styra/conformance", &fake_linked)
            .unwrap();

        // A pointer file, not a directory, in both.
        assert!(real_linked.join(".git").is_file());
        assert!(fake_linked.join(".git").is_file());
        // The metadata directory sits under the main checkout's .git in both.
        let real_dirs = SystemGit.directories(&real_linked).unwrap();
        let fake_dirs = fake.directories(&fake_linked).unwrap();
        assert!(real_dirs
            .git_dir
            .starts_with(checkout.canonicalize().unwrap().join(".git")));
        assert!(fake_dirs
            .git_dir
            .starts_with(fake_checkout.canonicalize().unwrap().join(".git")));
        assert_ne!(real_dirs.git_dir, real_dirs.common_dir);
        assert_ne!(fake_dirs.git_dir, fake_dirs.common_dir);
        // And the layout reader, which parses those files rather than asking,
        // finds the common directory through both.
        assert!(history_directories(&real_linked).contains(&real_dirs.common_dir));
        assert!(history_directories(&fake_linked).contains(&fake_dirs.common_dir));
        // The branch reads back in both.
        assert_eq!(
            SystemGit.current_branch(&real_linked).unwrap().as_deref(),
            Some("styra/conformance")
        );
        assert_eq!(
            fake.current_branch(&fake_linked).unwrap().as_deref(),
            Some("styra/conformance")
        );

        std::fs::remove_dir_all(base).unwrap();
    }

    /// The mount set is derived logic, so it must come out the same whichever
    /// implementation answered the questions underneath it.
    #[test]
    #[ignore = "requires a real git binary"]
    fn both_implementations_derive_the_same_shape_of_mount_set() {
        if !fixture::git_available() {
            eprintln!("skipping: no usable git");
            return;
        }
        let base = temporary_directory("conformance-mounts");
        let checkout = base.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        fixture::init(&checkout);

        let fake = FakeGit::new();
        let fake_checkout = base.join("fake-checkout");
        fake.init(&fake_checkout);

        let shape = |mounts: Vec<MountSpec>, root: &Path| {
            let root = root.canonicalize().unwrap();
            let mut relative: Vec<_> = mounts
                .iter()
                .map(|mount| {
                    (
                        mount
                            .destination
                            .strip_prefix(&root)
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|_| mount.destination.display().to_string()),
                        mount.writable,
                    )
                })
                .collect();
            relative.sort();
            relative
        };

        assert_eq!(
            shape(SystemGit.mounts(&checkout).unwrap(), &checkout),
            shape(fake.mounts(&fake_checkout).unwrap(), &fake_checkout)
        );

        std::fs::remove_dir_all(base).unwrap();
    }
}
