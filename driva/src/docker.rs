use crate::{
    effective_policy, ExecutionEvidence, ExecutionIo, ExecutionOutcome, ExecutionRequest,
    Isolation, MountAccess, ProcessExit,
};
use crate::{
    BackendReference, DiscoveredResource, DurableIsolation, ObservedProcessState,
    ProcessConnection, SessionId,
};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct DockerIsolation {
    pub executable: PathBuf,
    pub image: String,
}

impl DurableIsolation for DockerIsolation {
    fn backend_name(&self) -> &'static str {
        "docker"
    }
    fn start(&self, id: &SessionId, r: &ExecutionRequest) -> Result<BackendReference> {
        let old = self.command(r);
        let args: Vec<_> = old.get_args().map(|x| x.to_os_string()).collect();
        let mut c = Command::new(&self.executable);
        c.arg("run")
            .arg("--detach")
            .arg("--name")
            .arg(format!("driva-{}", id.0))
            .arg("--label")
            .arg("io.driva.managed=true")
            .arg("--label")
            .arg(format!("io.driva.session={}", id.0));
        for a in args.into_iter().skip(2) {
            c.arg(a);
        }
        let o = c.output()?;
        if !o.status.success() {
            anyhow::bail!(
                "docker start failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )
        }
        Ok(BackendReference(
            String::from_utf8_lossy(&o.stdout).trim().into(),
        ))
    }
    fn find(&self, id: &SessionId) -> Result<Option<BackendReference>> {
        crate::podman::find_engine(&self.executable, id)
    }
    fn enumerate_managed(&self) -> Result<Vec<DiscoveredResource>> {
        crate::podman::enumerate_engine(&self.executable)
    }
    fn inspect(&self, r: &BackendReference) -> Result<ObservedProcessState> {
        crate::podman::inspect_engine(&self.executable, r)
    }
    fn attach(&self, r: &BackendReference) -> Result<Box<dyn ProcessConnection>> {
        Ok(crate::podman::attach_engine(&self.executable, r))
    }
    fn resume(&self, r: &BackendReference) -> Result<Box<dyn ProcessConnection>> {
        Ok(crate::podman::resume_engine(&self.executable, r))
    }
    fn wait(&self, r: &BackendReference) -> Result<ProcessExit> {
        crate::podman::wait_engine(&self.executable, r)
    }
    fn terminate(&self, r: &BackendReference, g: Duration) -> Result<()> {
        crate::podman::engine_ok(
            Command::new(&self.executable)
                .arg("stop")
                .arg("--time")
                .arg(g.as_secs().to_string())
                .arg(&r.0),
        )
    }
    fn remove(&self, r: &BackendReference) -> Result<()> {
        crate::podman::engine_ok(
            Command::new(&self.executable)
                .arg("rm")
                .arg("--force")
                .arg(&r.0),
        )
    }
}

impl DockerIsolation {
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

impl Isolation for DockerIsolation {
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
                isolation_backend: "docker".into(),
                backend_reference: None,
                effective_policy: effective_policy(request),
                started_at,
                finished_at: SystemTime::now(),
            },
        })
    }
}
