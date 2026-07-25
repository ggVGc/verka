//! Per-attempt execution workspaces with private Git repositories.
//!
//! Orka owns workspace *policy* — where trees live, how branches are named,
//! when they may be removed — and the git mechanics that implement it. Each
//! attempt gets a fresh repository anchored to its frozen input commit, so the
//! user's checkout, branch, index, and uncommitted changes are never touched,
//! and concurrent attempts share no writable Git state. Only after Orka
//! validates the final repository does it import the output and candidate
//! branch into the project.
//!
//! Substituting a different workspace mechanism is genuinely useful (a plain
//! copy, an overlay, a remote checkout), so this stays a narrow Orka-owned
//! trait with the git implementation as one concrete adapter.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const WORKSPACE_SCHEMA: u32 = 1;

/// An isolated working tree prepared for one attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedWorkspace {
    pub schema: u32,
    pub path: PathBuf,
    /// Private Git metadata for this attempt. It is mounted writable into the
    /// sandbox without granting access to the project's shared `.git`.
    pub git_dir: PathBuf,
    /// The candidate branch the workspace is checked out on.
    pub branch: String,
    pub input_commit: String,
    /// Orka-minted identity stored in the private Git directory.
    pub identity: String,
}

/// A workspace whose repository identity has been independently attested by
/// Orka. Submission APIs require this type so a merely clean directory cannot
/// be mistaken for the attempt repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedWorkspace {
    pub workspace: PreparedWorkspace,
    pub head: String,
    pub tree: String,
}

/// What cleanup observed. A dirty workspace is retained, never discarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupOutcome {
    Removed,
    RetainedDirty,
    RetainedUnpublished,
    RetainedIntegrityFailure,
    AlreadyAbsent,
}

/// Whether an unexecuted workspace could be rolled back without losing work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscardOutcome {
    /// The private repository was removed before anything was promoted.
    Discarded,
    /// The private repository is dirty or its branch has commits beyond the input.
    RetainedChanged,
}

/// Preparing and cleaning isolated per-attempt working trees.
pub trait WorkspaceManager {
    /// Where `prepare` would put the attempt's workspace — pure, so the plan
    /// can be durably recorded before anything is created.
    fn plan(&self, attempt: &str, input_commit: &str) -> PreparedWorkspace;

    /// Create a fresh private repository at `input_commit` on a candidate
    /// branch named for `attempt`. Fails if the workspace already exists.
    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace>;

    /// Whether the private repository has no uncommitted changes. A coding
    /// agent is required to commit all its work, so a dirty tree at settle
    /// time means the agent left work uncaptured and the attempt is rejected.
    fn is_clean(&self, workspace: &PreparedWorkspace) -> Result<bool>;

    /// Verify that this is still the exact repository Orka prepared. This is
    /// structural validation, independent of anything the agent declared.
    fn validate(&self, workspace: &PreparedWorkspace) -> Result<ValidatedWorkspace>;

    /// Import a validated output into the project repository and advance only
    /// this attempt's candidate branch. No project object or ref is written
    /// before postflight validation succeeds.
    fn promote(&self, workspace: &ValidatedWorkspace, commit: &str) -> Result<()>;

    /// Remove a workspace whose attempt is sealed. Refuses to discard
    /// uncommitted changes, reporting `RetainedDirty` instead.
    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome>;

    /// Roll back an attempt that produced no exit evidence. Removes both the
    /// workspace and private repository only when they still exactly match
    /// the frozen input; otherwise retains them for inspection.
    fn discard_unchanged(&self, workspace: &PreparedWorkspace) -> Result<DiscardOutcome>;
}

pub struct GitWorkspaces {
    /// The project repository used as immutable preparation input and as the
    /// destination for validated output promotion.
    project: PathBuf,
    /// Where attempt file trees are created (e.g. `<workbench>/.orka/worktrees`).
    root: PathBuf,
    /// Private per-attempt Git repositories, kept outside the writable file
    /// tree so `.git` can be mounted read-only over the workspace bind.
    git_root: PathBuf,
}

impl GitWorkspaces {
    pub fn new(project: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let git_root = root
            .parent()
            .map(|parent| parent.join("gitdirs"))
            .unwrap_or_else(|| PathBuf::from("gitdirs"));
        Self {
            project: project.into(),
            root,
            git_root,
        }
    }

    pub fn branch_for(attempt: &str) -> String {
        format!("orka/attempts/{attempt}")
    }

    pub fn path_for(&self, attempt: &str) -> PathBuf {
        self.root.join(attempt)
    }

    pub fn git_dir_for(&self, attempt: &str) -> PathBuf {
        self.git_root.join(attempt)
    }
}

impl WorkspaceManager for GitWorkspaces {
    fn plan(&self, attempt: &str, input_commit: &str) -> PreparedWorkspace {
        PreparedWorkspace {
            schema: WORKSPACE_SCHEMA,
            path: self.path_for(attempt),
            git_dir: self.git_dir_for(attempt),
            branch: Self::branch_for(attempt),
            input_commit: input_commit.to_string(),
            identity: format!("orka-workspace-{attempt}"),
        }
    }

    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace> {
        let planned = self.plan(attempt, input_commit);
        create_workspace(&self.project, planned)
            .with_context(|| format!("preparing workspace for {attempt}"))
    }

    fn is_clean(&self, workspace: &PreparedWorkspace) -> Result<bool> {
        self.validate(workspace)?;
        repository_clean(&workspace.path)
    }

    fn validate(&self, workspace: &PreparedWorkspace) -> Result<ValidatedWorkspace> {
        validate_workspace(workspace)
    }

    fn promote(&self, workspace: &ValidatedWorkspace, commit: &str) -> Result<()> {
        if workspace.head != commit {
            bail!(
                "refusing to promote commit {commit}: validated workspace HEAD is {}",
                workspace.head
            );
        }
        let private_ref = format!("refs/heads/{}", workspace.workspace.branch);
        if checked(&workspace.workspace.path, &["rev-parse", &private_ref])? != commit {
            bail!(
                "refusing to promote commit {commit}: private attempt branch moved after validation"
            );
        }

        // Refuse a conflicting candidate before importing even unreachable
        // objects. A recovery retry that observes the same commit is
        // idempotent.
        let branch_ref = format!("refs/heads/{}", workspace.workspace.branch);
        match resolve_ref_optional(&self.project, &branch_ref)? {
            Some(existing) if existing == commit => Ok(()),
            Some(existing) => bail!(
                "candidate branch `{}` already exists at {existing}, refusing to replace it with {commit}",
                workspace.workspace.branch
            ),
            None => {
                // Fetch objects without naming a destination ref, then advance
                // the candidate with compare-and-swap semantics.
                let git_dir = workspace.workspace.git_dir.to_string_lossy().into_owned();
                checked(
                    &self.project,
                    &[
                        "fetch",
                        "--no-write-fetch-head",
                        "--no-tags",
                        &git_dir,
                        &private_ref,
                    ],
                )?;
                let zero = zero_oid(&self.project)?;
                checked(&self.project, &["update-ref", &branch_ref, commit, &zero])?;
                Ok(())
            }
        }
    }

    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome> {
        if !workspace.path.exists() {
            return Ok(if workspace.git_dir.exists() {
                CleanupOutcome::RetainedIntegrityFailure
            } else {
                CleanupOutcome::AlreadyAbsent
            });
        }
        let validated = match self.validate(workspace) {
            Ok(validated) => validated,
            Err(_) => return Ok(CleanupOutcome::RetainedIntegrityFailure),
        };
        if !repository_clean(&workspace.path)? {
            return Ok(CleanupOutcome::RetainedDirty);
        }
        if validated.head != workspace.input_commit {
            let branch_ref = format!("refs/heads/{}", workspace.branch);
            if resolve_ref_optional(&self.project, &branch_ref)?.as_deref()
                != Some(validated.head.as_str())
            {
                return Ok(CleanupOutcome::RetainedUnpublished);
            }
        }
        std::fs::remove_dir_all(&workspace.path)
            .with_context(|| format!("removing workspace {}", workspace.path.display()))?;
        std::fs::remove_dir_all(&workspace.git_dir).with_context(|| {
            format!(
                "removing private Git directory {}",
                workspace.git_dir.display()
            )
        })?;
        Ok(CleanupOutcome::Removed)
    }

    fn discard_unchanged(&self, workspace: &PreparedWorkspace) -> Result<DiscardOutcome> {
        if workspace.path.exists() {
            if self.validate(workspace).is_err()
                || !repository_clean(&workspace.path)?
                || checked(&workspace.path, &["rev-parse", "HEAD"])? != workspace.input_commit
            {
                return Ok(DiscardOutcome::RetainedChanged);
            }
            std::fs::remove_dir_all(&workspace.path)
                .with_context(|| format!("removing workspace {}", workspace.path.display()))?;
            if workspace.git_dir.exists() {
                std::fs::remove_dir_all(&workspace.git_dir).with_context(|| {
                    format!(
                        "removing private Git directory {}",
                        workspace.git_dir.display()
                    )
                })?;
            }
        }

        let branch_ref = format!("refs/heads/{}", workspace.branch);
        match resolve_ref_optional(&self.project, &branch_ref)? {
            Some(commit) if commit != workspace.input_commit => Ok(DiscardOutcome::RetainedChanged),
            Some(_) => {
                // Supply the expected old value so a concurrent ref move
                // cannot make rollback delete work produced elsewhere.
                checked(
                    &self.project,
                    &["update-ref", "-d", &branch_ref, &workspace.input_commit],
                )?;
                Ok(DiscardOutcome::Discarded)
            }
            None => Ok(DiscardOutcome::Discarded),
        }
    }
}

// --- private Git repository mechanics ---------------------------------------

/// Create a self-contained attempt repository. The project is input only:
/// `--no-hardlinks` ensures even the object store is private.
fn create_workspace(project: &Path, workspace: PreparedWorkspace) -> Result<PreparedWorkspace> {
    let input_commit = checked(
        project,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", workspace.input_commit),
        ],
    )?;
    if workspace.path.exists() {
        bail!(
            "execution workspace path already exists: {}",
            workspace.path.display()
        );
    }
    if workspace.git_dir.exists() {
        // A crash may remove the file tree after leaving the private
        // repository behind. Recreate only when its immutable identity,
        // branch, and HEAD still exactly match the frozen input.
        let identity = std::fs::read_to_string(workspace.git_dir.join("orka-identity"))
            .context("reading retained private workspace identity")?;
        let git_dir_arg = workspace.git_dir.to_string_lossy().into_owned();
        let retained_head = checked_git_dir(&git_dir_arg, &["rev-parse", "HEAD"])?;
        let retained_branch = checked_git_dir(&git_dir_arg, &["symbolic-ref", "--quiet", "HEAD"])?;
        if identity.trim() != workspace.identity
            || retained_head != input_commit
            || retained_branch != format!("refs/heads/{}", workspace.branch)
        {
            bail!(
                "retained private Git directory does not match unchanged attempt {}",
                workspace.identity
            );
        }
        std::fs::remove_dir_all(&workspace.git_dir).with_context(|| {
            format!(
                "removing unchanged retained Git directory {}",
                workspace.git_dir.display()
            )
        })?;
    }
    if let Some(parent) = workspace.path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating workspace directory {}", parent.display()))?;
    }
    if let Some(parent) = workspace.git_dir.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating Git directory root {}", parent.display()))?;
    }
    checked(
        project,
        &["check-ref-format", "--branch", &workspace.branch],
    )?;

    let project_arg = project.to_string_lossy().into_owned();
    let path_arg = workspace.path.to_string_lossy().into_owned();
    let git_dir_arg = workspace.git_dir.to_string_lossy().into_owned();
    let out = Command::new("git")
        .args([
            "clone",
            "--quiet",
            "--no-checkout",
            "--no-hardlinks",
            "--separate-git-dir",
            &git_dir_arg,
            &project_arg,
            &path_arg,
        ])
        .output()
        .context("running private workspace clone")?;
    if !out.status.success() {
        bail!(
            "private workspace clone failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    checked(
        &workspace.path,
        &[
            "checkout",
            "--quiet",
            "-b",
            &workspace.branch,
            &input_commit,
        ],
    )?;
    checked(&workspace.path, &["config", "user.name", "Orka Agent"])?;
    checked(
        &workspace.path,
        &["config", "user.email", "orka-agent@localhost"],
    )?;
    std::fs::write(
        workspace.git_dir.join("orka-identity"),
        format!("{}\n", workspace.identity),
    )
    .with_context(|| {
        format!(
            "writing workspace identity {}",
            workspace.git_dir.join("orka-identity").display()
        )
    })?;
    validate_workspace(&workspace)?;
    Ok(workspace)
}

fn validate_workspace(workspace: &PreparedWorkspace) -> Result<ValidatedWorkspace> {
    if workspace.schema != WORKSPACE_SCHEMA {
        bail!(
            "workspace uses unsupported schema {} (this build reads schema {WORKSPACE_SCHEMA})",
            workspace.schema
        );
    }
    if workspace.git_dir.as_os_str().is_empty() || workspace.identity.is_empty() {
        bail!("workspace record has no private Git identity");
    }
    let pointer_path = workspace.path.join(".git");
    let pointer_metadata = std::fs::symlink_metadata(&pointer_path)
        .with_context(|| format!("inspecting Git pointer {}", pointer_path.display()))?;
    if !pointer_metadata.file_type().is_file() || pointer_metadata.file_type().is_symlink() {
        bail!(
            "workspace Git pointer {} is not the regular file Orka prepared",
            pointer_path.display()
        );
    }
    let pointer = std::fs::read_to_string(&pointer_path)
        .with_context(|| format!("reading Git pointer {}", pointer_path.display()))?;
    let pointer_target = pointer
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .context("workspace Git pointer has invalid contents")?;
    let expected_path = workspace
        .path
        .canonicalize()
        .with_context(|| format!("canonicalising workspace {}", workspace.path.display()))?;
    let top = PathBuf::from(checked(&workspace.path, &["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .context("canonicalising reported Git worktree root")?;
    if top != expected_path {
        bail!(
            "workspace repository root changed: expected {}, observed {}",
            expected_path.display(),
            top.display()
        );
    }

    let expected_git_dir = workspace.git_dir.canonicalize().with_context(|| {
        format!(
            "canonicalising private Git directory {}",
            workspace.git_dir.display()
        )
    })?;
    let observed_pointer = PathBuf::from(pointer_target)
        .canonicalize()
        .context("canonicalising workspace Git pointer target")?;
    if observed_pointer != expected_git_dir {
        bail!(
            "workspace Git pointer changed: expected {}, observed {}",
            expected_git_dir.display(),
            observed_pointer.display()
        );
    }
    for (label, argument) in [
        ("Git directory", "--absolute-git-dir"),
        ("Git common directory", "--git-common-dir"),
    ] {
        let observed = PathBuf::from(checked(&workspace.path, &["rev-parse", argument])?)
            .canonicalize()
            .with_context(|| format!("canonicalising reported {label}"))?;
        if observed != expected_git_dir {
            bail!(
                "workspace {label} changed: expected {}, observed {}",
                expected_git_dir.display(),
                observed.display()
            );
        }
    }

    let identity = std::fs::read_to_string(workspace.git_dir.join("orka-identity"))
        .context("reading private workspace identity")?;
    if identity.trim() != workspace.identity {
        bail!("private workspace identity changed");
    }
    let branch_ref = format!("refs/heads/{}", workspace.branch);
    for lock in [
        workspace.git_dir.join("index.lock"),
        workspace.git_dir.join("HEAD.lock"),
        workspace
            .git_dir
            .join(format!("refs/heads/{}.lock", workspace.branch)),
    ] {
        if lock.exists() {
            bail!("workspace has a lingering Git lock: {}", lock.display());
        }
    }
    if checked(&workspace.path, &["symbolic-ref", "--quiet", "HEAD"])? != branch_ref {
        bail!("workspace is not on its expected attempt branch `{branch_ref}`");
    }
    let head = checked(&workspace.path, &["rev-parse", "HEAD"])?;
    if checked(&workspace.path, &["rev-parse", &branch_ref])? != head {
        bail!("workspace HEAD and attempt branch disagree");
    }
    checked(
        &workspace.path,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", workspace.input_commit),
        ],
    )
    .context("frozen input commit is missing from private workspace")?;
    let ancestor = Command::new("git")
        .arg("-C")
        .arg(&workspace.path)
        .args([
            "merge-base",
            "--is-ancestor",
            &workspace.input_commit,
            &head,
        ])
        .status()
        .context("checking private workspace ancestry")?;
    if !ancestor.success() {
        bail!("workspace HEAD no longer descends from the frozen input commit");
    }
    checked(&workspace.path, &["fsck", "--connectivity-only", &head])
        .context("private workspace object graph is corrupt")?;
    let tree = checked(&workspace.path, &["rev-parse", &format!("{head}^{{tree}}")])?;
    Ok(ValidatedWorkspace {
        workspace: workspace.clone(),
        head,
        tree,
    })
}

/// Whether a private execution repository has no uncommitted changes.
fn repository_clean(path: &Path) -> Result<bool> {
    Ok(checked(path, &["status", "--porcelain"])?.is_empty())
}

fn resolve_ref_optional(project: &Path, reference: &str) -> Result<Option<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["rev-parse", "--verify", "--quiet", reference])
        .output()
        .with_context(|| format!("failed to resolve Git ref `{reference}`"))?;
    if out.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ));
    }
    if out.status.code() == Some(1) {
        return Ok(None);
    }
    bail!(
        "resolving Git ref `{reference}` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

fn zero_oid(project: &Path) -> Result<String> {
    let format = checked(project, &["rev-parse", "--show-object-format"])?;
    match format.as_str() {
        "sha1" => Ok("0".repeat(40)),
        "sha256" => Ok("0".repeat(64)),
        other => bail!("unsupported Git object format `{other}`"),
    }
}

/// Run a git command, returning trimmed stdout or an error carrying stderr.
fn checked(base: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn checked_git_dir(git_dir: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(["--git-dir", git_dir])
        .args(args)
        .output()
        .with_context(|| format!("failed to run private `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "private `git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

    fn git(dir: &PathBuf, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("running git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn project() -> (TempDir, PathBuf, String) {
        let root = std::env::temp_dir().join(format!("orka-ws-git-test-{}", ulid::Ulid::new()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        git(&project, &["init", "-q"]);
        git(&project, &["config", "user.name", "orka test"]);
        git(&project, &["config", "user.email", "test@orka.invalid"]);
        std::fs::write(project.join("file.txt"), "base\n").unwrap();
        git(&project, &["add", "file.txt"]);
        git(&project, &["commit", "-q", "-m", "base"]);
        let head = git(&project, &["rev-parse", "HEAD"]);
        (TempDir(root), project, head)
    }

    fn workspaces(project: &PathBuf) -> GitWorkspaces {
        GitWorkspaces::new(project, project.parent().unwrap().join(".orka/worktrees"))
    }

    #[test]
    fn prepare_anchors_a_fresh_tree_without_touching_the_checkout() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);

        let ws = manager.prepare("attempt-1", &head).unwrap();
        assert_eq!(ws.input_commit, head);
        assert_eq!(ws.branch, "orka/attempts/attempt-1");
        assert_eq!(git(&ws.path, &["rev-parse", "HEAD"]), head);

        // Concurrent attempts get independent trees; the user's checkout and
        // branch never move.
        let other = manager.prepare("attempt-2", &head).unwrap();
        assert_ne!(ws.path, other.path);
        std::fs::write(ws.path.join("file.txt"), "one\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("file.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(git(&project, &["rev-parse", "HEAD"]), head);

        // A second preparation of the same attempt is refused.
        assert!(manager.prepare("attempt-1", &head).is_err());
    }

    #[test]
    fn workspace_records_require_the_private_repository_schema_and_identity() {
        let legacy = r#"
path = "/tmp/workspace"
branch = "orka/attempts/old"
input_commit = "deadbeef"
"#;
        assert!(
            toml::from_str::<PreparedWorkspace>(legacy).is_err(),
            "records without private Git metadata are not migrated"
        );

        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let mut workspace = manager.prepare("attempt-1", &head).unwrap();
        workspace.schema = WORKSPACE_SCHEMA + 1;
        let error = manager.validate(&workspace).unwrap_err();
        assert!(
            error.to_string().contains("unsupported schema"),
            "{error:#}"
        );
    }

    #[test]
    fn cleanup_removes_clean_trees_and_keeps_the_candidate_branch() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        // Commit output in the workspace, as a completed attempt would.
        std::fs::write(ws.path.join("out.txt"), "output\n").unwrap();
        git(&ws.path, &["add", "out.txt"]);
        git(&ws.path, &["commit", "-q", "-m", "output"]);
        let output = git(&ws.path, &["rev-parse", "HEAD"]);
        let validated = manager.validate(&ws).unwrap();
        manager.promote(&validated, &output).unwrap();

        assert_eq!(manager.cleanup(&ws).unwrap(), CleanupOutcome::Removed);
        assert!(!ws.path.exists());
        // The output stays reachable through the candidate branch.
        assert_eq!(git(&project, &["rev-parse", &ws.branch]), output);
        assert_eq!(manager.cleanup(&ws).unwrap(), CleanupOutcome::AlreadyAbsent);
    }

    #[test]
    fn is_clean_reflects_committed_versus_uncommitted_work() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        // A fresh worktree at the input commit is clean.
        assert!(manager.is_clean(&ws).unwrap());

        // An uncommitted write makes it dirty...
        std::fs::write(ws.path.join("out.txt"), "output\n").unwrap();
        assert!(!manager.is_clean(&ws).unwrap());

        // ...and committing it makes it clean again.
        git(&ws.path, &["add", "-A"]);
        git(&ws.path, &["commit", "-q", "-m", "output"]);
        assert!(manager.is_clean(&ws).unwrap());
    }

    #[test]
    fn cleanup_never_discards_uncommitted_changes() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();
        std::fs::write(ws.path.join("scratch.txt"), "unsaved\n").unwrap();

        assert_eq!(manager.cleanup(&ws).unwrap(), CleanupOutcome::RetainedDirty);
        assert_eq!(
            std::fs::read_to_string(ws.path.join("scratch.txt")).unwrap(),
            "unsaved\n"
        );
    }

    #[test]
    fn cleanup_never_discards_committed_but_unpromoted_work() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();
        std::fs::write(ws.path.join("committed.txt"), "valuable\n").unwrap();
        git(&ws.path, &["add", "-A"]);
        git(&ws.path, &["commit", "-q", "-m", "private work"]);

        assert_eq!(
            manager.cleanup(&ws).unwrap(),
            CleanupOutcome::RetainedUnpublished
        );
        assert!(ws.path.join("committed.txt").is_file());
        assert!(ws.git_dir.is_dir());
    }

    #[test]
    fn discard_removes_an_unchanged_tree_and_its_branch() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        assert_eq!(
            manager.discard_unchanged(&ws).unwrap(),
            DiscardOutcome::Discarded
        );
        assert!(!ws.path.exists());
        assert!(git(&project, &["branch", "--list", &ws.branch]).is_empty());
    }

    #[test]
    fn discard_retains_dirty_or_committed_work() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let dirty = manager.prepare("attempt-dirty", &head).unwrap();
        std::fs::write(dirty.path.join("scratch.txt"), "unsaved\n").unwrap();
        assert_eq!(
            manager.discard_unchanged(&dirty).unwrap(),
            DiscardOutcome::RetainedChanged
        );
        assert!(dirty.path.exists());

        let committed = manager.prepare("attempt-committed", &head).unwrap();
        std::fs::write(committed.path.join("file.txt"), "changed\n").unwrap();
        git(&committed.path, &["commit", "-q", "-am", "partial work"]);
        assert_eq!(
            manager.discard_unchanged(&committed).unwrap(),
            DiscardOutcome::RetainedChanged
        );
        assert!(committed.path.exists());
    }

    #[test]
    fn a_crashed_preparation_can_be_recovered_or_reanchored() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        // Simulate a crash that lost the tree but kept the branch.
        std::fs::remove_dir_all(&ws.path).unwrap();
        assert_eq!(
            manager.cleanup(&ws).unwrap(),
            CleanupOutcome::RetainedIntegrityFailure
        );

        // Re-preparation reuses the branch because it still sits at the
        // frozen input commit.
        let again = manager.prepare("attempt-1", &head).unwrap();
        assert_eq!(again.input_commit, head);
        assert_eq!(git(&again.path, &["rev-parse", "HEAD"]), head);

        // But a private branch that moved away from the frozen input is
        // refused — the attempt's identity must not silently re-anchor.
        std::fs::write(again.path.join("file.txt"), "moved\n").unwrap();
        git(&again.path, &["commit", "-q", "-am", "moved"]);
        std::fs::remove_dir_all(&again.path).unwrap();
        assert!(manager.prepare("attempt-1", &head).is_err());
    }
}
