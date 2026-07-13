use driva::{BwrapIsolation, Config, ExecutionRequest, Mount, MountAccess};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

struct TestRootfs(PathBuf);

impl TestRootfs {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "driva-bwrap-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for directory in ["proc", "dev", "tmp", "work"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        Self(root)
    }
}

impl Drop for TestRootfs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn translates_request_without_implicit_host_access() {
    let rootfs = TestRootfs::new();
    let backend = BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: rootfs.0.clone(),
        tmpfs: vec![],
    };
    let request = ExecutionRequest {
        command: vec!["printf".into(), "hello".into()],
        working_directory: "/work".into(),
        mounts: vec![Mount {
            source: "/host".into(),
            destination: "/work".into(),
            access: MountAccess::ReadWrite,
        }],
        environment: BTreeMap::from([(OsString::from("A"), OsString::from("B"))]),
        network: false,
        interactive: true,
    };

    let command = backend.command(&request).unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "--unshare-all",
            "--new-session",
            "--die-with-parent",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            "--setenv",
            "A",
            "B",
            "--ro-bind",
            rootfs.0.to_str().unwrap(),
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--bind",
            "/host",
            "/work",
            "--chdir",
            "/work",
            "--",
            "printf",
            "hello"
        ]
    );
}

#[test]
fn shares_network_only_when_granted() {
    let rootfs = TestRootfs::new();
    let backend = BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: rootfs.0.clone(),
        tmpfs: vec![],
    };
    let request = ExecutionRequest {
        command: vec!["true".into()],
        working_directory: "/work".into(),
        mounts: vec![],
        environment: BTreeMap::new(),
        network: true,
        interactive: false,
    };

    let command = backend.command(&request).unwrap();
    assert!(command.get_args().any(|argument| argument == "--share-net"));
}

#[test]
fn creates_private_tmpfs_before_nested_file_mounts() {
    let rootfs = TestRootfs::new();
    std::fs::create_dir_all(rootfs.0.join("state")).unwrap();
    std::fs::write(rootfs.0.join("state/auth.json"), "").unwrap();
    let backend = BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: rootfs.0.clone(),
        tmpfs: vec!["/state".into()],
    };
    let request = ExecutionRequest {
        command: vec!["true".into()],
        working_directory: "/work".into(),
        mounts: vec![Mount {
            source: "/host/auth.json".into(),
            destination: "/state/auth.json".into(),
            access: MountAccess::ReadWrite,
        }],
        environment: BTreeMap::new(),
        network: false,
        interactive: false,
    };

    let command = backend.command(&request).unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    let tmpfs = args
        .windows(2)
        .position(|args| args == ["--tmpfs", "/state"]);
    let bind = args
        .windows(3)
        .position(|args| args == ["--bind", "/host/auth.json", "/state/auth.json"]);
    assert!(tmpfs.unwrap() < bind.unwrap());
}

#[test]
fn permits_paths_created_beneath_private_tmpfs() {
    let rootfs = TestRootfs::new();
    std::fs::create_dir(rootfs.0.join("home")).unwrap();
    let backend = BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: rootfs.0.clone(),
        tmpfs: vec!["/home".into()],
    };
    let request = ExecutionRequest {
        command: vec!["true".into()],
        working_directory: "/tmp/workspace".into(),
        mounts: vec![
            Mount {
                source: "/host/project".into(),
                destination: "/tmp/workspace".into(),
                access: MountAccess::ReadWrite,
            },
            Mount {
                source: "/host/auth.json".into(),
                destination: "/home/codex/.codex/auth.json".into(),
                access: MountAccess::ReadWrite,
            },
        ],
        environment: BTreeMap::new(),
        network: false,
        interactive: false,
    };

    let command = backend.command(&request).unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    assert!(args.windows(2).any(|args| args == ["--tmpfs", "/home"]));
    assert!(args
        .windows(3)
        .any(|args| args == ["--bind", "/host/project", "/tmp/workspace"]));
}

#[test]
fn rejects_destinations_missing_from_read_only_rootfs() {
    let rootfs = TestRootfs::new();
    let backend = BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: rootfs.0.clone(),
        tmpfs: vec![],
    };
    let request = ExecutionRequest {
        command: vec!["true".into()],
        working_directory: "/missing".into(),
        mounts: vec![],
        environment: BTreeMap::new(),
        network: false,
        interactive: false,
    };

    let error = backend.command(&request).unwrap_err();
    assert!(error
        .to_string()
        .contains("working directory does not exist in the rootfs"));
}

#[test]
fn parses_bwrap_configuration() {
    let config: Config = toml::from_str(
        r#"
        [isolation]
        backend = "bwrap"

        [isolation.bwrap]
        rootfs = "/srv/driva/rootfs"
        workdir = "/work"
        executable = "/usr/bin/bwrap"
        "#,
    )
    .unwrap();

    assert_eq!(config.isolation.backend, "bwrap");
    assert_eq!(
        config.isolation.bwrap.rootfs.as_deref(),
        Some(Path::new("/srv/driva/rootfs"))
    );
    assert_eq!(config.isolation.bwrap.workdir, Path::new("/work"));
    assert_eq!(
        config.isolation.bwrap.executable,
        Path::new("/usr/bin/bwrap")
    );
}

#[test]
fn bwrap_is_the_configuration_default() {
    let config = Config::default();
    assert_eq!(config.isolation.backend, "bwrap");
    assert_eq!(config.isolation.bwrap.rootfs, None);
    assert_eq!(config.isolation.bwrap.workdir, Path::new("/tmp"));
    assert_eq!(config.isolation.bwrap.executable, Path::new("bwrap"));
}
