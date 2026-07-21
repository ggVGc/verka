use crate::{
    effective_policy, ExecutionEvidence, ExecutionIo, ExecutionOutcome, ExecutionRequest,
    Isolation, MountAccess, ProcessExit,
};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::SystemTime;

/// A synchronous, disposable Podman isolation backend.
#[derive(Clone, Debug)]
pub struct PodmanIsolation {
    pub executable: PathBuf,
    pub image: String,
}

impl PodmanIsolation {
    pub fn command(&self, request: &ExecutionRequest) -> Command {
        let mut command = Command::new(&self.executable);
        command.arg("run").arg("--rm");
        if request.interactive {
            command.arg("-i").arg("-t");
        }
        if !request.network {
            command.arg("--network").arg("none");
        }
        command.arg("--workdir").arg(&request.working_directory);
        for mount in &request.mounts {
            let mut value = mount.source.as_os_str().to_os_string();
            value.push(":");
            value.push(&mount.destination);
            if mount.access == MountAccess::ReadOnly {
                value.push(":ro");
            }
            command.arg("--volume").arg(value);
        }
        for (key, value) in &request.environment {
            let mut assignment = key.clone();
            assignment.push("=");
            assignment.push(value);
            command.arg("--env").arg(assignment);
        }
        command.arg(&self.image).args(&request.command);
        command
    }
}

impl Isolation for PodmanIsolation {
    fn run(&self, request: &ExecutionRequest, io: ExecutionIo) -> Result<ExecutionOutcome> {
        let started_at = SystemTime::now();
        let status = self
            .command(request)
            .stdin(Stdio::from(io.stdin))
            .stdout(Stdio::from(io.stdout))
            .stderr(Stdio::from(io.stderr))
            .status()
            .with_context(|| format!("failed to start {}", self.executable.display()))?;
        Ok(ExecutionOutcome {
            exit: ProcessExit::from(status),
            evidence: ExecutionEvidence {
                isolation_backend: "podman".into(),
                effective_policy: effective_policy(request),
                started_at,
                finished_at: SystemTime::now(),
            },
        })
    }
}
