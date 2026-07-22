use driva::{Config, MountAccess, TemplateConfig};
use std::fs;
use std::path::{Path, PathBuf};
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

fn prepare_codex_runtime(home: &Path) {
    let family = home.join(".local/share/driva/runtimes/codex");
    let version = family.join("0.144.3");
    let rootfs = version.join("rootfs");
    for directory in [
        "proc",
        "dev",
        "tmp",
        "driva",
        "etc",
        "root/.codex",
        "usr/local/bin",
    ] {
        fs::create_dir_all(rootfs.join(directory)).unwrap();
    }
    fs::write(rootfs.join("root/.codex/auth.json"), "").unwrap();
    fs::write(rootfs.join("etc/resolv.conf"), "").unwrap();
    fs::write(rootfs.join("usr/local/bin/codex"), "").unwrap();
    fs::write(rootfs.join("usr/local/bin/driva-codex"), "").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("0.144.3", family.join("current")).unwrap();
}

#[test]
fn provides_codex_templates() {
    let config = Config::default();
    let codex = config.template("codex").unwrap();
    assert_eq!(&codex.command[..2], ["/bin/sh", "-c"]);
    assert!(codex.command[2].contains("workspace=$(pwd -P)"));
    assert!(codex.command[2].contains("exec codex"));
    assert!(codex.command[2].contains("projects.\\\"$workspace\\\".trust_level"));
    assert!(codex.command[2].contains("\"$@\""));
    assert_eq!(codex.command.last().unwrap(), "driva-codex");
    assert_eq!(codex.backend.as_deref(), Some("bwrap"));
    assert_eq!(codex.rootfs.as_deref(), Some(Path::new("/")));
    assert_eq!(codex.tmpfs, [PathBuf::from("~"), PathBuf::from("/root")]);
    let workspace = &codex.workspace_mounts[0];
    assert_eq!(workspace.source, PathBuf::from("."));
    assert_eq!(workspace.destination, None);
    assert_eq!(workspace.access, MountAccess::ReadWrite);
    assert_eq!(
        codex.environment.get("HOME").map(String::as_str),
        Some("/root")
    );
    assert_eq!(
        codex.environment.get("TERM").map(String::as_str),
        Some("xterm-256color")
    );
    assert!(codex.workdir.is_none());
    assert_eq!(codex.network, Some(true));
    assert_eq!(codex.interactive, Some(true));
    assert_eq!(codex.mounts.len(), 1);
    assert_eq!(
        codex.mounts[0].destination.as_deref(),
        Some(Path::new("/root/.codex"))
    );
    assert_eq!(codex.mounts[0].access, MountAccess::ReadWrite);

    let codex_exec = config.template("codex-exec").unwrap();
    assert_eq!(&codex_exec.command[..2], ["/bin/sh", "-c"]);
    assert!(codex_exec.command[2].contains("exec codex"));
    assert!(codex_exec.command[2].contains("exec --skip-git-repo-check"));
    assert!(codex_exec.command[2].contains("\"$@\""));
    assert_eq!(codex_exec.command.last().unwrap(), "driva-codex-exec");
    assert_eq!(codex_exec.workspace_mounts.len(), 1);
    assert_eq!(codex_exec.interactive, Some(false));

    let codex_runtime = config.template("codex-runtime").unwrap();
    assert_eq!(
        codex_runtime.command.first().map(String::as_str),
        Some("/bin/sh")
    );
    assert_eq!(codex_runtime.backend.as_deref(), Some("bwrap"));
    assert_eq!(
        codex_runtime.rootfs.as_deref(),
        Some(Path::new(
            "~/.local/share/driva/runtimes/codex/current/rootfs"
        ))
    );
    assert_eq!(
        codex_runtime.workspace_mounts[0].destination.as_deref(),
        Some(Path::new("/driva"))
    );
    assert!(codex_runtime.command[2].contains("exec /usr/local/bin/driva-codex"));
    assert!(codex_runtime.command[2].contains("\"$@\""));
    assert_eq!(codex_runtime.command.last().unwrap(), "driva-codex-runtime");
    assert_eq!(codex_runtime.interactive, Some(true));
    assert_eq!(codex_runtime.mounts.len(), 2);
    assert_eq!(
        codex_runtime.mounts[1].destination.as_deref(),
        Some(Path::new("/root/.codex/auth.json"))
    );
}

#[cfg(unix)]
#[test]
fn codex_template_resolves_the_workspace_and_forwards_appended_arguments() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temporary_directory("codex-command-wrapper");
    let workspace = directory.join("workspace with spaces");
    let bin = directory.join("bin");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&bin).unwrap();
    let executable = bin.join("codex");
    fs::write(&executable, "#!/bin/sh\nprintf '<%s>\\n' \"$@\"\n").unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();

    let template = Config::default().template("codex").unwrap();
    let output = Command::new(&template.command[0])
        .args(&template.command[1..])
        .args(["prompt with spaces", "--example"])
        .current_dir(&workspace)
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    assert!(stdout.contains(&format!(
        "<projects.\"{}\".trust_level=\"trusted\">",
        workspace.display()
    )));
    assert!(stdout.contains("<--sandbox>\n<danger-full-access>"));
    assert!(stdout.ends_with("<prompt with spaces>\n<--example>\n"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn provides_claude_code_templates() {
    let config = Config::default();
    let claude = config.template("claude").unwrap();
    assert_eq!(
        claude.command,
        ["npx", "--yes", "@anthropic-ai/claude-code@latest"]
    );
    assert_eq!(claude.backend.as_deref(), Some("podman"));
    assert_eq!(claude.workspace_mounts[0].destination, None);
    assert!(claude.workdir.is_none());
    assert_eq!(claude.network, Some(true));
    assert_eq!(claude.interactive, Some(true));
    assert_eq!(claude.mounts.len(), 1);
    assert_eq!(
        claude.mounts[0].source,
        PathBuf::from("~/.claude/.credentials.json")
    );
    assert_eq!(
        claude.mounts[0].destination.as_deref(),
        Some(Path::new("/root/.claude/.credentials.json"))
    );
    assert_eq!(claude.mounts[0].access, MountAccess::ReadWrite);

    let claude_exec = config.template("claude-exec").unwrap();
    assert_eq!(claude_exec.command.last().unwrap(), "--print");
    assert_eq!(claude_exec.interactive, Some(false));
}

#[test]
fn builtin_template_assets_use_the_public_toml_schema() {
    for name in [
        "claude",
        "claude-exec",
        "codex",
        "codex-exec",
        "codex-runtime",
    ] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("templates")
            .join(format!("{name}.toml"));
        let source = fs::read_to_string(path).unwrap();
        let template: TemplateConfig = toml::from_str(&source).unwrap();
        assert!(matches!(
            template.command.first().map(String::as_str),
            Some("npx" | "/bin/sh")
        ));
        let expected_mounts = match name {
            "codex" | "codex-exec" | "claude" | "claude-exec" => 1,
            "codex-runtime" => 2,
            _ => unreachable!(),
        };
        assert_eq!(template.mounts.len(), expected_mounts);
    }
}

#[test]
fn builtin_codex_uses_the_host_root_and_current_workspace_path() {
    let directory = temporary_directory("builtin-codex");
    fs::create_dir(directory.join(".codex")).unwrap();
    fs::write(directory.join(".codex/auth.json"), "{}").unwrap();
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
    assert!(stdout.contains("backend: bwrap"));
    assert!(stdout.contains("\"--ro-bind\" \"/\" \"/\""));
    assert!(stdout.contains(&format!("\"--tmpfs\" \"{}", directory.display())));
    assert!(stdout.contains("exec codex"));
    let project = directory.display().to_string();
    assert!(stdout.contains(&format!("{project} -> {project} (read-write)")));
    assert!(stdout.contains(&format!("working-directory: {project}")));
    assert!(stdout.contains("workspace=$(pwd -P)"));
    assert!(stdout.contains("$workspace"));
    assert!(stdout.contains("/.codex -> /root/.codex (read-write)"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn builtin_claude_selects_podman_and_mounts_only_credentials() {
    let directory = temporary_directory("builtin-claude");
    fs::create_dir(directory.join(".claude")).unwrap();
    fs::write(directory.join(".claude/.credentials.json"), "{}").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory)
        .env("HOME", &directory)
        .args(["run", "--template", "claude", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("backend: podman"));
    assert!(stdout.contains("@anthropic-ai/claude-code@latest"));
    let project = directory.display().to_string();
    assert!(stdout.contains(&format!("{project} -> {project} (read-write)")));
    assert!(!stdout.contains("/workspace"));
    assert!(stdout
        .contains("/.claude/.credentials.json -> /root/.claude/.credentials.json (read-write)"));
    assert!(!stdout.contains(" -> /root/.claude (read-write)"));

    fs::remove_dir_all(directory).unwrap();
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
    assert_eq!(codex.network, None);
}

#[test]
fn project_template_workspace_mount_defaults_to_the_canonical_source_path() {
    let directory = temporary_directory("project-workspace-mount");
    let config_path = directory.join("driva.toml");
    fs::write(
        &config_path,
        r#"
[template.check]
command = ["true"]
backend = "podman"
image = "example/test:latest"

[[template.check.workspace-mount]]
source = "."
access = "write"
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory)
        .args(["run", "--template", "check", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let workspace = directory.display().to_string();
    assert!(stdout.contains(&format!("working-directory: {workspace}")));
    assert!(stdout.contains(&format!(
        "{} -> {workspace} (read-write)",
        directory.display()
    )));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn builtin_codex_runtime_selects_bwrap_and_works_without_arguments() {
    let directory = temporary_directory("builtin-codex-runtime");
    prepare_codex_runtime(&directory);
    fs::create_dir(directory.join(".codex")).unwrap();
    fs::write(directory.join(".codex/auth.json"), "{}").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory)
        .env("HOME", &directory)
        .args(["run", "--template", "codex-runtime", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("backend: bwrap"));
    assert!(stdout.contains("exec /usr/local/bin/driva-codex"));
    assert!(stdout.contains("--ro-bind"));
    assert!(stdout.contains("\"--tmpfs\" \"/root/.codex\""));
    assert!(stdout.contains("\"--tmpfs\" \"/driva\""));
    assert!(stdout.contains(&format!("{} -> /driva (read-write)", directory.display())));
    assert!(stdout.contains("working-directory: /driva"));
    assert!(stdout.contains("workspace=$(pwd -P)"));
    assert!(!stdout.contains("/workspace"));
    assert!(stdout.contains("/.local/share/driva/runtimes/codex/0.144.3/rootfs"));
    assert!(stdout.contains("--sandbox danger-full-access"));
    assert!(stdout.contains("/etc/resolv.conf -> /etc/resolv.conf (read-only)"));
    assert!(stdout.contains("/.codex/auth.json -> /root/.codex/auth.json (read-write)"));
    assert!(!stdout.contains(" -> /root/.codex (read-write)"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn builtin_codex_runtime_explains_how_to_install_a_missing_runtime() {
    let directory = temporary_directory("missing-codex-runtime");
    fs::create_dir(directory.join(".codex")).unwrap();
    fs::write(directory.join(".codex/auth.json"), "{}").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory)
        .env("HOME", &directory)
        .args(["run", "--template", "codex-runtime", "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("driva runtime install codex@VERSION"));

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
