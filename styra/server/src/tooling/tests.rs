use super::*;

/// The private root already holds the system runtime, so a tool installed
/// there needs no mount of its own — and one installed outside it does.
#[test]
fn only_executables_outside_the_private_root_become_mounts() {
    let runtime = vec![
        RuntimeEntry::ReadOnly {
            source: PathBuf::from("/usr"),
            path: PathBuf::from("/usr"),
        },
        RuntimeEntry::Symlink {
            target: PathBuf::from("usr/bin"),
            path: PathBuf::from("/bin"),
        },
    ];
    assert!(carried_by_runtime(&runtime, Path::new("/usr/bin/tmux")));
    assert!(!carried_by_runtime(
        &runtime,
        Path::new("/home/operator/.local/bin/claude")
    ));
    // The symlinked path is not itself bound; the resolved target is what
    // the private root carries.
    assert!(!carried_by_runtime(&runtime, Path::new("/bin/tmux")));
}

/// A launcher outside the runtime is mounted at the path the command names
/// and at the path it resolves to, once each.
#[test]
fn a_launcher_and_its_target_are_both_mounted_once() {
    let directory = std::env::temp_dir().join(format!("styra-tooling-{}", std::process::id()));
    let installed = directory.join("share/agent");
    let launcher = directory.join("bin/agent");
    std::fs::create_dir_all(installed.parent().expect("share directory")).expect("create");
    std::fs::create_dir_all(launcher.parent().expect("bin directory")).expect("create");
    std::fs::write(&installed, b"#!/bin/sh\n").expect("write agent");
    std::os::unix::fs::symlink(&installed, &launcher).expect("link agent");

    let mounts = executable_mounts(
        &[launcher.clone(), launcher.clone()],
        &driva::BaseConfig::default(),
    )
    .expect("mounts");

    let destinations: Vec<&Path> = mounts
        .iter()
        .map(|mount| mount.destination.as_path())
        .collect();
    assert_eq!(
        destinations,
        vec![
            launcher.as_path(),
            installed.canonicalize().expect("canonical").as_path()
        ]
    );
    assert!(mounts.iter().all(|mount| !mount.writable));
    std::fs::remove_dir_all(&directory).ok();
}

/// An executable that is not there is a launch that cannot work; say so
/// while the session is being planned rather than after it fails to spawn.
#[test]
fn a_missing_executable_is_an_error() {
    let missing = PathBuf::from("/nonexistent/styra/agent");
    assert!(executable_mounts(&[missing], &driva::BaseConfig::default()).is_err());
}
