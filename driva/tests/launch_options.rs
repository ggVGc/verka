use std::path::PathBuf;
use std::process::{Command, Output};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "driva-launch-test-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write_config(&self, source: &str) {
        std::fs::write(self.0.join("driva.toml"), source).unwrap();
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_driva"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .unwrap()
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn cli_selects_backend_and_backend_specific_options() {
    let directory = TestDirectory::new("backend");
    let output = stdout(directory.run(&[
        "run",
        "--dry-run",
        "--backend",
        "docker",
        "--image",
        "example:cli",
        "--",
        "true",
    ]));

    assert!(output.contains("backend: docker"));
    assert!(output.contains("\"example:cli\" \"true\""));
}

#[test]
fn cli_rootfs_and_tmpfs_reach_bubblewrap() {
    let directory = TestDirectory::new("bwrap");
    let rootfs = directory.0.join("rootfs");
    for path in ["proc", "dev", "tmp", "work", "home"] {
        std::fs::create_dir_all(rootfs.join(path)).unwrap();
    }
    let output = stdout(directory.run(&[
        "run",
        "--dry-run",
        "--backend",
        "bwrap",
        "--rootfs",
        rootfs.to_str().unwrap(),
        "--tmpfs",
        "/home",
        "--workdir",
        "/work",
        "--",
        "true",
    ]));

    assert!(output.contains("backend: bwrap"));
    assert!(output.contains(&format!("\"--ro-bind\" {:?} \"/\"", rootfs)));
    assert!(output.contains("\"--tmpfs\" \"/home\""));
}

#[test]
fn template_path_uses_the_same_semantics_as_cli_path() {
    let directory = TestDirectory::new("template-path");
    let tools = directory.0.join("tools");
    std::fs::create_dir(&tools).unwrap();
    directory.write_config(
        r#"
        [template.tools]
        backend = "docker"
        path = ["tools"]
        "#,
    );
    let output = stdout(directory.run(&[
        "run",
        "--dry-run",
        "--template",
        "tools",
        "--",
        "example-tool",
    ]));
    let tools = tools.canonicalize().unwrap();

    assert!(output.contains(&format!(
        "mount: {} -> {} (read-only)",
        tools.display(),
        tools.display()
    )));
    assert!(output.contains(&format!("{}:{}", tools.display(), driva::DEFAULT_PATH)));
}

#[test]
fn template_false_overrides_enabled_project_networking() {
    let directory = TestDirectory::new("network");
    directory.write_config(
        r#"
        [network]
        enabled = true

        [template.offline]
        network = false
        "#,
    );
    let output =
        stdout(directory.run(&["run", "--dry-run", "--template", "offline", "--", "true"]));

    assert!(output.contains("network: disabled"));
}

#[test]
fn cli_can_disable_template_interactivity() {
    let directory = TestDirectory::new("interactive");
    directory.write_config(
        r#"
        [template.terminal]
        interactive = true
        "#,
    );
    let output = stdout(directory.run(&[
        "run",
        "--dry-run",
        "--template",
        "terminal",
        "--no-interactive",
        "--",
        "true",
    ]));

    assert!(output.contains("interactive: false"));
}

#[test]
fn no_write_makes_every_host_mount_read_only() {
    let directory = TestDirectory::new("no-write");
    for path in ["configured", "template", "cli"] {
        std::fs::create_dir(directory.0.join(path)).unwrap();
    }
    directory.write_config(
        r#"
        [[mount]]
        source = "configured"
        destination = "/configured"
        access = "write"

        [template.readonly]
        backend = "docker"
        workspace_root = "/workspace"

        [[template.readonly.mount]]
        source = "template"
        destination = "/template"
        access = "write"
        "#,
    );
    let output = stdout(directory.run(&[
        "run",
        "--dry-run",
        "--template",
        "readonly",
        "--no-write",
        "--write",
        "cli:/cli",
        "--",
        "true",
    ]));

    assert_eq!(output.matches("(read-only)").count(), 4);
    assert!(!output.contains("(read-write)"));
    assert_eq!(output.matches(":ro").count(), 4);
}

#[test]
fn rejects_options_for_the_wrong_backend() {
    let directory = TestDirectory::new("wrong-backend");
    let output = directory.run(&[
        "run",
        "--dry-run",
        "--backend",
        "docker",
        "--rootfs",
        "/rootfs",
        "--",
        "true",
    ]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--rootfs is only supported by the Bubblewrap backend"));
}

#[test]
fn rejects_unknown_template_fields() {
    let directory = TestDirectory::new("unknown-field");
    directory.write_config(
        r#"
        [template.broken]
        pathh = ["tools"]
        "#,
    );
    let output = directory.run(&["templates"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field `pathh`"));
}
