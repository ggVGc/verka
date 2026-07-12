//! Thin wrappers over the `git` CLI.
//!
//! linka does not hash produced files itself — git already does. When a node is
//! completed, its produced files are committed, and the resulting commit hash (a
//! content hash of the diff) is stored on the node. Staleness of those outputs is
//! then just `git diff <commit>`, which also gives the explicit reason for free.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::store::Store;
use crate::vcs::Vcs;

/// A linked worktree on a permanent candidate branch, prepared for one
/// isolated execution.
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub input_commit: String,
    pub input_tree: String,
}

/// Resolve `rev` and return its full commit and tree ids.
pub fn resolve_revision(project: &Path, rev: &str) -> Result<(String, String)> {
    let commit_spec = format!("{rev}^{{commit}}");
    let commit = checked(project, &["rev-parse", "--verify", &commit_spec])?;
    let tree_spec = format!("{commit}^{{tree}}");
    let tree = checked(project, &["rev-parse", &tree_spec])?;
    Ok((commit, tree))
}

/// Create a linked worktree on a new named branch without moving the user's
/// checked-out project branch. The candidate branch remains after cleanup.
pub fn create_worktree(project: &Path, path: PathBuf, branch: &str, rev: &str) -> Result<Worktree> {
    let (input_commit, input_tree) = resolve_revision(project, rev)?;
    if path.exists() {
        bail!("execution worktree path already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating worktree directory {}", parent.display()))?;
    }
    let path_arg = path.to_string_lossy().into_owned();
    checked(project, &["check-ref-format", "--branch", branch])?;
    let branch_ref = format!("refs/heads/{branch}");
    if let Ok(existing) = checked(project, &["rev-parse", "--verify", &branch_ref]) {
        if existing != input_commit {
            bail!("candidate branch `{branch}` exists at {existing}, expected {input_commit}");
        }
        checked(project, &["worktree", "add", &path_arg, branch])?;
    } else {
        checked(
            project,
            &["worktree", "add", "-b", branch, &path_arg, &input_commit],
        )?;
    }
    Ok(Worktree {
        path,
        branch: branch.to_string(),
        input_commit,
        input_tree,
    })
}

pub fn worktree_clean(path: &Path) -> Result<bool> {
    Ok(checked(path, &["status", "--porcelain"])?.is_empty())
}

pub fn remove_worktree(project: &Path, path: &Path) -> Result<()> {
    let path_arg = path.to_string_lossy().into_owned();
    checked(project, &["worktree", "remove", &path_arg])?;
    Ok(())
}

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

    /// Use a linked execution worktree for all project-side operations while
    /// continuing to commit graph state in the store's workbench repository.
    pub fn for_execution(store: &Store, execution_tree: PathBuf) -> Self {
        Self::new(execution_tree, store.workbench_root())
    }
}

impl Vcs for GitVcs {
    fn capture(&self, paths: &[String], message: &str) -> Result<String> {
        commit_paths(&self.project, paths, message)
    }
    fn head_commit(&self) -> Result<Option<String>> {
        if !git(&self.project, &["rev-parse", "--verify", "--quiet", "HEAD"])?
            .status
            .success()
        {
            return Ok(None);
        }
        Ok(Some(checked(&self.project, &["rev-parse", "HEAD"])?))
    }
    fn current_branch(&self) -> Result<Option<String>> {
        let out = git(
            &self.project,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }
    fn resolve_revision(&self, rev: &str) -> Result<(String, String)> {
        resolve_revision(&self.project, rev)
    }
    fn tree_id(&self, commit: &str) -> Result<String> {
        let spec = format!("{commit}^{{tree}}");
        checked(&self.project, &["rev-parse", &spec])
    }
    fn retain_output(&self, node: &str, commit: &str) -> Result<()> {
        let refname = format!("refs/linka/outputs/{node}");
        checked(&self.project, &["update-ref", &refname, commit])?;
        Ok(())
    }
    fn file_blob(&self, path: &str) -> Result<Option<String>> {
        crate::store::file_blob(&self.project.join(path))
    }
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>> {
        let spec = format!("{revision}:{path}");
        let output = git(&self.project, &["rev-parse", "--verify", &spec])?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    }
    fn commit_store(&self, path: &str, message: &str) -> Result<()> {
        commit_path(&self.workbench, path, message)
    }
    fn drift(&self, id: &str) -> Result<Option<String>> {
        output_drift(&self.project, id)
    }

    fn dirty_paths(&self) -> Result<Vec<String>> {
        // Do not use `checked`: its outer trim would remove the leading index
        // status column from the first line (for example ` M file`).
        let out = git(&self.project, &["status", "--porcelain"])?;
        if !out.status.success() {
            bail!(
                "`git status --porcelain` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.get(3..).map(|p| p.trim().to_string()))
            .filter(|p| !p.is_empty())
            .collect())
    }

    fn files_in(&self, id: &str) -> Result<Vec<String>> {
        commit_files(&self.project, id)
    }

    fn root_commit(&self) -> Result<Option<String>> {
        root_commit(&self.project)
    }

    fn commit_exists(&self, hash: &str) -> Result<bool> {
        // `cat-file -e` exits non-zero for a missing object; that is the
        // answer, not an error.
        let probe = format!("{hash}^{{commit}}");
        Ok(git(&self.project, &["cat-file", "-e", &probe])?
            .status
            .success())
    }

    fn remote_url(&self) -> Result<Option<String>> {
        // Non-zero means "no such remote" — an answer, not an error.
        let out = git(&self.project, &["remote", "get-url", "origin"])?;
        if !out.status.success() {
            return Ok(None);
        }
        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok((!url.is_empty()).then_some(url))
    }

    fn ref_commit(&self, reference: &str) -> Result<Option<String>> {
        let out = git(&self.project, &["rev-parse", "--verify", reference])?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    fn publish_fast_forward(&self, target: &str, old: &str, new: &str) -> Result<bool> {
        let target_ref = format!("refs/heads/{target}");
        checked(&self.project, &["check-ref-format", "--branch", target])?;
        let checked_out = checked(&self.project, &["symbolic-ref", "-q", "HEAD"]).ok();
        if checked_out.as_deref() == Some(&target_ref) {
            if !worktree_clean(&self.project)? {
                bail!("target checkout is dirty; refusing to publish over local changes");
            }
            if checked(&self.project, &["rev-parse", "HEAD"])? != old {
                return Ok(false);
            }
            return Ok(git(&self.project, &["merge", "--ff-only", new])?
                .status
                .success());
        }
        Ok(git(&self.project, &["update-ref", &target_ref, new, old])?
            .status
            .success())
    }
    fn create_worktree(&self, path: &Path, branch: &str, rev: &str) -> Result<()> {
        create_worktree(&self.project, path.to_path_buf(), branch, rev)?;
        Ok(())
    }
    fn worktree_clean(&self, path: &Path) -> Result<bool> {
        worktree_clean(path)
    }
    fn remove_worktree(&self, path: &Path) -> Result<()> {
        remove_worktree(&self.project, path)
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

/// The root commit of a repository's mainline: the parentless end of the
/// first-parent walk from HEAD. `None` for a repository with no commits.
/// First-parent keeps the answer single and deterministic even after merges
/// of unrelated histories.
pub fn root_commit(base: &Path) -> Result<Option<String>> {
    if !git(base, &["rev-parse", "--verify", "--quiet", "HEAD"])?
        .status
        .success()
    {
        return Ok(None); // no commits yet
    }
    let out = checked(
        base,
        &["rev-list", "--max-parents=0", "--first-parent", "HEAD"],
    )?;
    Ok(out.lines().next().map(str::to_string))
}

/// Ensure the repository has at least one commit, creating an empty root
/// commit if it has none — so a freshly-initialised project has an identity
/// the store↔project pairing can anchor to. Returns whether a commit was
/// created. Requires a configured git identity when it does commit.
pub fn ensure_root_commit(base: &Path) -> Result<bool> {
    if root_commit(base)?.is_some() {
        return Ok(false);
    }
    checked(base, &["commit", "--allow-empty", "-m", "project root"])?;
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
    Ok(out
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn repo() -> (TempDir, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "linka-git-worktree-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&root).unwrap();
        checked(&root, &["init"]).unwrap();
        checked(&root, &["config", "user.name", "linka test"]).unwrap();
        checked(&root, &["config", "user.email", "test@linka.invalid"]).unwrap();
        std::fs::write(root.join("file.txt"), "base\n").unwrap();
        checked(&root, &["add", "file.txt"]).unwrap();
        checked(&root, &["commit", "-m", "base"]).unwrap();
        (TempDir(root.clone()), root)
    }

    #[test]
    fn isolated_worktree_captures_without_touching_checkout_and_ref_survives_cleanup() {
        let (_temp, project) = repo();
        let original = checked(&project, &["rev-parse", "HEAD"]).unwrap();
        let path = project
            .parent()
            .unwrap()
            .join(format!(".linka-worktree-test-{}", ulid::Ulid::new()));
        let branch = format!("linka/candidates/{}", ulid::Ulid::new());
        let worktree = create_worktree(&project, path.clone(), &branch, "HEAD").unwrap();
        assert_eq!(worktree.input_commit, original);
        assert_eq!(worktree.branch, branch);

        std::fs::write(path.join("file.txt"), "isolated\n").unwrap();
        let vcs = GitVcs::new(path.clone(), project.parent().unwrap().to_path_buf());
        let output = vcs.capture(&["file.txt".into()], "node output").unwrap();
        vcs.retain_output("node-test", &output).unwrap();

        assert_eq!(
            std::fs::read_to_string(project.join("file.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(checked(&project, &["rev-parse", "HEAD"]).unwrap(), original);
        assert!(worktree_clean(&path).unwrap());

        remove_worktree(&project, &path).unwrap();
        assert!(!path.exists());
        assert_eq!(checked(&project, &["rev-parse", &branch]).unwrap(), output);
        assert_eq!(
            checked(&project, &["rev-parse", "refs/linka/outputs/node-test"]).unwrap(),
            output
        );
        assert!(git(
            &project,
            &["cat-file", "-e", &format!("{output}^{{commit}}")]
        )
        .unwrap()
        .status
        .success());
    }

    #[test]
    fn reviewed_candidate_fast_forwards_clean_checked_out_main() {
        let (_temp, project) = repo();
        let base = checked(&project, &["rev-parse", "HEAD"]).unwrap();
        let path = project.parent().unwrap().join(format!(
            ".linka-review-publish-test-{}",
            ulid::Ulid::new()
        ));
        let branch = format!("linka/candidates/{}", ulid::Ulid::new());
        create_worktree(&project, path.clone(), &branch, &base).unwrap();
        std::fs::write(path.join("file.txt"), "accepted\n").unwrap();
        checked(&path, &["add", "file.txt"]).unwrap();
        checked(&path, &["commit", "-m", "candidate"]).unwrap();
        let candidate = checked(&path, &["rev-parse", "HEAD"]).unwrap();

        let vcs = GitVcs::new(project.clone(), project.parent().unwrap().to_path_buf());
        assert!(vcs.publish_fast_forward("main", &base, &candidate).unwrap());
        assert_eq!(
            checked(&project, &["rev-parse", "HEAD"]).unwrap(),
            candidate
        );
        assert_eq!(
            std::fs::read_to_string(project.join("file.txt")).unwrap(),
            "accepted\n"
        );
        remove_worktree(&project, &path).unwrap();
        assert_eq!(
            checked(&project, &["rev-parse", &branch]).unwrap(),
            candidate
        );
    }
}
