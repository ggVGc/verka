//! The base system as an operator meets it: what the command line and a
//! project's `driva.toml` do to the private root, checked through the compiled
//! binary rather than the library, since that composition is the feature.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Project(PathBuf);

impl Project {
    fn new(name: &str, config: &str) -> Self {
        let root = std::env::temp_dir().join(format!("driva-base-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).expect("create the project directory");
        if !config.is_empty() {
            std::fs::write(root.join("driva.toml"), config).expect("write driva.toml");
        }
        Self(root)
    }

    fn run(&self, arguments: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_driva"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .expect("failed to execute the driva binary")
    }

    fn dry_run(&self, arguments: &[&str]) -> String {
        let mut all = vec!["run", "--dry-run"];
        all.extend_from_slice(arguments);
        all.extend_from_slice(&["--", "/bin/sh", "-c", "true"]);
        let output = self.run(&all);
        assert!(
            output.status.success(),
            "`driva {}` failed: {}",
            all.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("driva emitted non-UTF-8 output")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// The default base is what a project without configuration runs on, and it is
/// the host's system paths — not its root, and not the operator's home.
#[test]
fn the_default_base_is_laid_down_without_configuration() {
    let project = Project::new("default", "");
    let invocation = project.dry_run(&[]);

    assert!(invocation.contains("\"--tmpfs\" \"/\""));
    assert!(invocation.contains("\"--ro-bind\" \"/usr\" \"/usr\""));
    assert!(!invocation.contains("\"--ro-bind\" \"/\" \"/\""));
    if let Some(home) = std::env::var_os("HOME") {
        let home = Path::new(&home).display().to_string();
        assert!(
            !invocation.contains(&format!("\"--ro-bind\" \"{home}\"")),
            "{invocation}"
        );
    }
}

/// `--no-base` is the whole opt-out: an empty root holding only what is
/// mounted into it, for a caller who states every path themselves.
#[test]
fn no_base_leaves_the_root_empty() {
    let project = Project::new("empty", "");
    let invocation = project.dry_run(&["--no-base"]);

    assert!(invocation.contains("\"--tmpfs\" \"/\""));
    assert!(!invocation.contains("\"--ro-bind\" \"/usr\" \"/usr\""));
}

/// A capability left out is left out, and the rest of the base is untouched:
/// the unit of the opt-out is the capability, not the path.
#[test]
fn a_capability_can_be_dropped_from_one_invocation() {
    let project = Project::new("drop", "");
    let with = project.dry_run(&[]);
    let without = project.dry_run(&["--no-capability", "timezone"]);

    assert!(with.contains("\"/etc/localtime\""), "{with}");
    assert!(!without.contains("\"/etc/localtime\""), "{without}");
    assert!(without.contains("\"--ro-bind\" \"/usr\" \"/usr\""));
}

/// A project states which capabilities its sandboxes are built from, and what
/// one means on this host when the built-in definition is wrong for it.
#[test]
fn a_project_replaces_the_list_and_a_definition() {
    let project = Project::new(
        "configured",
        r#"
[base]
include = ["core", "site"]

[capability.site]
description = "This site's own additions"
path = [{ at = "/etc/hosts" }, { at = "/nonexistent/site", optional = true }]
"#,
    );
    let invocation = project.dry_run(&[]);

    assert!(invocation.contains("\"--ro-bind\" \"/usr\" \"/usr\""));
    assert!(invocation.contains("\"/etc/hosts\""), "{invocation}");
    // `identity` and `timezone` are not in the project's list.
    assert!(!invocation.contains("\"/etc/localtime\""), "{invocation}");
    assert!(
        !invocation.contains("\"/nonexistent/site\""),
        "{invocation}"
    );
}

/// A path a capability requires and this host does not have fails the launch,
/// naming the capability. A sandbox missing part of its floor is the failure
/// this replaces — one that would otherwise surface inside whatever ran.
#[test]
fn a_missing_required_path_fails_the_launch_by_name() {
    let project = Project::new(
        "missing",
        r#"
[base]
include = ["core", "site"]

[capability.site]
path = [{ at = "/nonexistent/site" }]
"#,
    );
    let output = project.run(&["run", "--dry-run", "--", "/bin/sh", "-c", "true"]);
    let message = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(message.contains("site"), "{message}");
    assert!(message.contains("/nonexistent/site"), "{message}");
}

/// A capability nobody defined is a typo in policy, and the error says which
/// names would have worked.
#[test]
fn an_unknown_capability_is_rejected() {
    let project = Project::new("unknown", "");
    let output = project.run(&[
        "run",
        "--dry-run",
        "--capability",
        "speling",
        "--",
        "/bin/sh",
        "-c",
        "true",
    ]);
    let message = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(message.contains("speling"), "{message}");
    assert!(message.contains("certificates"), "{message}");
}

/// A capability may forward host environment variables a program cannot find
/// on disk — a proxy, a certificate bundle — and a value the invocation states
/// itself still wins.
#[test]
fn a_capability_forwards_named_host_variables() {
    let project = Project::new(
        "environment",
        r#"
[base]
include = ["core", "site"]

[capability.site]
path = [{ at = "/etc/hosts" }]
environment = ["DRIVA_TEST_PROXY"]
"#,
    );
    let forwarded = |arguments: &[&str], value: &str| {
        let mut all = vec!["run", "--dry-run"];
        all.extend_from_slice(arguments);
        all.extend_from_slice(&["--", "/bin/sh", "-c", "true"]);
        let output = Command::new(env!("CARGO_BIN_EXE_driva"))
            .current_dir(&project.0)
            .env("DRIVA_TEST_PROXY", value)
            .args(&all)
            .output()
            .expect("failed to execute the driva binary");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("driva emitted non-UTF-8 output")
    };

    let invocation = forwarded(&[], "http://proxy.invalid:3128");
    assert!(
        invocation.contains("\"--setenv\" \"DRIVA_TEST_PROXY\" \"http://proxy.invalid:3128\""),
        "{invocation}"
    );

    // The base is the floor for the environment the way its paths are for the
    // filesystem: what the invocation names is what the command sees.
    let overridden = forwarded(
        &["--env", "DRIVA_TEST_PROXY=http://stated.invalid:3128"],
        "http://proxy.invalid:3128",
    );
    assert!(
        overridden.contains("\"--setenv\" \"DRIVA_TEST_PROXY\" \"http://stated.invalid:3128\""),
        "{overridden}"
    );
    assert!(
        !overridden.contains("\"http://proxy.invalid:3128\""),
        "{overridden}"
    );
}

/// A template states what its command needs and the base grows to match, so a
/// project that trims its list does not have to know what an agent requires.
#[test]
fn a_template_adds_the_capabilities_its_command_requires() {
    let project = Project::new(
        "template",
        r#"
[base]
include = ["core"]

[template.probe]
description = "A command that needs to resolve a name"
command = ["/bin/sh", "-c", "true"]
capability = ["dns"]
"#,
    );
    let without = project.dry_run(&[]);
    let with = project.dry_run(&["--template", "probe"]);

    assert!(!without.contains("\"/etc/resolv.conf\""), "{without}");
    assert!(with.contains("\"/etc/resolv.conf\""), "{with}");
}

/// `driva capabilities` says what is available and, by position, which ones
/// this configuration includes and in what order they are laid down.
#[test]
fn capabilities_reports_what_the_configuration_includes() {
    let project = Project::new(
        "listing",
        r#"
[base]
include = ["core", "dns"]
"#,
    );
    let output = project.run(&["capabilities"]);
    let listing = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(listing.contains("1\tcore\t"), "{listing}");
    assert!(listing.contains("2\tdns\t"), "{listing}");
    // Defined, available to `--capability`, but not part of this base.
    assert!(listing.contains("-\ttimezone\t"), "{listing}");
}
