use crate::{
    effective_policy, ExecutionEvidence, ExecutionIo, ExecutionOutcome, ExecutionRequest,
    Isolation, MountAccess, ProcessExit,
};
use crate::{
    BackendReference, DurableIsolation, ObservedProcessState, ProcessConnection, SessionId,
};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::time::SystemTime;

/// A synchronous, disposable Podman isolation backend.
#[derive(Clone, Debug)]
pub struct PodmanIsolation {
    pub executable: PathBuf,
    pub image: String,
}

struct PodmanConnection {
    executable: PathBuf,
    reference: BackendReference,
}
impl ProcessConnection for PodmanConnection {
    fn connect(self: Box<Self>, io: ExecutionIo) -> Result<ProcessExit> {
        let status = Command::new(&self.executable)
            .arg("attach")
            .arg(&self.reference.0)
            .stdin(Stdio::from(io.stdin))
            .stdout(Stdio::from(io.stdout))
            .stderr(Stdio::from(io.stderr))
            .status()?;
        Ok(status.into())
    }
}

impl DurableIsolation for PodmanIsolation {
    fn backend_name(&self) -> &'static str {
        "podman"
    }
    fn start(&self, id: &SessionId, request: &ExecutionRequest) -> Result<BackendReference> {
        let mut c = self.command(request);
        let args: Vec<_> = c.get_args().map(|x| x.to_os_string()).collect();
        c = Command::new(&self.executable);
        c.arg("run")
            .arg("--detach")
            .arg("--name")
            .arg(format!("driva-{}", id.0))
            .arg("--label")
            .arg(format!("io.driva.session={}", id.0));
        for a in args.into_iter().skip(2) {
            c.arg(a);
        }
        let o = c.output()?;
        if !o.status.success() {
            anyhow::bail!(
                "podman start failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )
        }
        Ok(BackendReference(
            String::from_utf8_lossy(&o.stdout).trim().into(),
        ))
    }
    fn find(&self, id: &SessionId) -> Result<Option<BackendReference>> {
        find_engine(&self.executable, id)
    }
    fn inspect(&self, r: &BackendReference) -> Result<ObservedProcessState> {
        inspect_engine(&self.executable, r)
    }
    fn attach(&self, r: &BackendReference) -> Result<Box<dyn ProcessConnection>> {
        Ok(Box::new(PodmanConnection {
            executable: self.executable.clone(),
            reference: r.clone(),
        }))
    }
    fn wait(&self, r: &BackendReference) -> Result<ProcessExit> {
        wait_engine(&self.executable, r)
    }
    fn terminate(&self, r: &BackendReference, g: Duration) -> Result<()> {
        engine_ok(
            Command::new(&self.executable)
                .arg("stop")
                .arg("--time")
                .arg(g.as_secs().to_string())
                .arg(&r.0),
        )
    }
    fn remove(&self, r: &BackendReference) -> Result<()> {
        engine_ok(
            Command::new(&self.executable)
                .arg("rm")
                .arg("--force")
                .arg(&r.0),
        )
    }
}

pub(crate) fn engine_ok(c: &mut Command) -> Result<()> {
    let o = c.output()?;
    if !o.status.success() {
        anyhow::bail!(
            "container engine failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )
    }
    Ok(())
}
pub(crate) fn find_engine(exe: &PathBuf, id: &SessionId) -> Result<Option<BackendReference>> {
    let o = Command::new(exe)
        .args([
            "ps",
            "-aq",
            "--filter",
            &format!("label=io.driva.session={}", id.0),
        ])
        .output()?;
    if !o.status.success() {
        anyhow::bail!(
            "container lookup failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )
    }
    let s = String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    Ok((!s.is_empty()).then_some(BackendReference(s)))
}
pub(crate) fn inspect_engine(exe: &PathBuf, r: &BackendReference) -> Result<ObservedProcessState> {
    let o = Command::new(exe)
        .args([
            "inspect",
            "--format",
            "{{.State.Status}} {{.State.ExitCode}}",
            &r.0,
        ])
        .output()?;
    if !o.status.success() {
        return Ok(ObservedProcessState::Missing);
    }
    let text = String::from_utf8_lossy(&o.stdout);
    let mut p = text.split_whitespace();
    Ok(match p.next().unwrap_or("") {
        "created" => ObservedProcessState::Created,
        "running" | "restarting" | "paused" => ObservedProcessState::Running,
        "exited" | "dead" => ObservedProcessState::Exited(ProcessExit::Code(
            p.next().unwrap_or("1").parse().unwrap_or(1),
        )),
        s => ObservedProcessState::Unknown {
            error: format!("unrecognized backend state {s:?}"),
        },
    })
}
pub(crate) fn wait_engine(exe: &PathBuf, r: &BackendReference) -> Result<ProcessExit> {
    let o = Command::new(exe).arg("wait").arg(&r.0).output()?;
    if !o.status.success() {
        anyhow::bail!(
            "container wait failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )
    }
    Ok(ProcessExit::Code(
        String::from_utf8_lossy(&o.stdout).trim().parse()?,
    ))
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
                backend_reference: None,
                effective_policy: effective_policy(request),
                started_at,
                finished_at: SystemTime::now(),
            },
        })
    }
}
