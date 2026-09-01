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

use crate::agent::MountSpec;
use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The checkout and shared metadata Git associates with a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    /// Root of the checkout containing the directory passed to [`discover`].
    pub root: PathBuf,
    /// Git's common metadata directory. For a normal checkout this is `.git`;
    /// for a linked worktree it is the main checkout's shared `.git` directory.
    pub common_dir: PathBuf,
}

/// Discover the Git checkout containing `start`.
///
/// `Ok(None)` means the path is not inside a working tree. Other failures — an
/// unreadable path, a missing Git executable, or malformed successful output —
/// remain errors so callers do not silently discard a repository they should
/// have mounted.
pub fn discover(start: &Path) -> Result<Option<Repository>> {
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

/// Resolve `path` to the root of its nearest enclosing Git checkout.
pub fn repository_root(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Git repository path {} must exist", path.display()))?;
    git_path(&path, ["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not inside a Git repository", path.display()))
}

/// Mandatory mounts for a Workspace's associated repository.
pub fn mounts(root: &Path) -> Result<Vec<MountSpec>> {
    let root = repository_root(root)?;
    let git_dir =
        git_path(&root, ["rev-parse", "--absolute-git-dir"]).context("resolving Git directory")?;
    let common_dir = git_path(
        &root,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .context("resolving Git common directory")?;

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

/// Whether Git would accept `name` as a branch name, asked of `repository` so
/// that the answer is the one that repository's configuration would give.
pub fn branch_name_is_valid(repository: &Path, name: &str) -> Result<bool> {
    let checked = Invocation::new(repository, "validate the branch name")
        .args(["check-ref-format", "--branch"])
        .arg(name)
        .optional_output()?;
    Ok(checked.as_deref() == Some(name))
}

/// Create `branch` as a new branch, checked out in a linked worktree at
/// `path`.
pub fn create_worktree(repository: &Path, branch: &str, path: &Path) -> Result<()> {
    Invocation::new(repository, "create the branch and worktree")
        .args(["worktree", "add", "-b"])
        .arg(branch)
        .arg("--")
        .arg(path)
        .succeed()
}

/// The branch checked out in `checkout`, or `None` when its head is detached.
pub fn current_branch(checkout: &Path) -> Result<Option<String>> {
    let branch = Invocation::new(checkout, "read the current branch")
        .args(["branch", "--show-current"])
        .output()?;
    Ok(Some(branch).filter(|branch| !branch.is_empty()))
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

/// Repository setup the crate's own tests need. Here rather than in each test
/// module so that these invocations, too, are built in one place.
#[cfg(test)]
pub(crate) mod fixture {
    use super::Invocation;
    use std::path::Path;

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

    /// Check the current head out again in a linked worktree at `path`.
    pub fn add_detached_worktree(repository: &Path, path: &Path) {
        Invocation::new(repository, "add a test worktree")
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(path)
            .succeed()
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "styra-git-{tag}-{}-{}",
            std::process::id(),
            crate::journal::now_ms()
        ))
    }

    #[test]
    fn finds_the_checkout_from_a_nested_directory() {
        let root = temporary_directory("nested");
        let nested = root.join("one/two");
        std::fs::create_dir_all(&nested).unwrap();
        fixture::init(&root);

        let repository = discover(&nested).unwrap().unwrap();
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

        assert_eq!(discover(&root).unwrap(), None);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn linked_worktrees_report_the_shared_common_directory() {
        let root = temporary_directory("linked");
        let checkout = root.join("checkout");
        let linked = root.join("linked");
        std::fs::create_dir_all(&checkout).unwrap();
        fixture::init(&checkout);
        fixture::commit_empty(&checkout, "initial");
        fixture::add_detached_worktree(&checkout, &linked);

        let repository = discover(&linked).unwrap().unwrap();
        assert_eq!(repository.root, linked.canonicalize().unwrap());
        assert_eq!(
            repository.common_dir,
            checkout.join(".git").canonicalize().unwrap()
        );
        // The two mechanisms answer the same question for different callers,
        // so what the layout reader reports has to be what `git` reports.
        assert_eq!(enclosing_root(&linked.join("nested")), Some(linked.clone()));
        assert!(history_directories(&linked).contains(&repository.common_dir));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_enclosing_root_is_the_nearest_directory_holding_a_git() {
        let base = temporary_directory("enclosing");
        let root = base.join("checkout");
        let nested = root.join("one/two");
        std::fs::create_dir_all(nested.join(".git-not-this-one")).unwrap();
        fixture::init(&root);

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
}
