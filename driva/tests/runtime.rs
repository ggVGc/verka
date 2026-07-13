use driva::{RuntimeSpec, RuntimeStore};
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
fn parses_only_pinned_codex_runtimes() {
    let spec = RuntimeSpec::parse("codex@0.144.3").unwrap();
    assert_eq!(spec.name, "codex");
    assert_eq!(spec.version, "0.144.3");
    assert!(RuntimeSpec::parse("codex@latest").is_err());
    assert!(RuntimeSpec::parse("codex").is_err());
    assert!(RuntimeSpec::parse("other@1.0.0").is_err());
    assert!(RuntimeSpec::parse("codex@1.0.0/../../escape").is_err());
}

#[test]
fn activates_lists_and_removes_installed_runtimes() {
    let directory = temporary_directory("runtime-store");
    let store = RuntimeStore::new(directory.clone());
    let first = RuntimeSpec::parse("codex@0.144.3").unwrap();
    let second = RuntimeSpec::parse("codex@0.145.0").unwrap();
    fs::create_dir_all(store.rootfs(&first)).unwrap();
    fs::create_dir_all(store.rootfs(&second)).unwrap();

    store.activate(&first).unwrap();
    assert_eq!(
        store.list().unwrap(),
        vec![(first.clone(), true), (second.clone(), false)]
    );
    assert_eq!(
        fs::canonicalize(store.current_rootfs("codex")).unwrap(),
        fs::canonicalize(store.rootfs(&first)).unwrap()
    );

    store.activate(&second).unwrap();
    store.remove(&first).unwrap();
    assert_eq!(store.list().unwrap(), vec![(second.clone(), true)]);
    store.remove(&second).unwrap();
    assert!(store.list().unwrap().is_empty());

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn installs_an_exported_rootfs_and_publishes_it_atomically() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temporary_directory("runtime-install");
    let fixture = directory.join("fixture");
    for path in [
        "proc",
        "dev",
        "tmp",
        "workspace",
        "root/.codex",
        "usr/local/bin",
    ] {
        fs::create_dir_all(fixture.join(path)).unwrap();
    }
    fs::write(fixture.join("usr/local/bin/codex"), "prepared").unwrap();
    fs::write(fixture.join("root/.codex/auth.json"), "").unwrap();

    let fake_podman = directory.join("podman");
    fs::write(
        &fake_podman,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  create) echo test-container > \"$3\" ;;\n  start) exit 0 ;;\n  export) tar -cf \"$3\" -C \"{}\" . ;;\n  rm) exit 0 ;;\n  *) exit 2 ;;\nesac\n",
            fixture.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_podman, fs::Permissions::from_mode(0o755)).unwrap();

    let store = RuntimeStore::new(directory.join("store"));
    let spec = RuntimeSpec::parse("codex@0.144.3").unwrap();
    store
        .install_codex(&spec, "example/node:22", &fake_podman)
        .unwrap();
    assert_eq!(
        fs::read_to_string(store.rootfs(&spec).join("usr/local/bin/codex")).unwrap(),
        "prepared"
    );
    assert!(store
        .rootfs(&spec)
        .parent()
        .unwrap()
        .join("manifest.toml")
        .is_file());
    assert_eq!(store.list().unwrap(), vec![(spec, true)]);

    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn runtime_cli_lists_and_removes_versions() {
    let home = temporary_directory("runtime-cli");
    let store = RuntimeStore::new(home.join(".local/share/driva/runtimes"));
    let spec = RuntimeSpec::parse("codex@0.144.3").unwrap();
    fs::create_dir_all(store.rootfs(&spec)).unwrap();
    store.activate(&spec).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&home)
        .env("HOME", &home)
        .args(["runtime", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "codex@0.144.3\tcurrent\n"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_driva"))
        .current_dir(&home)
        .env("HOME", &home)
        .args(["runtime", "remove", "codex@0.144.3"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!store.rootfs(&spec).exists());

    fs::remove_dir_all(home).unwrap();
}
