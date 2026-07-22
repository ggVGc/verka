use std::path::PathBuf;
use std::process::Command;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "driva-path-test-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("first")).unwrap();
        std::fs::create_dir_all(root.join("second")).unwrap();
        Self(root)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dry_run(backend: &str) -> (TestDirectory, String) {
    let directory = TestDirectory::new(backend);
    std::fs::write(
        directory.0.join("driva.toml"),
        format!("[isolation]\nbackend = {backend:?}\n"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory.0)
        .args([
            "run",
            "--dry-run",
            "--env",
            "PATH=/custom/bin",
            "--path",
            "first",
            "--path",
            "second",
            "--",
            "example-command",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (directory, String::from_utf8(output.stdout).unwrap())
}

#[test]
fn path_directories_are_read_only_and_prepended() {
    let (directory, stdout) = dry_run("bwrap");
    let first = directory.0.join("first").canonicalize().unwrap();
    let second = directory.0.join("second").canonicalize().unwrap();

    assert!(stdout.contains(&format!(
        "mount: {} -> {} (read-only)",
        first.display(),
        first.display()
    )));
    assert!(stdout.contains(&format!(
        "mount: {} -> {} (read-only)",
        second.display(),
        second.display()
    )));
    assert!(stdout.contains(&format!(
        "{}:{}:/custom/bin",
        first.display(),
        second.display()
    )));
    assert!(stdout.contains(&format!("\"--ro-bind\" {:?} {:?}", first, first)));
}

#[test]
fn path_requires_an_existing_directory() {
    let directory = TestDirectory::new("not-directory");
    let file = directory.0.join("tool");
    std::fs::write(&file, "not a directory").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory.0)
        .args(["run", "--dry-run", "--path", "tool", "--", "true"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("PATH addition is not a directory: tool")
    );
}

#[test]
fn path_uses_the_standard_baseline_when_path_is_not_configured() {
    let directory = TestDirectory::new("default");
    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&directory.0)
        .args(["run", "--dry-run", "--path", "first", "--", "true"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let first = directory.0.join("first").canonicalize().unwrap();
    assert!(stdout.contains(&format!("{}:{}", first.display(), driva::DEFAULT_PATH)));
}
