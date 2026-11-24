use super::*;
use crate::executor::MountSpec;
use driva::{ExecutionOutcome, ExecutionRequest, ProcessExit};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// A backend that records the validated request it received and writes to
/// the caller's streams the way a real command would. The request log is
/// shared so the test keeps a handle after boxing the backend.
struct StubBackend {
    seen: Arc<Mutex<Vec<ExecutionRequest>>>,
    exit: i32,
    stdout: &'static str,
}

impl Isolation for StubBackend {
    fn run(&self, request: &ExecutionRequest, mut io: ExecutionIo) -> Result<ExecutionOutcome> {
        self.seen.lock().unwrap().push(request.clone());
        writeln!(io.stdout, "{}", self.stdout).unwrap();
        writeln!(io.stderr, "to stderr").unwrap();
        let now = SystemTime::now();
        Ok(ExecutionOutcome {
            exit: ProcessExit::Code(self.exit),
            evidence: driva::ExecutionEvidence {
                isolation_backend: "stub".into(),
                effective_policy: driva::effective_policy(request),
                started_at: now,
                finished_at: now,
            },
        })
    }
}

fn spec(dir: &Path) -> ExecutionSpec {
    ExecutionSpec {
        command: vec!["agent".into(), "--work".into()],
        protocol: OutputFormat::Plain,
        working_directory: "/tmp/orka/workspace".into(),
        mounts: vec![
            MountSpec {
                source: dir.join("ws"),
                destination: "/tmp/orka/workspace".into(),
                writable: true,
            },
            MountSpec {
                source: dir.join("ctx"),
                destination: "/context".into(),
                writable: false,
            },
        ],
        environment: BTreeMap::from([(
            "ORKA_OUTCOME".into(),
            "/tmp/orka/exchange/outcome.toml".into(),
        )]),
        network: false,
    }
}

fn artifacts(dir: &Path, protocol: OutputFormat) -> ExecutionArtifacts {
    ExecutionArtifacts {
        transcript: dir.join("transcript.log"),
        diagnostics: dir.join("diagnostics.log"),
        raw_events: (protocol == OutputFormat::Agent(genta::event::Protocol::CodexJsonl))
            .then(|| dir.join("events.raw.jsonl")),
        file_changes: (protocol == OutputFormat::Agent(genta::event::Protocol::CodexJsonl))
            .then(|| dir.join("file-changes.v1.jsonl")),
        file_change_ref: (protocol == OutputFormat::Agent(genta::event::Protocol::CodexJsonl))
            .then(|| "refs/orka/file-changes/test".into()),
        accesses: dir.join("accesses.v1.jsonl"),
    }
}

fn init_workspace_repository(path: &Path) {
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "test"][..],
        &["config", "user.email", "test@example.com"][..],
        &["commit", "--allow-empty", "-qm", "base"][..],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

#[test]
fn the_grant_is_translated_verbatim_and_streams_are_kept_separate() {
    let dir = std::env::temp_dir().join(format!("orka-driva-test-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    std::fs::create_dir_all(dir.join("ctx")).unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let executor = DrivaExecutor::new(Box::new(StubBackend {
        seen: seen.clone(),
        exit: 3,
        stdout: "to stdout",
    }));

    let artifacts = artifacts(&dir, OutputFormat::Plain);
    let report = executor.run(&spec(&dir), &artifacts).unwrap();
    assert_eq!(report.exit_code, 3);
    assert_eq!(report.backend, "stub");

    assert_eq!(
        std::fs::read_to_string(&artifacts.transcript).unwrap(),
        "to stdout\n"
    );
    assert_eq!(
        std::fs::read_to_string(&artifacts.diagnostics).unwrap(),
        "to stderr\n"
    );

    let seen = seen.lock().unwrap();
    let request = &seen[0];
    assert_eq!(request.command, vec!["agent", "--work"]);
    assert!(!request.network, "networking stays denied");
    assert!(!request.interactive);
    assert_eq!(request.mounts.len(), 2);
    let Mount::Bind { source, access, .. } = &request.mounts[0] else {
        panic!("workspace grant is not a bind mount");
    };
    assert_eq!(*access, MountAccess::ReadWrite);
    assert_eq!(
        source,
        &dir.join("ws").canonicalize().unwrap(),
        "driva canonicalised the source"
    );
    let Mount::Bind { access, .. } = &request.mounts[1] else {
        panic!("context grant is not a bind mount");
    };
    assert_eq!(*access, MountAccess::ReadOnly);
    assert_eq!(
        request.environment.get(&OsString::from("ORKA_OUTCOME")),
        Some(&OsString::from("/tmp/orka/exchange/outcome.toml"))
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn codex_jsonl_is_captured_raw_without_writing_any_interpretation() {
    let dir = std::env::temp_dir().join(format!("orka-codex-events-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    std::fs::create_dir_all(dir.join("ctx")).unwrap();
    init_workspace_repository(&dir.join("ws"));
    let executor = DrivaExecutor::new(Box::new(StubBackend {
        seen: Arc::new(Mutex::new(Vec::new())),
        exit: 0,
        stdout: r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Finished cleanly"}}"#,
    }));
    let mut spec = spec(&dir);
    spec.protocol = OutputFormat::Agent(genta::event::Protocol::CodexJsonl);
    let artifacts = artifacts(
        &dir,
        OutputFormat::Agent(genta::event::Protocol::CodexJsonl),
    );

    executor.run(&spec, &artifacts).unwrap();

    // The raw event stream is captured verbatim as the fundamental fact.
    let raw = std::fs::read_to_string(artifacts.raw_events.as_ref().unwrap()).unwrap();
    assert!(raw.contains("agent_message"));
    // No transcript is written for an event-stream agent: the readable form
    // is an interpretation, produced on demand, never stored at rest.
    assert!(
        !artifacts.transcript.exists(),
        "no rendered transcript should be persisted for a Codex agent"
    );
    assert_eq!(
        std::fs::read_to_string(artifacts.diagnostics).unwrap(),
        "to stderr\n"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_mount_source_is_refused_before_the_backend_runs() {
    let dir = std::env::temp_dir().join(format!("orka-driva-missing-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let executor = DrivaExecutor::new(Box::new(StubBackend {
        seen: seen.clone(),
        exit: 0,
        stdout: "unused",
    }));
    // `ws` and `ctx` were never created: validation must refuse the grant.
    let result = executor.run(&spec(&dir), &artifacts(&dir, OutputFormat::Plain));
    assert!(result.is_err());
    assert!(seen.lock().unwrap().is_empty(), "backend never ran");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn admission_probe_runs_orka_owned_git_writability_checks() {
    let dir = std::env::temp_dir().join(format!("orka-probe-test-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    std::fs::create_dir_all(dir.join("ctx")).unwrap();
    init_workspace_repository(&dir.join("ws"));
    let input_commit = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(dir.join("ws"))
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let executor = DrivaExecutor::new(Box::new(StubBackend {
        seen: seen.clone(),
        exit: 0,
        stdout: "",
    }));
    let workspace = PreparedWorkspace {
        schema: crate::workspace::WORKSPACE_SCHEMA,
        path: dir.join("ws"),
        git_dir: dir.join("ws/.git"),
        branch: "orka/attempts/test".into(),
        input_commit,
        identity: "probe-test".into(),
        audit: Default::default(),
    };

    executor
        .validate_workspace_access(&spec(&dir), &workspace)
        .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 5);
    assert!(seen.iter().all(|request| !request.network));
    assert!(seen.iter().all(|request| request.command[0] == "git"));
    assert!(seen
        .iter()
        .any(|request| request.command.iter().any(|arg| arg == "add")));
    assert!(seen
        .iter()
        .any(|request| request.command.iter().any(|arg| arg == "hash-object")));
    assert_eq!(
        seen.iter()
            .filter(|request| request.command.iter().any(|arg| arg == "update-ref"))
            .count(),
        2
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn real_bwrap_probe_rejects_a_linked_worktree_with_read_only_git_metadata() {
    // Some CI kernels disable unprivileged user namespaces. In that
    // environment the backend cannot run at all, so this integration
    // assertion is not applicable.
    let usable = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--", "true"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !usable {
        return;
    }

    let dir = std::env::temp_dir().join(format!("orka-linked-probe-test-{}", ulid::Ulid::new()));
    let project = dir.join("project");
    let workspace_path = dir.join("linked");
    std::fs::create_dir_all(&project).unwrap();
    init_workspace_repository(&project);
    assert!(Command::new("git")
        .arg("-C")
        .arg(&project)
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "orka/attempts/test",
            workspace_path.to_str().unwrap(),
        ])
        .status()
        .unwrap()
        .success());
    let input_commit = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&workspace_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let git_dir = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&workspace_path)
            .args(["rev-parse", "--absolute-git-dir"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .into();
    let workspace = PreparedWorkspace {
        schema: crate::workspace::WORKSPACE_SCHEMA,
        path: workspace_path.clone(),
        git_dir,
        branch: "orka/attempts/test".into(),
        input_commit,
        identity: "linked-test".into(),
        audit: Default::default(),
    };
    let spec = ExecutionSpec {
        command: vec!["unused".into()],
        protocol: OutputFormat::Plain,
        working_directory: "/tmp/orka/workspace".into(),
        mounts: vec![MountSpec {
            source: workspace_path,
            destination: "/tmp/orka/workspace".into(),
            writable: true,
        }],
        environment: BTreeMap::new(),
        network: false,
    };
    let executor = DrivaExecutor::bwrap("bwrap", "/", vec![]);

    assert!(
        executor
            .validate_workspace_access(&spec, &workspace)
            .is_err(),
        "the old linked-worktree grant must fail before an agent starts"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn real_bwrap_probe_accepts_an_audited_shared_git_mount() {
    let usable = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--", "true"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !usable {
        return;
    }

    use crate::workspace::{GitWorkspaces, WorkspaceManager};
    let dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("orka-shared-probe-test-{}", ulid::Ulid::new()));
    let project = dir.join("project");
    std::fs::create_dir_all(&project).unwrap();
    init_workspace_repository(&project);
    let input_commit = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let manager = GitWorkspaces::new(&project, dir.join(".orka/worktrees"));
    let workspace = manager.prepare("shared-probe", &input_commit).unwrap();
    let mut spec = ExecutionSpec {
        command: vec!["unused".into()],
        protocol: OutputFormat::Plain,
        working_directory: "/tmp/orka/workspace".into(),
        mounts: vec![
            MountSpec {
                source: workspace.path.clone(),
                destination: "/tmp/orka/workspace".into(),
                writable: true,
            },
            MountSpec {
                source: workspace.git_dir.clone(),
                destination: workspace.git_dir.clone(),
                writable: true,
            },
        ],
        environment: BTreeMap::new(),
        network: false,
    };
    let executor = DrivaExecutor::bwrap("bwrap", "/", vec![]);

    executor
        .validate_workspace_access(&spec, &workspace)
        .expect("shared Git metadata should be writable through the exact sandbox grant");
    manager
        .validate(&workspace)
        .expect("the admission probe must preserve the shared repository audit");

    spec.command = vec![
        "sh".into(),
        "-c".into(),
        "printf 'sandbox commit\\n' > committed.txt && \
             git add -A && git commit -q -m 'sandbox commit'"
            .into(),
    ];
    let report = executor
        .run(&spec, &artifacts(&dir, OutputFormat::Plain))
        .expect("a real sandboxed Git commit should succeed");
    assert_eq!(report.exit_code, 0);
    let validated = manager
        .validate(&workspace)
        .expect("sandboxed commit must preserve the shared repository audit");
    assert_ne!(validated.head, workspace.input_commit);
    assert!(manager.is_clean(&workspace).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
}
