//! Per-attempt execution workspaces backed by ordinary Git worktrees.
//!
//! Orka owns workspace *policy* — where trees live, how branches are named,
//! when they may be removed — and the Git mechanics that implement it. Agents
//! use Git normally in a linked worktree. The common repository is shared, so
//! Orka records its protected state before execution and refuses settlement if
//! anything except the attempt-owned refs changed.
//!
//! Substituting a different workspace mechanism is genuinely useful (a plain
//! copy, an overlay, a remote checkout), so this stays a narrow Orka-owned
//! trait with the git implementation as one concrete adapter.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const WORKSPACE_SCHEMA: u32 = 2;

/// Protected shared-repository state captured after worktree preparation.
///
/// This record is persisted with the attempt, outside the repository it
/// attests. Object files are deliberately not compared byte-for-byte: normal
/// commits, packing, and maintenance change them. Ref reachability plus fsck
/// provide the semantic object-store check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryAudit {
    pub refs: BTreeMap<String, String>,
    pub protected_files: BTreeMap<String, String>,
    pub worktrees: String,
    pub object_format: String,
}

/// An ordinary linked worktree prepared for one attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedWorkspace {
    pub schema: u32,
    pub path: PathBuf,
    /// The project's shared common Git directory. It is mounted writable so
    /// the agent can use Git normally, then audited before settlement.
    pub git_dir: PathBuf,
    /// The candidate branch the workspace is checked out on.
    pub branch: String,
    pub input_commit: String,
    /// Orka-minted attempt identity used to scope temporary refs.
    pub identity: String,
    pub audit: RepositoryAudit,
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
    /// The linked worktree was removed before anything was promoted.
    Discarded,
    /// The worktree is dirty or its branch has commits beyond the input.
    RetainedChanged,
}

/// Preparing and cleaning isolated per-attempt working trees.
pub trait WorkspaceManager {
    /// Where `prepare` would put the attempt's workspace — pure, so the plan
    /// can be durably recorded before anything is created.
    fn plan(&self, attempt: &str, input_commit: &str) -> PreparedWorkspace;

    /// Create a linked worktree at `input_commit` on a candidate branch named
    /// for `attempt`. Fails if the workspace already exists.
    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace>;

    /// Whether the worktree has no uncommitted changes. A coding
    /// agent is required to commit all its work, so a dirty tree at settle
    /// time means the agent left work uncaptured and the attempt is rejected.
    fn is_clean(&self, workspace: &PreparedWorkspace) -> Result<bool>;

    /// Verify the worktree and audit all protected shared-repository state.
    fn validate(&self, workspace: &PreparedWorkspace) -> Result<ValidatedWorkspace>;

    /// Mark a validated attempt branch as promoted. The commit already lives
    /// in the shared repository because the agent used a normal worktree.
    fn promote(&self, workspace: &ValidatedWorkspace, commit: &str) -> Result<()>;

    /// Remove a workspace whose attempt is sealed. Refuses to discard
    /// uncommitted changes, reporting `RetainedDirty` instead.
    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome>;

    /// Roll back an attempt that produced no exit evidence. Removes both the
    /// worktree only when it still exactly matches the frozen input; otherwise
    /// retains it for inspection.
    fn discard_unchanged(&self, workspace: &PreparedWorkspace) -> Result<DiscardOutcome>;
}

pub struct GitWorkspaces {
    /// The project repository that owns all attempt worktrees.
    project: PathBuf,
    /// Where attempt file trees are created (e.g. `<workbench>/.orka/worktrees`).
    root: PathBuf,
}

impl GitWorkspaces {
    pub fn new(project: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Self {
        Self {
            project: project.into(),
            root: root.into(),
        }
    }

    pub fn branch_for(attempt: &str) -> String {
        format!("orka/attempts/{attempt}")
    }

    pub fn path_for(&self, attempt: &str) -> PathBuf {
        self.root.join(attempt)
    }
}

impl WorkspaceManager for GitWorkspaces {
    fn plan(&self, attempt: &str, input_commit: &str) -> PreparedWorkspace {
        let git_dir = common_git_dir(&self.project).unwrap_or_default();
        PreparedWorkspace {
            schema: WORKSPACE_SCHEMA,
            path: self.path_for(attempt),
            git_dir,
            branch: Self::branch_for(attempt),
            input_commit: input_commit.to_string(),
            identity: format!("orka-workspace-{attempt}"),
            audit: RepositoryAudit {
                refs: BTreeMap::new(),
                protected_files: BTreeMap::new(),
                worktrees: String::new(),
                object_format: String::new(),
            },
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
        let attempt_ref = format!("refs/heads/{}", workspace.workspace.branch);
        if checked(&workspace.workspace.path, &["rev-parse", &attempt_ref])? != commit {
            bail!("refusing to promote commit {commit}: attempt branch moved after validation");
        }
        let marker = promoted_ref(&workspace.workspace);
        match resolve_ref_optional(&self.project, &marker)? {
            Some(existing) if existing == commit => Ok(()),
            Some(existing) => bail!(
                "promotion marker `{marker}` already exists at {existing}, refusing to replace it with {commit}"
            ),
            None => {
                let zero = zero_oid(&self.project)?;
                checked(&self.project, &["update-ref", &marker, commit, &zero])?;
                Ok(())
            }
        }
    }

    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome> {
        if !workspace.path.exists() {
            return Ok(
                if resolve_ref_optional(&self.project, &promoted_ref(workspace))?.is_some()
                    || resolve_ref_optional(
                        &self.project,
                        &format!("refs/heads/{}", workspace.branch),
                    )?
                    .is_none()
                {
                    CleanupOutcome::AlreadyAbsent
                } else {
                    CleanupOutcome::RetainedIntegrityFailure
                },
            );
        }
        let validated = match self.validate(workspace) {
            Ok(validated) => validated,
            Err(_) => return Ok(CleanupOutcome::RetainedIntegrityFailure),
        };
        if !repository_clean(&workspace.path)? {
            return Ok(CleanupOutcome::RetainedDirty);
        }
        if validated.head != workspace.input_commit
            && resolve_ref_optional(&self.project, &promoted_ref(workspace))?.as_deref()
                != Some(validated.head.as_str())
        {
            return Ok(CleanupOutcome::RetainedUnpublished);
        }
        remove_worktree(&self.project, &workspace.path)?;
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
            remove_worktree(&self.project, &workspace.path)?;
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

// --- audited linked-worktree mechanics --------------------------------------

fn create_workspace(project: &Path, mut workspace: PreparedWorkspace) -> Result<PreparedWorkspace> {
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
    if let Some(parent) = workspace.path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating workspace directory {}", parent.display()))?;
    }
    checked(
        project,
        &["check-ref-format", "--branch", &workspace.branch],
    )?;

    let path_arg = workspace.path.to_string_lossy().into_owned();
    let branch_ref = format!("refs/heads/{}", workspace.branch);
    match resolve_ref_optional(project, &branch_ref)? {
        Some(existing) if existing != input_commit => bail!(
            "retained attempt branch `{}` moved from frozen input {} to {existing}",
            workspace.branch,
            input_commit
        ),
        Some(_) => {
            checked(project, &["worktree", "prune"])?;
            checked(
                project,
                &["worktree", "add", "--quiet", &path_arg, &workspace.branch],
            )?;
        }
        None => {
            checked(
                project,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "-b",
                    &workspace.branch,
                    &path_arg,
                    &input_commit,
                ],
            )?;
        }
    }
    workspace.git_dir = common_git_dir(project)?;
    checked(project, &["fsck", "--connectivity-only"])
        .context("shared repository failed preflight connectivity check")?;
    workspace.audit = capture_audit(&workspace)?;
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
        bail!("workspace record has no shared Git audit identity");
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

    let expected_common_dir = workspace.git_dir.canonicalize().with_context(|| {
        format!(
            "canonicalising shared Git directory {}",
            workspace.git_dir.display()
        )
    })?;
    let observed_pointer = PathBuf::from(pointer_target)
        .canonicalize()
        .context("canonicalising workspace Git pointer target")?;
    if !observed_pointer.starts_with(expected_common_dir.join("worktrees")) {
        bail!(
            "workspace Git pointer escaped shared repository {}: observed {}",
            expected_common_dir.display(),
            observed_pointer.display()
        );
    }
    let observed_git_dir = PathBuf::from(checked(
        &workspace.path,
        &["rev-parse", "--absolute-git-dir"],
    )?)
    .canonicalize()
    .context("canonicalising reported Git directory")?;
    if observed_git_dir != observed_pointer {
        bail!("workspace Git pointer and reported Git directory disagree");
    }
    let observed_common = common_git_dir(&workspace.path)?;
    if observed_common != expected_common_dir {
        bail!(
            "workspace common Git directory changed: expected {}, observed {}",
            expected_common_dir.display(),
            observed_common.display()
        );
    }
    let branch_ref = format!("refs/heads/{}", workspace.branch);
    for lock in [
        observed_git_dir.join("index.lock"),
        observed_git_dir.join("HEAD.lock"),
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
    .context("frozen input commit is missing from shared repository")?;
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
        .context("checking workspace ancestry")?;
    if !ancestor.success() {
        bail!("workspace HEAD no longer descends from the frozen input commit");
    }
    checked(&workspace.path, &["fsck", "--connectivity-only"])
        .context("shared repository object graph is corrupt")?;
    verify_audit(workspace)?;
    let tree = checked(&workspace.path, &["rev-parse", &format!("{head}^{{tree}}")])?;
    Ok(ValidatedWorkspace {
        workspace: workspace.clone(),
        head,
        tree,
    })
}

/// Whether an execution worktree has no uncommitted changes.
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

fn common_git_dir(base: &Path) -> Result<PathBuf> {
    let reported = PathBuf::from(checked(base, &["rev-parse", "--git-common-dir"])?);
    let path = if reported.is_absolute() {
        reported
    } else {
        base.join(reported)
    };
    path.canonicalize()
        .context("canonicalising shared Git directory")
}

fn promoted_ref(workspace: &PreparedWorkspace) -> String {
    format!("refs/orka/promoted/{}", workspace.identity)
}

fn checkpoint_ref(workspace: &PreparedWorkspace) -> String {
    let attempt = workspace
        .branch
        .strip_prefix("orka/attempts/")
        .unwrap_or(&workspace.identity);
    format!("refs/orka/file-changes/{attempt}")
}

fn allowed_refs(workspace: &PreparedWorkspace) -> [String; 3] {
    [
        format!("refs/heads/{}", workspace.branch),
        checkpoint_ref(workspace),
        promoted_ref(workspace),
    ]
}

fn capture_audit(workspace: &PreparedWorkspace) -> Result<RepositoryAudit> {
    Ok(RepositoryAudit {
        refs: refs(&workspace.path)?,
        protected_files: protected_files(workspace)?,
        worktrees: normalized_worktrees(&workspace.path, &workspace.path)?,
        object_format: checked(&workspace.path, &["rev-parse", "--show-object-format"])?,
    })
}

fn verify_audit(workspace: &PreparedWorkspace) -> Result<()> {
    let current = capture_audit(workspace)?;
    let allowed = allowed_refs(workspace);
    let head = checked(&workspace.path, &["rev-parse", "HEAD"])?;
    let protected = |refs: &BTreeMap<String, String>, baseline: bool| {
        refs.iter()
            .filter(|(name, value)| {
                if allowed.contains(name) {
                    return false;
                }
                // Linka may retain the already validated output between
                // promotion and cleanup, or before crash recovery. Existing
                // Linka refs remain protected; only a newly created pin to
                // this exact attempt HEAD is an approved host-side transition.
                let approved_linka_pin = !baseline
                    && name.starts_with("refs/linka/outputs/")
                    && !workspace.audit.refs.contains_key(*name)
                    && value.split('\t').next() == Some(head.as_str());
                !approved_linka_pin
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    if protected(&current.refs, false) != protected(&workspace.audit.refs, true) {
        bail!("protected shared Git refs changed during attempt");
    }
    if current.protected_files != workspace.audit.protected_files {
        bail!("protected shared Git configuration, hooks, or alternates changed during attempt");
    }
    if current.worktrees != workspace.audit.worktrees {
        bail!("shared Git worktree registrations changed during attempt");
    }
    if current.object_format != workspace.audit.object_format {
        bail!("shared Git object format changed during attempt");
    }
    Ok(())
}

fn refs(base: &Path) -> Result<BTreeMap<String, String>> {
    let output = checked(
        base,
        &[
            "for-each-ref",
            "--format=%(refname)%09%(objectname)%09%(symref)",
        ],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            line.split_once('\t')
                .map(|(name, value)| (name.into(), value.into()))
        })
        .collect())
}

fn normalized_worktrees(base: &Path, attempt_path: &Path) -> Result<String> {
    let attempt = attempt_path
        .canonicalize()
        .with_context(|| format!("canonicalising attempt worktree {}", attempt_path.display()))?;
    let raw = checked(base, &["worktree", "list", "--porcelain"])?;
    let mut current_is_attempt = false;
    let mut normalized = Vec::new();
    for line in raw.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_is_attempt = PathBuf::from(path)
                .canonicalize()
                .map(|path| path == attempt)
                .unwrap_or(false);
        }
        if current_is_attempt && line.starts_with("HEAD ") {
            normalized.push("HEAD <attempt>".to_string());
        } else {
            normalized.push(line.to_string());
        }
    }
    Ok(normalized.join("\n"))
}

fn protected_files(workspace: &PreparedWorkspace) -> Result<BTreeMap<String, String>> {
    let git_dir = &workspace.git_dir;
    let mut files = BTreeMap::new();
    for relative in [
        "HEAD",
        "index",
        "config",
        "config.worktree",
        "hooks",
        "info/alternates",
        "info/attributes",
        "info/exclude",
        "objects/info/alternates",
        "shallow",
        "MERGE_HEAD",
        "MERGE_MSG",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
        "logs/HEAD",
    ] {
        fingerprint_path(git_dir, Path::new(relative), &mut files)?;
    }
    fingerprint_other_worktrees(workspace, &mut files)?;
    fingerprint_protected_branch_logs(workspace, &mut files)?;
    Ok(files)
}

fn fingerprint_other_worktrees(
    workspace: &PreparedWorkspace,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let attempt_git_dir = PathBuf::from(checked(
        &workspace.path,
        &["rev-parse", "--absolute-git-dir"],
    )?)
    .canonicalize()
    .context("canonicalising attempt Git directory for audit")?;
    let root = workspace.git_dir.join("worktrees");
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("reading shared worktree metadata"),
    };
    for entry in entries {
        if entry.path().canonicalize().ok().as_ref() == Some(&attempt_git_dir) {
            continue;
        }
        fingerprint_path(
            &workspace.git_dir,
            &Path::new("worktrees").join(entry.file_name()),
            files,
        )?;
    }
    Ok(())
}

fn fingerprint_protected_branch_logs(
    workspace: &PreparedWorkspace,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let root = Path::new("logs/refs/heads");
    let excluded = root.join(&workspace.branch);
    fingerprint_tree_except(&workspace.git_dir, root, &excluded, files)
}

fn fingerprint_tree_except(
    root: &Path,
    relative: &Path,
    excluded: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    if relative == excluded {
        return Ok(());
    }
    let path = root.join(relative);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    if !metadata.is_dir() {
        return fingerprint_path(root, relative, files);
    }
    let mut entries = std::fs::read_dir(&path)
        .with_context(|| format!("reading {}", path.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        fingerprint_tree_except(root, &relative.join(entry.file_name()), excluded, files)?;
    }
    Ok(())
}

fn fingerprint_path(
    root: &Path,
    relative: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let path = root.join(relative);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", path.display())),
    };
    if metadata.is_dir() {
        let mut entries = std::fs::read_dir(&path)
            .with_context(|| format!("reading {}", path.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            fingerprint_path(root, &relative.join(entry.file_name()), files)?;
        }
        return Ok(());
    }
    let bytes = if metadata.file_type().is_symlink() {
        std::fs::read_link(&path)?
            .to_string_lossy()
            .as_bytes()
            .to_vec()
    } else {
        std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?
    };
    files.insert(relative.to_string_lossy().into_owned(), hash_bytes(&bytes)?);
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> Result<String> {
    let mut child = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("starting Git content hash")?;
    child
        .stdin
        .take()
        .context("opening Git hash stdin")?
        .write_all(bytes)?;
    let output = child
        .wait_with_output()
        .context("waiting for Git content hash")?;
    if !output.status.success() {
        bail!("Git content hash failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

fn remove_worktree(project: &Path, path: &Path) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    checked(project, &["worktree", "remove", &path]).map(|_| ())
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
    fn workspace_records_require_the_shared_repository_audit_schema() {
        let legacy = r#"
path = "/tmp/workspace"
branch = "orka/attempts/old"
input_commit = "deadbeef"
"#;
        assert!(
            toml::from_str::<PreparedWorkspace>(legacy).is_err(),
            "records without a shared Git audit are not migrated"
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
    fn validation_allows_only_attempt_owned_ref_changes() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        std::fs::write(ws.path.join("out.txt"), "output\n").unwrap();
        git(&ws.path, &["add", "-A"]);
        git(&ws.path, &["commit", "-q", "-m", "expected output"]);
        git(
            &project,
            &["update-ref", "refs/orka/file-changes/attempt-1", &head],
        );
        manager
            .validate(&ws)
            .expect("attempt and checkpoint refs are expected");

        git(&project, &["branch", "unexpected", &head]);
        let error = manager.validate(&ws).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("protected shared Git refs changed"),
            "{error:#}"
        );
        assert_eq!(
            manager.cleanup(&ws).unwrap(),
            CleanupOutcome::RetainedIntegrityFailure
        );
    }

    #[test]
    fn validation_detects_shared_git_configuration_changes() {
        let (_temp, project, head) = project();
        let manager = workspaces(&project);
        let ws = manager.prepare("attempt-1", &head).unwrap();

        git(
            &project,
            &["config", "core.hooksPath", "/tmp/untrusted-hooks"],
        );
        let error = manager.validate(&ws).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("configuration, hooks, or alternates changed"),
            "{error:#}"
        );
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
