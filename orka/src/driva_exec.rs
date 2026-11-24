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
                // Orka runs on a prepared rootfs, which brings its own system;
                // the base applies to a private root only.
                base: driva::BaseConfig::empty(),
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
mod tests;
