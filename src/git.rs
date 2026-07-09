//! Thin wrappers over the `git` CLI.
//!
//! llaundry does not hash produced files itself — git already does. When a node is
//! completed, its produced files are committed, and the resulting commit hash (a
//! content hash of the diff) is stored on the node. Staleness of those outputs is
//! then just `git diff <commit>`, which also gives the explicit reason for free.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::store::Store;
use crate::vcs::Vcs;

/// The real [`Vcs`]: drives the `git` CLI over the workbench's two separate
/// repositories. Store commits go to the workbench repo; everything about
/// outputs — capturing, drift, file listing, working-tree cleanliness — is
/// the project repo's business. The [`Vcs`] trait already splits along that
/// line, so each method simply picks its repository.
pub struct GitVcs {
    /// The project repository (`<workbench>/project`): output commits.
    project: PathBuf,
    /// The workbench repository (the store's parent): store history.
    workbench: PathBuf,
}

impl GitVcs {
    pub fn new(project: PathBuf, workbench: PathBuf) -> Self {
        Self { project, workbench }
    }

    /// The conventional wiring: both repository roots derived from the store's
    /// location in its workbench.
    pub fn for_store(store: &Store) -> Self {
        Self::new(store.project_root(), store.workbench_root())
    }
}

impl Vcs for GitVcs {
    fn capture(&self, paths: &[String], message: &str) -> Result<String> {
        commit_paths(&self.project, paths, message)
    }
    fn commit_store(&self, path: &str, message: &str) -> Result<()> {
        commit_path(&self.workbench, path, message)
    }
    fn drift(&self, id: &str) -> Result<Option<String>> {
        output_drift(&self.project, id)
    }

    fn dirty_paths(&self) -> Result<Vec<String>> {
        let out = checked(&self.project, &["status", "--porcelain"])?;
        Ok(out
            .lines()
            .filter_map(|l| l.get(3..).map(|p| p.trim().to_string()))
            .filter(|p| !p.is_empty())
            .collect())
    }

    fn files_in(&self, id: &str) -> Result<Vec<String>> {
        commit_files(&self.project, id)
    }
}

/// `git init` a directory unless it already is a repository (its own, not a
/// parent's). Returns whether a repository was created.
pub fn ensure_repo(dir: &Path) -> Result<bool> {
    if dir.join(".git").exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    checked(dir, &["init"])?;
    Ok(true)
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

/// The files a commit touches.
fn commit_files(base: &Path, commit: &str) -> Result<Vec<String>> {
    let out = checked(
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
    Ok(out.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// If any file introduced by `commit` differs from its state at that commit,
/// return a short `git diff --name-status` description; otherwise `None`.
fn output_drift(base: &Path, commit: &str) -> Result<Option<String>> {
    let paths = commit_files(base, commit)?;
    if paths.is_empty() {
        return Ok(None);
    }

    let mut args = vec!["diff", "--name-status", commit, "--"];
    args.extend(paths.iter().map(String::as_str));
    let drift = checked(base, &args)?;
    Ok((!drift.is_empty()).then_some(drift))
}
