//! The production [`IsolatedExecutor`] adapter over the Driva library.
//!
//! Orka's [`ExecutionSpec`] is translated into a Driva execution request and
//! run through `driva::execute`, which validates the grant (deny-by-default
//! mounts and networking) before invoking the backend. Stdout is retained as
//! either plain text or a raw event journal, stderr is retained separately as
//! diagnostics, and the returned report carries harness-observed evidence.

use crate::access::{read_access_summary, write_access_summary, AccessRecorder};
use crate::agent::OutputFormat;
use crate::executor::{ExecutionArtifacts, ExecutionReport, ExecutionSpec, IsolatedExecutor};
use crate::file_changes::FileChangeRecorder;
use crate::workspace::PreparedWorkspace;
use anyhow::{Context, Result};
use driva::{ExecutionIo, Isolation, Mount, MountAccess, WritableMountMode};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct DrivaExecutor {
    backend: Box<dyn Isolation>,
    temporary_mounts: Vec<std::path::PathBuf>,
}

impl DrivaExecutor {
    pub fn new(backend: Box<dyn Isolation>) -> Self {
        Self {
            backend,
            temporary_mounts: Vec::new(),
        }
    }

    pub fn bwrap(
        executable: impl Into<std::path::PathBuf>,
        rootfs: impl Into<std::path::PathBuf>,
        temporary_mounts: Vec<std::path::PathBuf>,
    ) -> Self {
        Self {
            backend: Box::new(driva::BwrapIsolation {
                executable: executable.into(),
                rootfs: Some(rootfs.into()),
            }),
            temporary_mounts,
        }
    }

    fn request(
        &self,
        spec: &ExecutionSpec,
        command: Vec<OsString>,
        network: bool,
    ) -> driva::ExecutionRequest {
        driva::ExecutionRequest {
            command,
            working_directory: spec.working_directory.clone(),
            mounts: self
                .temporary_mounts
                .iter()
                .cloned()
                .map(|destination| Mount::Temporary { destination })
                .chain(spec.mounts.iter().map(|m| Mount::Bind {
                    source: m.source.clone(),
                    destination: m.destination.clone(),
                    access: if m.writable {
                        MountAccess::ReadWrite
                    } else {
                        MountAccess::ReadOnly
                    },
                }))
                .collect(),
            writable_mounts: WritableMountMode::Direct,
            environment: spec
                .environment
                .iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
            network,
            interactive: false,
            new_session: true,
        }
    }

    fn run_probe_command(&self, spec: &ExecutionSpec, arguments: &[String]) -> Result<()> {
        let command = std::iter::once(OsString::from("git"))
            .chain(arguments.iter().map(OsString::from))
            .collect();
        let request = self.request(spec, command, false);
        let io = ExecutionIo {
            stdin: File::open("/dev/null").context("opening probe stdin")?,
            stdout: OpenOptions::new()
                .write(true)
                .open("/dev/null")
                .context("opening probe stdout")?,
            stderr: OpenOptions::new()
                .write(true)
                .open("/dev/null")
                .context("opening probe stderr")?,
        };
        let outcome = driva::execute(self.backend.as_ref(), &request, io)?;
        if outcome.exit.code() != 0 {
            anyhow::bail!(
                "sandbox Git admission probe `git {}` exited {}",
                arguments.join(" "),
                outcome.exit.code()
            );
        }
        Ok(())
    }
}

impl IsolatedExecutor for DrivaExecutor {
    fn validate_workspace_access(
        &self,
        spec: &ExecutionSpec,
        workspace: &PreparedWorkspace,
    ) -> Result<()> {
        let root = spec.working_directory.to_string_lossy().into_owned();
        self.run_probe_command(
            spec,
            &[
                "-C".into(),
                root.clone(),
                "status".into(),
                "--porcelain".into(),
            ],
        )?;
        // Exercise the exact index lock/write path required by the contract.
        // The freshly prepared repository is clean, so this is a no-op tree.
        self.run_probe_command(
            spec,
            &["-C".into(), root.clone(), "add".into(), "-A".into()],
        )?;
        // Exercise the private object store. Writing the canonical empty blob
        // is harmless and creates no commit or ref.
        self.run_probe_command(
            spec,
            &[
                "-C".into(),
                root.clone(),
                "hash-object".into(),
                "-w".into(),
                "--stdin".into(),
            ],
        )?;
        let reference = format!("refs/orka/admission/{}", workspace.identity);
        let object_format = std::process::Command::new("git")
            .arg("-C")
            .arg(&workspace.path)
            .args(["rev-parse", "--show-object-format"])
            .output()
            .context("reading workspace Git object format for admission")?;
        if !object_format.status.success() {
            anyhow::bail!("could not read workspace Git object format for admission");
        }
        let zero = match String::from_utf8_lossy(&object_format.stdout).trim() {
            "sha1" => "0".repeat(40),
            "sha256" => "0".repeat(64),
            other => anyhow::bail!("unsupported Git object format `{other}`"),
        };
        self.run_probe_command(
            spec,
            &[
                "-C".into(),
                root.clone(),
                "update-ref".into(),
                reference.clone(),
                workspace.input_commit.clone(),
                zero,
            ],
        )?;
        if let Err(error) = self.run_probe_command(
            spec,
            &[
                "-C".into(),
                root,
                "update-ref".into(),
                "-d".into(),
                reference,
                workspace.input_commit.clone(),
            ],
        ) {
            return Err(error.context("sandbox Git admission probe left its temporary ref behind"));
        }
        Ok(())
    }

    fn run(&self, spec: &ExecutionSpec, artifacts: &ExecutionArtifacts) -> Result<ExecutionReport> {
        let request = self.request(
            spec,
            spec.command.iter().map(OsString::from).collect(),
            spec.network,
        );

        std::fs::write(&artifacts.diagnostics, b"")
            .with_context(|| format!("creating diagnostics {}", artifacts.diagnostics.display()))?;
        // The agent's stdout is the fundamental fact and is captured verbatim:
        // a plain agent's stdout is its transcript, an event-stream agent's is
        // its raw journal. No normalized or rendered copy is written at rest;
        // those interpretations are produced on demand from what is captured
        // here.
        let stdout = match spec.protocol {
            OutputFormat::Plain => {
                std::fs::write(&artifacts.transcript, b"").with_context(|| {
                    format!("creating transcript {}", artifacts.transcript.display())
                })?;
                append_handle(&artifacts.transcript)?
            }
            OutputFormat::Agent(_) => {
                let raw = artifacts
                    .raw_events
                    .as_ref()
                    .context("structured agent execution has no raw event path")?;
                std::fs::write(raw, b"")
                    .with_context(|| format!("creating event journal {}", raw.display()))?;
                append_handle(raw)?
            }
        };
        let io = ExecutionIo {
            stdin: File::open("/dev/null").context("opening /dev/null for agent stdin")?,
            stdout,
            stderr: append_handle(&artifacts.diagnostics)?,
        };

        let file_change_recorder = if spec.protocol.records_file_changes() {
            let workspace = spec
                .mounts
                .iter()
                .find(|mount| mount.destination == spec.working_directory)
                .context("Codex JSONL execution has no workspace mount")?;
            Some(FileChangeRecorder::start(
                &workspace.source,
                &spec.working_directory,
                spec.environment
                    .get("ORKA_OUTCOME")
                    .map(PathBuf::from)
                    .into_iter()
                    .collect(),
                artifacts
                    .raw_events
                    .as_deref()
                    .context("Codex JSONL execution has no raw event path")?,
                artifacts
                    .file_changes
                    .as_deref()
                    .context("Codex JSONL execution has no file-change journal")?,
                artifacts
                    .file_change_ref
                    .as_deref()
                    .context("Codex JSONL execution has no file-change ref")?,
            )?)
        } else {
            None
        };

        let access_recorder = spec
            .mounts
            .iter()
            .find(|mount| mount.destination == spec.working_directory)
            .map(|mount| AccessRecorder::start(&mount.source, &artifacts.accesses));
        if access_recorder.is_none() {
            write_access_summary(
                &artifacts.accesses,
                "filesystem-watcher",
                &[],
                false,
                Some(format!(
                    "no workspace mount found at {}",
                    spec.working_directory.display()
                )),
            )?;
        }
        let outcome = driva::execute(self.backend.as_ref(), &request, io);
        let checkpoint_error = if let Some(recorder) = file_change_recorder {
            match recorder.finish() {
                Ok(()) => None,
                Err(error) => {
                    if let Ok(mut diagnostics) = append_handle(&artifacts.diagnostics) {
                        let _ = writeln!(
                            diagnostics,
                            "orka: could not finish file-change checkpointing: {error:#}"
                        );
                    }
                    Some(error)
                }
            }
        } else {
            None
        };
        if let Some(recorder) = access_recorder {
            if let Err(error) = recorder.finish() {
                if let Ok(mut diagnostics) = append_handle(&artifacts.diagnostics) {
                    let _ = writeln!(
                        diagnostics,
                        "orka: could not finish filesystem access tracking: {error:#}"
                    );
                }
            }
        }
        match read_access_summary(&artifacts.accesses) {
            Ok(summary) if !summary.complete => {
                if let Ok(mut diagnostics) = append_handle(&artifacts.diagnostics) {
                    let _ = writeln!(
                        diagnostics,
                        "orka: filesystem access tracking is incomplete: {}",
                        summary
                            .reason
                            .as_deref()
                            .unwrap_or("no reason was recorded")
                    );
                }
            }
            Err(error) => {
                if let Ok(mut diagnostics) = append_handle(&artifacts.diagnostics) {
                    let _ = writeln!(
                        diagnostics,
                        "orka: could not read filesystem access evidence: {error:#}"
                    );
                }
            }
            _ => {}
        }
        if let Some(error) = checkpoint_error {
            return Err(error.context(
                "refusing to complete execution because Git checkpointing lost repository integrity",
            ));
        }
        let outcome = outcome?;
        Ok(ExecutionReport {
            backend: outcome.evidence.isolation_backend,
            exit_code: outcome.exit.code(),
            started_at_ms: unix_millis(outcome.evidence.started_at),
            finished_at_ms: unix_millis(outcome.evidence.finished_at),
        })
    }
}

fn append_handle(path: &Path) -> Result<File> {
    OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("opening output stream {}", path.display()))
}

fn unix_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
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

        let dir =
            std::env::temp_dir().join(format!("orka-linked-probe-test-{}", ulid::Ulid::new()));
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
}
