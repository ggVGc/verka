mod common;

use common::{git, workbench, TempDir};
use orka::config::{CONFIG_FILE, DEFAULT_CONFIG};
use std::path::PathBuf;
use std::process::Command;

fn temp_dir(tag: &str) -> (TempDir, PathBuf) {
    let root = std::env::temp_dir().join(format!("orka-cli-{tag}-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&root).unwrap();
    (TempDir(root.clone()), root)
}

fn orka() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orka"));
    command
        .env("GIT_AUTHOR_NAME", "orka test")
        .env("GIT_AUTHOR_EMAIL", "test@orka.invalid")
        .env("GIT_COMMITTER_NAME", "orka test")
        .env("GIT_COMMITTER_EMAIL", "test@orka.invalid");
    command
}

#[test]
fn run_documents_automatic_acceptance_and_publication() {
    let output = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args(["run", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--auto-accept"));
    assert!(stdout.contains("verification node and publish"));
}

#[test]
fn init_creates_the_default_config_and_refuses_to_replace_it() {
    let (_temp, root) = temp_dir("create");

    let first = orka()
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "init",
            "--create-project",
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join(CONFIG_FILE)).unwrap(),
        DEFAULT_CONFIG
    );

    std::fs::write(root.join(CONFIG_FILE), "keep me\n").unwrap();
    let second = orka()
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "init",
            "--create-project",
        ])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to overwrite"));
    assert_eq!(
        std::fs::read_to_string(root.join(CONFIG_FILE)).unwrap(),
        "keep me\n"
    );
}

#[test]
fn init_attaches_an_existing_git_repository() {
    let (_temp, root) = temp_dir("attach-workbench");
    let (_project_temp, project) = temp_dir("attach-project");
    git(&project, &["init", "-q"]);
    git(&project, &["config", "user.name", "orka test"]);
    git(&project, &["config", "user.email", "test@orka.invalid"]);
    git(
        &project,
        &["commit", "--allow-empty", "-qm", "project root"],
    );
    let project_head = git(&project, &["rev-parse", "HEAD"]);

    let output = orka()
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "init",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        root.join("project").canonicalize().unwrap(),
        project.canonicalize().unwrap()
    );
    assert_eq!(
        git(&root.join("project"), &["rev-parse", "HEAD"]),
        project_head
    );
    assert!(root.join(".linka/pairing.toml").is_file());
    assert_eq!(
        std::fs::read_to_string(root.join(CONFIG_FILE)).unwrap(),
        DEFAULT_CONFIG
    );
}

#[test]
fn init_requires_a_git_directory_or_create_project() {
    let output = orka().args(["init"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("<GIT_DIR>"));
}

#[test]
fn audit_succeeds_when_every_output_has_evidence() {
    let (_temp, root) = workbench();
    let output = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args(["--workbench", root.to_str().unwrap(), "audit"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("complete evidence"));
}
