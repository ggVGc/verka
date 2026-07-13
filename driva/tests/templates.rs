use driva::{Config, MountAccess};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_directory(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("driva-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn provides_codex_templates() {
    let config = Config::default();
    let codex = config.template("codex").unwrap();
    assert_eq!(codex.command, ["npx", "--yes", "@openai/codex@latest"]);
    assert_eq!(codex.backend.as_deref(), Some("podman"));
    assert_eq!(codex.workdir.unwrap(), PathBuf::from("/workspace"));
    assert!(codex.network);
    assert!(codex.interactive);
    assert_eq!(codex.mounts.len(), 2);
    assert_eq!(codex.mounts[0].access, MountAccess::ReadWrite);

    let codex_exec = config.template("codex-exec").unwrap();
    assert_eq!(codex_exec.command.last().unwrap(), "exec");
    assert!(!codex_exec.interactive);
}

#[test]
fn project_template_replaces_a_builtin() {
    let config: Config = toml::from_str(
        r#"
[template.codex]
description = "Local Codex build"
command = ["codex-from-image"]
image = "example/codex:pinned"
"#,
    )
    .unwrap();
    let codex = config.template("codex").unwrap();
    assert_eq!(codex.description, "Local Codex build");
    assert_eq!(codex.command, ["codex-from-image"]);
    assert!(!codex.network);
}

#[test]
fn builtin_codex_selects_podman_and_works_without_arguments() {
    let directory = temporary_directory("builtin-codex");
    fs::create_dir(directory.join(".codex")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory)
        .env("HOME", &directory)
        .args(["run", "--template", "codex", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("backend: podman"));
    assert!(stdout.contains("docker.io/library/node:22-bookworm"));
    assert!(stdout.contains("\"npx\" \"--yes\" \"@openai/codex@latest\""));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn applies_a_project_template_and_appends_arguments() {
    let directory = temporary_directory("template");
    let config_path = directory.join("driva.toml");
    fs::write(
        &config_path,
        r#"
[isolation]
backend = "podman"

[environment]
BASE = "global"

[template.lint]
description = "Run the project linter"
command = ["cargo", "clippy"]
image = "rust:custom"
workdir = "/src"
network = true
interactive = true

[template.lint.environment]
RUST_LOG = "debug"
BASE = "template"

[[template.lint.mount]]
source = "."
destination = "/src"
access = "write"
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "run",
            "--template",
            "lint",
            "--dry-run",
            "--env",
            "BASE=cli",
            "--",
            "--all-targets",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("network: enabled"));
    assert!(stdout.contains("interactive: true"));
    assert!(stdout.contains("working-directory: /src"));
    assert!(stdout.contains("rust:custom"));
    assert!(stdout.contains("\"RUST_LOG=debug\""));
    assert!(stdout.contains("\"BASE=cli\""));
    assert!(!stdout.contains("\"BASE=global\""));
    assert!(!stdout.contains("\"BASE=template\""));
    assert!(stdout.contains("\"cargo\" \"clippy\" \"--all-targets\""));

    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "run",
            "--template",
            "lint",
            "--no-network",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("network: disabled"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reports_unknown_templates() {
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .args(["run", "--template", "missing", "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unknown template \"missing\""));
    assert!(stderr.contains("driva templates"));
}
