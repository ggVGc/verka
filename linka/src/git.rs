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
use crate::vcs::{ArtifactStore, BranchStore, ContextIdentity, RepositoryIdentity, StoreHistory};

/// Git-backed implementations of Linka's narrow graph capabilities.
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

impl ArtifactStore for GitVcs {
    fn capture(&self, paths: &[String], message: &str) -> Result<String> {
        commit_paths(&self.project, paths, message)
    }
    fn capture_worktree(&self, parent: &str, message: &str) -> Result<Option<String>> {
        commit_worktree(&self.project, parent, message)
    }
    fn retain_output(&self, node: &str, commit: &str) -> Result<()> {
        let refname = format!("refs/linka/outputs/{node}");
        checked(&self.project, &["update-ref", &refname, commit])?;
        Ok(())
    }
    fn drift(&self, id: &str) -> Result<Option<String>> {
        output_drift(&self.project, id)
    }

    fn drift_at(&self, id: &str, revision: &str) -> Result<Option<String>> {
        output_drift_at(&self.project, id, revision)
    }

    fn dirty_paths(&self) -> Result<Vec<String>> {
        // Do not use `checked`: its outer trim would remove the leading index
        // status column from the first line (for example ` M file`).
        // Git's default untracked-files mode reports a wholly untracked
        // directory as one `dir/` entry. Completion validates exact declared
        // output paths, so enumerate the files beneath it instead.
        let out = git(
            &self.project,
            &["status", "--porcelain", "--untracked-files=all"],
        )?;
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

    fn commit_exists(&self, hash: &str) -> Result<bool> {
        // `cat-file -e` exits non-zero for a missing object; that is the
        // answer, not an error.
        let probe = format!("{hash}^{{commit}}");
        Ok(git(&self.project, &["cat-file", "-e", &probe])?
            .status
            .success())
    }
}

impl ContextIdentity for GitVcs {
    fn head_commit(&self) -> Result<Option<String>> {
        if !git(&self.project, &["rev-parse", "--verify", "--quiet", "HEAD"])?
            .status
            .success()
        {
            return Ok(None);
        }
        Ok(Some(checked(&self.project, &["rev-parse", "HEAD"])?))
    }
    fn linka_node(&self, commit: &str) -> Result<Option<String>> {
        let format = "%(trailers:key=Linka-Node,valueonly)";
        let value = checked(
            &self.project,
            &["show", "-s", &format!("--format={format}"), commit],
        )?;
        let mut values = value.lines().filter(|line| !line.trim().is_empty());
        let Some(node) = values.next() else {
            return Ok(None);
        };
        if values.next().is_some() {
            bail!("commit {commit} has more than one Linka-Node trailer");
        }
        Ok(Some(node.trim().to_string()))
    }
    fn tree_id(&self, commit: &str) -> Result<String> {
        checked(&self.project, &["rev-parse", &format!("{commit}^{{tree}}")])
    }
    fn file_blob(&self, path: &str) -> Result<Option<String>> {
        crate::store::file_blob(&self.project.join(path))
    }
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>> {
        let output = git(
            &self.project,
            &["rev-parse", "--verify", &format!("{revision}:{path}")],
        )?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    }
}

impl StoreHistory for GitVcs {
    fn require_clean_store(&self, path: &str) -> Result<()> {
        // The store is entirely Linka-owned, so surrounding repository ignore
        // rules must not hide store state from the transaction boundary.
        let out = git(
            &self.workbench,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--ignored",
                "--",
                path,
            ],
        )?;
        if !out.status.success() {
            bail!(
                "`git status --porcelain=v1 --untracked-files=all --ignored -- {path}` failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let dirty: Vec<_> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        if !dirty.is_empty() {
            bail!("uncommitted store changes:\n  {}", dirty.join("\n  "));
        }
        Ok(())
    }

    fn commit_store(&self, path: &str, message: &str) -> Result<()> {
        commit_path(&self.workbench, path, message)
    }

    fn output_was_recorded(&self, path: &str, node: &str, commit: &str) -> Result<bool> {
        let result = format!("{path}/nodes/{node}/result.toml");
        let out = checked(
            &self.workbench,
            &["log", "--format=%H", "-S", commit, "--", &result],
        )?;
        Ok(!out.is_empty())
    }
}

impl RepositoryIdentity for GitVcs {
    fn root_commit(&self) -> Result<Option<String>> {
        root_commit(&self.project)
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
}

impl BranchStore for GitVcs {
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

    fn ref_commit(&self, reference: &str) -> Result<Option<String>> {
        let out = git(
            &self.project,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ],
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        let out = git(
            &self.project,
            &["merge-base", "--is-ancestor", ancestor, descendant],
        )?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => bail!(
                "git merge-base --is-ancestor failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        }
    }

    fn publish_fast_forward(
        &self,
        target: &str,
        expected_previous: &str,
        candidate: &str,
    ) -> Result<bool> {
        if self.ref_commit(target)?.as_deref() != Some(expected_previous) {
            return Ok(false);
        }
        let ancestor = git(
            &self.project,
            &["merge-base", "--is-ancestor", expected_previous, candidate],
        )?;
        if !ancestor.status.success() {
            return Ok(false);
        }

        let checked_out = checked(&self.project, &["symbolic-ref", "--quiet", "HEAD"]).ok();
        if checked_out.as_deref() == Some(target) {
            if !checked(&self.project, &["status", "--porcelain"])?.is_empty() {
                bail!("project checkout is dirty; commit or stash changes before publishing");
            }
            if checked(&self.project, &["rev-parse", "HEAD"])? != expected_previous {
                return Ok(false);
            }
            checked(&self.project, &["merge", "--ff-only", candidate])?;
        } else {
            checked(
                &self.project,
                &["update-ref", target, candidate, expected_previous],
            )?;
        }
        Ok(true)
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

/// Snapshot the entire final worktree state as one commit parented on
/// `parent`, the frozen input commit the work started from, and leave that
/// commit as the tip of the worktree's checked-out branch.
///
/// Staging with `add -A` folds in every modification, addition, and deletion
/// the agent left — whether or not it made its own commits — so the resulting
/// tree is the absolute final state, and the commit's diff against `parent` is
/// the complete, declaration-free change set. The commit is built with
/// `commit-tree` (parented directly on `parent`, discarding any ad-hoc commits
/// the agent made along the way), then `reset --soft` advances the current
/// branch onto it. Because the index and worktree already equal the captured
/// tree, that leaves the worktree clean — ready to be removed — and the branch
/// pointing at the output, mirroring what a plain `capture` commit produces.
/// Returns `None` when nothing changed relative to `parent`.
fn commit_worktree(base: &Path, parent: &str, message: &str) -> Result<Option<String>> {
    checked(base, &["add", "-A"])?;
    let tree = checked(base, &["write-tree"])?;
    let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
    let parent_arg;
    if parent.is_empty() {
        // No input commit (an empty project repository): the output is a root
        // commit, and any tracked file at all is a change worth capturing.
        if checked(base, &["ls-files"])?.is_empty() {
            return Ok(None);
        }
    } else {
        let parent_tree = checked(base, &["rev-parse", &format!("{parent}^{{tree}}")])?;
        if tree == parent_tree {
            return Ok(None);
        }
        parent_arg = parent.to_string();
        args.push("-p");
        args.push(&parent_arg);
    }
    let commit = checked(base, &args)?;
    checked(base, &["reset", "--soft", &commit])?;
    Ok(Some(commit))
}

/// Commit changes under a single path (e.g. the store directory). The caller
/// has already established a clean baseline, so no staged change means the
/// purported mutation did not produce an atomic operation.
fn commit_path(base: &Path, path: &str, message: &str) -> Result<()> {
    // Ignore rules in the workbench must never exclude part of an atomic
    // store operation. `--all` also stages deletions.
    checked(base, &["add", "--force", "--all", "--", path])?;
    if nothing_staged(base, std::slice::from_ref(&path.to_string()))? {
        bail!("store mutation produced no changes to commit");
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

fn output_drift_at(base: &Path, commit: &str, revision: &str) -> Result<Option<String>> {
    let paths = commit_files(base, commit)?;
    if paths.is_empty() {
        return Ok(None);
    }
    let mut args = vec!["diff", "--name-status", commit, revision, "--"];
    args.extend(paths.iter().map(String::as_str));
    let drift = checked(base, &args)?;
    Ok((!drift.is_empty()).then_some(drift))
}

#[cfg(test)]
mod store_mutation_tests {
    use super::*;
    use crate::model::Author;
    use crate::ops::{self, NewNode};

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn workbench() -> (TempDir, Store, GitVcs) {
        let root = std::env::temp_dir().join(format!(
            "linka-store-mutation-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&root).unwrap();
        checked(&root, &["init"]).unwrap();
        checked(&root, &["config", "user.name", "linka test"]).unwrap();
        checked(&root, &["config", "user.email", "test@linka.invalid"]).unwrap();
        let store = Store::init(root.join(".linka")).unwrap();
        std::fs::write(store.root().join("store.toml"), "schema = 1\n").unwrap();
        checked(&root, &["add", ".linka"]).unwrap();
        checked(&root, &["commit", "-m", "initial store"]).unwrap();
        let vcs = GitVcs::for_store(&store);
        (TempDir(root), store, vcs)
    }

    fn new_node(description: &str) -> NewNode {
        NewNode {
            description: description.into(),
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
        }
    }

    #[test]
    fn clean_store_mutation_is_one_commit_and_finishes_clean() {
        let (_temp, store, vcs) = workbench();
        let before = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();

        let id = ops::add(&store, &vcs, new_node("atomic addition")).unwrap();

        let after = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(before, after);
        vcs.require_clean_store(&store.store_name()).unwrap();
        let changed = checked(
            &store.workbench_root(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        )
        .unwrap();
        assert_eq!(
            changed.lines().collect::<Vec<_>>(),
            vec![
                format!(".linka/nodes/{id}/description.md"),
                format!(".linka/nodes/{id}/node.toml")
            ]
        );
        assert!(store
            .workbench_root()
            .join(".git/linka-mutation.lock")
            .is_file());
    }

    #[test]
    fn dirty_store_refuses_mutation_before_any_edit() {
        let (_temp, store, vcs) = workbench();
        let id = ops::add(&store, &vcs, new_node("existing")).unwrap();
        let before = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(store.node_dir(&id).join("description.md"), "hand edit").unwrap();

        let error = ops::add(&store, &vcs, new_node("must not be created")).unwrap_err();

        assert!(
            format!("{error:#}").contains("uncommitted store changes"),
            "{error:#}"
        );
        assert_eq!(
            checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap(),
            before
        );
        assert_eq!(store.list_ids().unwrap(), vec![id]);
    }

    #[test]
    fn ignored_store_files_are_included_in_the_mutation_commit() {
        let (_temp, store, vcs) = workbench();
        std::fs::write(
            store.workbench_root().join(".gitignore"),
            "description.md\n",
        )
        .unwrap();
        checked(&store.workbench_root(), &["add", ".gitignore"]).unwrap();
        checked(
            &store.workbench_root(),
            &["commit", "-m", "ignore descriptions"],
        )
        .unwrap();

        let id = ops::add(&store, &vcs, new_node("must be committed")).unwrap();

        let changed = checked(
            &store.workbench_root(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        )
        .unwrap();
        assert_eq!(
            changed.lines().collect::<Vec<_>>(),
            vec![
                format!(".linka/nodes/{id}/description.md"),
                format!(".linka/nodes/{id}/node.toml")
            ]
        );
        vcs.require_clean_store(&store.store_name()).unwrap();
    }

    #[test]
    fn preexisting_ignored_store_content_blocks_mutation() {
        let (_temp, store, vcs) = workbench();
        std::fs::write(store.workbench_root().join(".gitignore"), "*.tmp\n").unwrap();
        checked(&store.workbench_root(), &["add", ".gitignore"]).unwrap();
        checked(
            &store.workbench_root(),
            &["commit", "-m", "ignore temporary files"],
        )
        .unwrap();
        std::fs::write(store.root().join("leftover.tmp"), "incomplete mutation\n").unwrap();
        let before = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();

        let error = ops::add(&store, &vcs, new_node("must not be created")).unwrap_err();

        assert!(
            format!("{error:#}").contains("!! .linka/leftover.tmp"),
            "{error:#}"
        );
        assert_eq!(
            checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap(),
            before
        );
        assert!(store.list_ids().unwrap().is_empty());
    }

    #[test]
    fn dirty_paths_enumerates_files_in_untracked_directories() {
        let (temp, store, _) = workbench();
        let project = store.project_root();
        ensure_repo(&project).unwrap();
        std::fs::create_dir_all(project.join("test/nested")).unwrap();
        std::fs::write(project.join("test/nested/output.txt"), "produced\n").unwrap();
        let vcs = GitVcs::new(project, temp.0.clone());

        assert_eq!(vcs.dirty_paths().unwrap(), vec!["test/nested/output.txt"]);
    }

    #[test]
    fn concurrent_store_mutation_is_refused_until_lock_release() {
        let (_temp, store, vcs) = workbench();
        let lock = store.mutation_lock(&vcs).unwrap();

        let error = ops::add(&store, &vcs, new_node("contended")).unwrap_err();
        assert!(error.to_string().contains("mutation is in progress"));
        drop(lock);

        ops::add(&store, &vcs, new_node("after release")).unwrap();
    }
}
