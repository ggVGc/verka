use super::*;

#[test]
fn resolution_takes_the_first_executable_file_on_the_search_path() {
    let root = std::env::temp_dir().join(format!("genta-resolve-{}", std::process::id()));
    let (empty, decoy, real) = (root.join("empty"), root.join("decoy"), root.join("real"));
    for directory in [&empty, &decoy, &real] {
        std::fs::create_dir_all(directory).unwrap();
    }
    // A same-named file that is not executable must not shadow the install.
    std::fs::write(decoy.join("claude"), "notes about claude").unwrap();
    let installed = real.join("claude");
    std::fs::write(&installed, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();

    let search = std::env::join_paths([&empty, &decoy, &real]).unwrap();
    assert_eq!(
        resolve_executable_on_path(Path::new("claude"), &search).unwrap(),
        installed
    );
}

/// The common Claude Code install is a launcher symlink into a versioned
/// directory. Both are visible inside the sandbox, so the link is kept: it is
/// what the operator installed and what an update repoints.
#[test]
fn an_explicit_executable_is_used_as_given_including_a_launcher_symlink() {
    let root = std::env::temp_dir().join(format!("genta-symlink-{}", std::process::id()));
    let (bin, versions) = (root.join("bin"), root.join("versions"));
    for directory in [&bin, &versions] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let versioned = versions.join("2.1.219");
    std::fs::write(&versioned, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&versioned, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launcher = bin.join("claude");
    let _ = std::fs::remove_file(&launcher);
    std::os::unix::fs::symlink(&versioned, &launcher).unwrap();

    assert_eq!(
        resolve_executable_on_path(&launcher, OsStr::new("")).unwrap(),
        launcher,
        "the symlink is followed to check the target, but not resolved away"
    );
}

#[test]
fn an_explicit_executable_that_is_not_a_program_is_rejected() {
    let path = std::env::temp_dir().join(format!("genta-not-a-program-{}", std::process::id()));
    std::fs::write(&path, "not executable").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let error = resolve_executable_on_path(&path, OsStr::new(""))
        .expect_err("a non-executable file is not an agent");
    assert!(format!("{error:#}").contains("not an executable file"));
}
