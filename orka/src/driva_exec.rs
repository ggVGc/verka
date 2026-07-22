//! The production [`IsolatedExecutor`] adapter over the Driva library.
//!
//! Orka's [`ExecutionSpec`] is translated into a Driva execution request and
//! run through `driva::execute`, which validates the grant (deny-by-default
//! mounts and networking) before invoking the backend. Stdout is retained as
//! either plain text or a raw event journal, stderr is retained separately as
//! diagnostics, and the returned report carries harness-observed evidence.

use crate::access::{read_access_summary, write_access_summary, AccessRecorder};
use crate::agent::AgentProtocol;
use crate::events::materialize_codex_events_with_checkpoints;
use crate::executor::{ExecutionArtifacts, ExecutionReport, ExecutionSpec, IsolatedExecutor};
use crate::file_changes::FileChangeRecorder;
use anyhow::{anyhow, Context, Result};
use driva::{ExecutionIo, Isolation, Mount, MountAccess};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
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

    pub fn podman(executable: impl Into<std::path::PathBuf>, image: impl Into<String>) -> Self {
        Self::new(Box::new(driva::PodmanIsolation {
            executable: executable.into(),
            image: image.into(),
        }))
    }

    pub fn docker(executable: impl Into<std::path::PathBuf>, image: impl Into<String>) -> Self {
        Self::new(Box::new(driva::DockerIsolation {
            executable: executable.into(),
            image: image.into(),
        }))
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
}

impl IsolatedExecutor for DrivaExecutor {
    fn run(&self, spec: &ExecutionSpec, artifacts: &ExecutionArtifacts) -> Result<ExecutionReport> {
        let request = driva::ExecutionRequest {
            command: spec.command.iter().map(OsString::from).collect(),
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
            environment: spec
                .environment
                .iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
            network: spec.network,
            interactive: false,
        };

        std::fs::write(&artifacts.transcript, b"")
            .with_context(|| format!("creating transcript {}", artifacts.transcript.display()))?;
        std::fs::write(&artifacts.diagnostics, b"")
            .with_context(|| format!("creating diagnostics {}", artifacts.diagnostics.display()))?;
        let stdout = match spec.protocol {
            AgentProtocol::Plain => append_handle(&artifacts.transcript)?,
            AgentProtocol::CodexJsonl => {
                let raw = artifacts
                    .raw_events
                    .as_ref()
                    .context("Codex JSONL execution has no raw event path")?;
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

        let file_change_recorder = if spec.protocol == AgentProtocol::CodexJsonl {
            let workspace = spec
                .mounts
                .iter()
                .find(|mount| mount.destination == spec.working_directory)
                .context("Codex JSONL execution has no workspace mount")?;
            Some(FileChangeRecorder::start(
                &workspace.source,
                &spec.working_directory,
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
        if let Some(recorder) = file_change_recorder {
            if let Err(error) = recorder.finish() {
                if let Ok(mut diagnostics) = append_handle(&artifacts.diagnostics) {
                    let _ = writeln!(
                        diagnostics,
                        "orka: could not finish file-change checkpointing: {error:#}"
                    );
                }
            }
        }
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
            Ok(Some(summary)) if !summary.complete => {
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
        if spec.protocol == AgentProtocol::CodexJsonl {
            let projection = match artifacts.events.as_ref() {
                Some(events) => materialize_codex_events_with_checkpoints(
                    artifacts.raw_events.as_ref().expect("validated above"),
                    events,
                    &artifacts.transcript,
                    artifacts.file_changes.as_deref(),
                ),
                None => Err(anyhow!(
                    "Codex JSONL execution has no normalized event path"
                )),
            };
            if let Err(error) = projection {
                // The raw stream and process outcome remain authoritative.
                // A presentation failure must not erase harness evidence or
                // make Orka treat completed agent work as never executed.
                if let Ok(mut diagnostics) = append_handle(&artifacts.diagnostics) {
                    let _ = writeln!(
                        diagnostics,
                        "orka: could not project Codex event journal: {error:#}"
                    );
                }
            }
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
            protocol: AgentProtocol::Plain,
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

    fn artifacts(dir: &Path, protocol: AgentProtocol) -> ExecutionArtifacts {
        ExecutionArtifacts {
            transcript: dir.join("transcript.log"),
            diagnostics: dir.join("diagnostics.log"),
            raw_events: (protocol == AgentProtocol::CodexJsonl)
                .then(|| dir.join("events.raw.jsonl")),
            events: (protocol == AgentProtocol::CodexJsonl).then(|| dir.join("events.v1.jsonl")),
            file_changes: (protocol == AgentProtocol::CodexJsonl)
                .then(|| dir.join("file-changes.v1.jsonl")),
            file_change_ref: (protocol == AgentProtocol::CodexJsonl)
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

        let artifacts = artifacts(&dir, AgentProtocol::Plain);
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
    fn codex_jsonl_is_journaled_normalized_and_rendered_as_a_transcript() {
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
        spec.protocol = AgentProtocol::CodexJsonl;
        let artifacts = artifacts(&dir, AgentProtocol::CodexJsonl);

        executor.run(&spec, &artifacts).unwrap();

        let raw = std::fs::read_to_string(artifacts.raw_events.unwrap()).unwrap();
        assert!(raw.contains("agent_message"));
        let normalized = std::fs::read_to_string(artifacts.events.unwrap()).unwrap();
        assert!(normalized.contains("agent_message"));
        let transcript = std::fs::read_to_string(artifacts.transcript).unwrap();
        assert!(transcript.contains("Finished cleanly"));
        assert_eq!(
            std::fs::read_to_string(artifacts.diagnostics).unwrap(),
            "to stderr\n"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn event_projection_failure_does_not_erase_process_evidence() {
        let dir = std::env::temp_dir().join(format!("orka-codex-events-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        std::fs::create_dir_all(dir.join("ctx")).unwrap();
        init_workspace_repository(&dir.join("ws"));
        let executor = DrivaExecutor::new(Box::new(StubBackend {
            seen: Arc::new(Mutex::new(Vec::new())),
            exit: 7,
            stdout: r#"{"type":"turn.completed","usage":{}}"#,
        }));
        let mut spec = spec(&dir);
        spec.protocol = AgentProtocol::CodexJsonl;
        let mut artifacts = artifacts(&dir, AgentProtocol::CodexJsonl);
        artifacts.events = Some(dir.join("missing/events.v1.jsonl"));

        let report = executor.run(&spec, &artifacts).unwrap();

        assert_eq!(report.exit_code, 7);
        assert!(std::fs::read_to_string(&artifacts.diagnostics)
            .unwrap()
            .contains("could not project Codex event journal"));
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
        let result = executor.run(&spec(&dir), &artifacts(&dir, AgentProtocol::Plain));
        assert!(result.is_err());
        assert!(seen.lock().unwrap().is_empty(), "backend never ran");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// End-to-end against real podman. Ignored by default: requires a working
    /// container engine and image. Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn podman_runs_a_real_isolated_command() {
        let dir = std::env::temp_dir().join(format!("orka-podman-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        let executor = DrivaExecutor::podman("podman", "docker.io/library/busybox:latest");
        let spec = ExecutionSpec {
            command: vec![
                "sh".into(),
                "-c".into(),
                "echo ran > /tmp/orka/workspace/out.txt".into(),
            ],
            protocol: AgentProtocol::Plain,
            working_directory: "/tmp/orka/workspace".into(),
            mounts: vec![MountSpec {
                source: dir.join("ws"),
                destination: "/tmp/orka/workspace".into(),
                writable: true,
            }],
            environment: BTreeMap::new(),
            network: false,
        };
        let report = executor
            .run(&spec, &artifacts(&dir, AgentProtocol::Plain))
            .unwrap();
        assert_eq!(report.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("ws/out.txt")).unwrap(),
            "ran\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
