//! Every Git operation Nota performs.
//!
//! Review logic asks [`Git`] questions and never builds a `git` process
//! itself. [`SystemGit`] answers them by running `git`; tests can supply an
//! in-memory implementation instead.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The Git operations Nota needs. `repository` is always a root returned by
/// [`Git::repository_root`], and commits are full object ids.
pub trait Git {
    /// The root of the working tree containing `path`.
    fn repository_root(&self, path: &Path) -> Result<PathBuf>;

    /// The full id of the commit `revision` names.
    fn resolve_commit(&self, repository: &Path, revision: &str) -> Result<String>;

    /// The checked-out branch, or `None` when `HEAD` is detached.
    fn current_branch(&self, repository: &Path) -> Result<Option<String>>;

    /// Reject `branch` unless Git accepts it as a branch name.
    fn validate_branch_name(&self, repository: &Path, branch: &str) -> Result<()>;

    fn branch_exists(&self, repository: &Path, branch: &str) -> Result<bool>;

    /// The commit `branch` points at. Unlike [`Git::resolve_commit`], a tag or
    /// other ref with the same name is never chosen instead.
    fn branch_tip(&self, repository: &Path, branch: &str) -> Result<String>;

    /// Local branch names and their captured tip object ids. Excludes remote
    /// tracking refs and tags, even when a short name would be ambiguous.
    fn local_branches(&self, repository: &Path) -> Result<Vec<(String, String)>>;

    /// Create `branch` at `commit` without checking it out. Fails if the
    /// branch already exists.
    fn create_branch(&self, repository: &Path, branch: &str, commit: &str) -> Result<()>;

    /// Create a commit whose only parent is `parent` and whose tree is the
    /// parent's, without changing any ref, the index, or the checkout.
    fn commit_empty(&self, repository: &Path, parent: &str, message: &str) -> Result<String>;

    /// Move existing `branch` from `expected` to `commit`, failing if it no
    /// longer points at `expected`.
    fn update_branch(
        &self,
        repository: &Path,
        branch: &str,
        commit: &str,
        expected: &str,
    ) -> Result<()>;

    /// `revision` and its first-parent ancestors, newest first.
    fn first_parent_history(&self, repository: &Path, revision: &str) -> Result<Vec<String>>;

    /// Commits on the first-parent histories of `tips` whose message has a
    /// line starting with `<key>:` for any of `keys`, in no particular order.
    /// Such a line need not be a trailer, so callers must still parse them.
    fn first_parent_commits_with_trailers(
        &self,
        repository: &Path,
        tips: &[String],
        keys: &[&str],
    ) -> Result<Vec<Commit>>;

    /// Read `commits`, in the order given.
    fn commits(&self, repository: &Path, commits: &[String]) -> Result<Vec<Commit>>;
}

/// A commit as Nota reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    /// `None` for a root commit.
    pub first_parent: Option<String>,
    /// The full message, without surrounding whitespace.
    pub message: String,
    /// The paths a non-merge commit changes relative to its parent.
    pub paths: Vec<String>,
}

/// [`Git`] answered by running the `git` executable.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGit;

impl Git for SystemGit {
    fn repository_root(&self, path: &Path) -> Result<PathBuf> {
        let root = checked(path, &["rev-parse", "--show-toplevel"])
            .with_context(|| format!("{} is not inside a Git repository", path.display()))?;
        Ok(root.into())
    }

    fn resolve_commit(&self, repository: &Path, revision: &str) -> Result<String> {
        checked(
            repository,
            &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
        )
        .with_context(|| format!("resolving Git revision `{revision}`"))
    }

    fn current_branch(&self, repository: &Path) -> Result<Option<String>> {
        let args = ["symbolic-ref", "--quiet", "--short", "HEAD"];
        // `--quiet` makes a detached HEAD exit 1 silently; anything else is
        // a real failure.
        if output(repository, &args)?.status.code() == Some(1) {
            return Ok(None);
        }
        checked(repository, &args).map(Some)
    }

    fn validate_branch_name(&self, repository: &Path, branch: &str) -> Result<()> {
        checked(repository, &["check-ref-format", "--branch", branch]).map(drop)
    }

    fn branch_exists(&self, repository: &Path, branch: &str) -> Result<bool> {
        let refname = format!("refs/heads/{branch}");
        Ok(
            output(repository, &["show-ref", "--verify", "--quiet", &refname])?
                .status
                .success(),
        )
    }

    fn branch_tip(&self, repository: &Path, branch: &str) -> Result<String> {
        self.resolve_commit(repository, &format!("refs/heads/{branch}"))
    }

    fn local_branches(&self, repository: &Path) -> Result<Vec<(String, String)>> {
        let refs = checked(
            repository,
            &[
                "for-each-ref",
                "--format=%(refname)%09%(objectname)",
                "refs/heads/",
            ],
        )?;
        refs.lines()
            .map(|line| {
                let (name, tip) = line.split_once('\t').context("invalid branch listing")?;
                let branch = name
                    .strip_prefix("refs/heads/")
                    .context("invalid local ref")?;
                Ok((branch.to_string(), tip.to_string()))
            })
            .collect()
    }

    fn create_branch(&self, repository: &Path, branch: &str, commit: &str) -> Result<()> {
        let refname = format!("refs/heads/{branch}");
        checked(repository, &["update-ref", &refname, commit, ""]).map(drop)
    }

    fn commit_empty(&self, repository: &Path, parent: &str, message: &str) -> Result<String> {
        let tree = checked(repository, &["rev-parse", &format!("{parent}^{{tree}}")])?;
        checked_with_input(
            repository,
            &["commit-tree", &tree, "-p", parent, "-F", "-"],
            message,
        )
    }

    fn update_branch(
        &self,
        repository: &Path,
        branch: &str,
        commit: &str,
        expected: &str,
    ) -> Result<()> {
        let refname = format!("refs/heads/{branch}");
        checked(repository, &["update-ref", &refname, commit, expected]).map(drop)
    }

    fn first_parent_history(&self, repository: &Path, revision: &str) -> Result<Vec<String>> {
        Ok(lines(&checked(
            repository,
            &["rev-list", "--first-parent", revision],
        )?))
    }

    fn first_parent_commits_with_trailers(
        &self,
        repository: &Path,
        tips: &[String],
        keys: &[&str],
    ) -> Result<Vec<Commit>> {
        let mut args = vec!["--first-parent".to_string()];
        args.extend(keys.iter().map(|key| format!("--grep=^{key}:")));
        log(repository, &args, tips)
    }

    fn commits(&self, repository: &Path, commits: &[String]) -> Result<Vec<Commit>> {
        log(repository, &["--no-walk=unsorted".to_string()], commits)
    }
}

/// One `git log` over `revisions`, which are passed on stdin so their number
/// is not limited by the command line.
fn log(repository: &Path, args: &[String], revisions: &[String]) -> Result<Vec<Commit>> {
    // Without revisions, `git log` would read `HEAD`.
    if revisions.is_empty() {
        return Ok(Vec::new());
    }
    // Each record is `RS id NUL parents NUL message NUL`, followed by the
    // changed paths, which `--name-only` prints after the format.
    let mut command = vec![
        "-c",
        "log.showSignature=false",
        "log",
        "--no-renames",
        "--name-only",
        "--format=%x1e%H%x00%P%x00%B%x00",
    ];
    command.extend(args.iter().map(String::as_str));
    command.push("--stdin");
    let output = checked_with_input(repository, &command, &(revisions.join("\n") + "\n"))?;
    output
        .split('\x1e')
        .skip(1)
        .map(|record| {
            let mut fields = record.splitn(4, '\0');
            let (Some(id), Some(parents), Some(message), Some(paths)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                bail!("invalid `git log` output in {}", repository.display());
            };
            Ok(Commit {
                id: id.to_string(),
                first_parent: parents.split_whitespace().next().map(str::to_string),
                message: message.trim().to_string(),
                paths: lines(paths),
            })
        })
        .collect()
}

fn lines(value: &str) -> Vec<String> {
    value
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn output(repository: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "running `git {}` in {}",
                args.join(" "),
                repository.display()
            )
        })
}

fn checked(repository: &Path, args: &[&str]) -> Result<String> {
    let result = output(repository, args)?;
    if !result.status.success() {
        bail!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            repository.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

fn checked_with_input(repository: &Path, args: &[&str], input: &str) -> Result<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "running `git {}` in {}",
                args.join(" "),
                repository.display()
            )
        })?;
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("piped git stdin")
        .write_all(input.as_bytes())?;
    let result = child.wait_with_output()?;
    if !result.status.success() {
        bail!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            repository.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}
