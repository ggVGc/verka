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
