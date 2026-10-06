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

    /// Create `branch` at `commit` without checking it out. Fails if the
    /// branch already exists.
    fn create_branch(&self, repository: &Path, branch: &str, commit: &str) -> Result<()>;

    /// Create a commit whose only parent is `parent` and whose tree is the
    /// parent's, without changing any ref, the index, or the checkout.
    fn commit_empty(&self, repository: &Path, parent: &str, message: &str) -> Result<String>;

    /// Write `contents` to `path`, relative to the root, and commit only that
    /// file on the current branch, even if it is ignored. Other staged changes
    /// stay staged and out of the commit.
    fn commit_file(
        &self,
        repository: &Path,
        path: &str,
        contents: &str,
        message: &str,
    ) -> Result<String>;

    /// `revision` and its first-parent ancestors, newest first.
    fn first_parent_history(&self, repository: &Path, revision: &str) -> Result<Vec<String>>;

    fn commit_message(&self, repository: &Path, commit: &str) -> Result<String>;

    /// The first parent of `commit`, or `None` for a root commit.
    fn first_parent(&self, repository: &Path, commit: &str) -> Result<Option<String>>;

    /// The paths `commit` changes relative to its parent.
    fn changed_paths(&self, repository: &Path, commit: &str) -> Result<Vec<String>>;
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

    fn commit_file(
        &self,
        repository: &Path,
        path: &str,
        contents: &str,
        message: &str,
    ) -> Result<String> {
        let absolute = repository.join(path);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&absolute, contents)
            .with_context(|| format!("writing {}", absolute.display()))?;
        checked(repository, &["add", "--force", "--", path])?;
        checked_with_input(
            repository,
            &["commit", "--only", "-F", "-", "--", path],
            message,
        )?;
        self.resolve_commit(repository, "HEAD")
    }

    fn first_parent_history(&self, repository: &Path, revision: &str) -> Result<Vec<String>> {
        Ok(lines(&checked(
            repository,
            &["rev-list", "--first-parent", revision],
        )?))
    }

    fn commit_message(&self, repository: &Path, commit: &str) -> Result<String> {
        checked(repository, &["show", "-s", "--format=%B", commit])
    }

    fn first_parent(&self, repository: &Path, commit: &str) -> Result<Option<String>> {
        let parents = checked(repository, &["show", "-s", "--format=%P", commit])?;
        Ok(parents.split_whitespace().next().map(str::to_string))
    }

    fn changed_paths(&self, repository: &Path, commit: &str) -> Result<Vec<String>> {
        Ok(lines(&checked(
            repository,
            &["diff-tree", "--no-commit-id", "--name-only", "-r", commit],
        )?))
    }
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
