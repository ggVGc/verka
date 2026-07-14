//! The production [`IsolatedExecutor`] adapter over the Driva library.
//!
//! Orka's [`ExecutionSpec`] is translated into a Driva execution request and
//! run through `driva::execute`, which validates the grant (deny-by-default
//! mounts and networking) before invoking the backend. The command's combined
//! stdout/stderr streams into the attempt's transcript file as it runs, and
//! the returned report carries only harness-observed evidence.

use crate::executor::{ExecutionReport, ExecutionSpec, IsolatedExecutor};
use anyhow::{Context, Result};
use driva::{ExecutionIo, Isolation, Mount, MountAccess};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct DrivaExecutor {
    backend: Box<dyn Isolation>,
}

impl DrivaExecutor {
    pub fn new(backend: Box<dyn Isolation>) -> Self {
        Self { backend }
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
        tmpfs: Vec<std::path::PathBuf>,
    ) -> Self {
        Self::new(Box::new(driva::BwrapIsolation {
            executable: executable.into(),
            rootfs: rootfs.into(),
            tmpfs,
        }))
    }
}

impl IsolatedExecutor for DrivaExecutor {
    fn run(&self, spec: &ExecutionSpec, transcript: &Path) -> Result<ExecutionReport> {
        let request = driva::ExecutionRequest {
            command: spec.command.iter().map(OsString::from).collect(),
            working_directory: spec.working_directory.clone(),
            mounts: spec
                .mounts
                .iter()
                .map(|m| Mount {
                    source: m.source.clone(),
                    destination: m.destination.clone(),
                    access: if m.writable {
                        MountAccess::ReadWrite
                    } else {
                        MountAccess::ReadOnly
                    },
                })
                .collect(),
            environment: spec
                .environment
                .iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
            network: spec.network,
            interactive: false,
        };

        // Two independent append handles so stdout and stderr interleave in
        // one transcript instead of overwriting each other.
        std::fs::write(transcript, b"")
            .with_context(|| format!("creating transcript {}", transcript.display()))?;
        let io = ExecutionIo {
            stdin: File::open("/dev/null").context("opening /dev/null for agent stdin")?,
            stdout: append_handle(transcript)?,
            stderr: append_handle(transcript)?,
        };

        let outcome = driva::execute(self.backend.as_ref(), &request, io)?;
        Ok(ExecutionReport {
            backend: outcome.evidence.isolation_backend,
            backend_reference: outcome.evidence.backend_reference,
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
        .with_context(|| format!("opening transcript {}", path.display()))
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
    use std::sync::{Arc, Mutex};

    /// A backend that records the validated request it received and writes to
    /// the caller's streams the way a real command would. The request log is
    /// shared so the test keeps a handle after boxing the backend.
    struct StubBackend {
        seen: Arc<Mutex<Vec<ExecutionRequest>>>,
        exit: i32,
    }

    impl Isolation for StubBackend {
        fn run(&self, request: &ExecutionRequest, mut io: ExecutionIo) -> Result<ExecutionOutcome> {
            self.seen.lock().unwrap().push(request.clone());
            writeln!(io.stdout, "to stdout").unwrap();
            writeln!(io.stderr, "to stderr").unwrap();
            let now = SystemTime::now();
            Ok(ExecutionOutcome {
                exit: ProcessExit::Code(self.exit),
                evidence: driva::ExecutionEvidence {
                    isolation_backend: "stub".into(),
                    backend_reference: Some("stub-1".into()),
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
            working_directory: "/workspace".into(),
            mounts: vec![
                MountSpec {
                    source: dir.join("ws"),
                    destination: "/workspace".into(),
                    writable: true,
                },
                MountSpec {
                    source: dir.join("ctx"),
                    destination: "/context".into(),
                    writable: false,
                },
            ],
            environment: BTreeMap::from([("ORKA_OUTCOME".into(), "/orka/outcome.toml".into())]),
            network: false,
        }
    }

    #[test]
    fn the_grant_is_translated_verbatim_and_the_transcript_captures_both_streams() {
        let dir = std::env::temp_dir().join(format!("orka-driva-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        std::fs::create_dir_all(dir.join("ctx")).unwrap();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = DrivaExecutor::new(Box::new(StubBackend {
            seen: seen.clone(),
            exit: 3,
        }));

        let transcript = dir.join("transcript.log");
        let report = executor.run(&spec(&dir), &transcript).unwrap();
        assert_eq!(report.exit_code, 3);
        assert_eq!(report.backend, "stub");
        assert_eq!(report.backend_reference.as_deref(), Some("stub-1"));

        let text = std::fs::read_to_string(&transcript).unwrap();
        assert!(text.contains("to stdout") && text.contains("to stderr"));

        let seen = seen.lock().unwrap();
        let request = &seen[0];
        assert_eq!(request.command, vec!["agent", "--work"]);
        assert!(!request.network, "networking stays denied");
        assert!(!request.interactive);
        assert_eq!(request.mounts.len(), 2);
        assert_eq!(request.mounts[0].access, MountAccess::ReadWrite);
        assert_eq!(request.mounts[1].access, MountAccess::ReadOnly);
        // driva validated (canonicalised) the sources.
        assert_eq!(
            request.mounts[0].source,
            dir.join("ws").canonicalize().unwrap()
        );
        assert_eq!(
            request.environment.get(&OsString::from("ORKA_OUTCOME")),
            Some(&OsString::from("/orka/outcome.toml"))
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
        }));
        // `ws` and `ctx` were never created: validation must refuse the grant.
        let result = executor.run(&spec(&dir), &dir.join("transcript.log"));
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
                "echo ran > /workspace/out.txt".into(),
            ],
            working_directory: "/workspace".into(),
            mounts: vec![MountSpec {
                source: dir.join("ws"),
                destination: "/workspace".into(),
                writable: true,
            }],
            environment: BTreeMap::new(),
            network: false,
        };
        let report = executor.run(&spec, &dir.join("transcript.log")).unwrap();
        assert_eq!(report.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("ws/out.txt")).unwrap(),
            "ran\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
