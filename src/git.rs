//! Thin wrappers over the `git` CLI.
//!
//! llaundry does not hash produced files itself — git already does. When a node is
//! completed, its produced files are committed, and the resulting commit hash (a
//! content hash of the diff) is stored on the node. Staleness of those outputs is
//! then just `git diff <commit>`, which also gives the explicit reason for free.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::vcs::Vcs;

/// The real [`Vcs`]: drives the `git` CLI rooted at `base` (the project root).
pub struct GitVcs {
    base: PathBuf,
}

impl GitVcs {
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }
}

impl Vcs for GitVcs {
    fn capture(&self, paths: &[String], message: &str) -> Result<String> {
        commit_paths(&self.base, paths, message)
    }
    fn commit_store(&self, path: &str, message: &str) -> Result<()> {
        commit_path(&self.base, path, message)
    }
    fn drift(&self, id: &str) -> Result<Option<String>> {
        output_drift(&self.base, id)
    }
    fn content_id(&self, path: &str) -> Result<Option<String>> {
        // `git hash-object` gives git's blob id for the working-tree file, with no
        // commit required and regardless of whether the file is tracked.
        if !self.base.join(path).exists() {
            return Ok(None);
        }
        Ok(Some(checked(&self.base, &["hash-object", "--", path])?))
    }
}

fn git(base: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `git {}`", args.join(" ")))
}

/// Run a git command, returning trimmed stdout or an error carrying stderr.
fn checked(base: &Path, args: &[&str]) -> Result<String> {
    let out = git(base, args)?;
    if !out.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// True if there are no staged changes for `paths`.
fn nothing_staged(base: &Path, paths: &[String]) -> Result<bool> {
    let mut args = vec!["diff", "--cached", "--quiet", "--"];
    args.extend(paths.iter().map(String::as_str));
    Ok(git(base, &args)?.status.success())
}

/// Commit exactly `paths` (a partial commit that ignores anything else that may be
/// staged), returning the new commit hash.
fn commit_paths(base: &Path, paths: &[String], message: &str) -> Result<String> {
    let mut add = vec!["add", "--"];
    add.extend(paths.iter().map(String::as_str));
    checked(base, &add)?;

    if nothing_staged(base, paths)? {
        bail!("nothing to commit in {paths:?} — were the outputs actually produced?");
    }

    let mut commit = vec!["commit", "-m", message, "--"];
    commit.extend(paths.iter().map(String::as_str));
    checked(base, &commit)?;
    checked(base, &["rev-parse", "HEAD"])
}

/// Commit changes under a single path (e.g. the store directory). No-op if clean.
fn commit_path(base: &Path, path: &str, message: &str) -> Result<()> {
    checked(base, &["add", "--", path])?;
    if nothing_staged(base, std::slice::from_ref(&path.to_string()))? {
        return Ok(());
    }
    checked(base, &["commit", "-m", message, "--", path])?;
    Ok(())
}

/// If any file introduced by `commit` differs from its state at that commit,
/// return a short `git diff --name-status` description; otherwise `None`.
fn output_drift(base: &Path, commit: &str) -> Result<Option<String>> {
    let changed = checked(
        base,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            commit,
        ],
    )?;
    let paths: Vec<&str> = changed.lines().filter(|l| !l.is_empty()).collect();
    if paths.is_empty() {
        return Ok(None);
    }

    let mut args = vec!["diff", "--name-status", commit, "--"];
    args.extend(paths.iter().copied());
    let drift = checked(base, &args)?;
    Ok((!drift.is_empty()).then_some(drift))
}
