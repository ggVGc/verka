//! Portable policy and execution interface for isolated commands.

mod bwrap;
mod config;
mod docker;
mod podman;
mod runtime;

pub use bwrap::BwrapIsolation;
pub use config::{
    BwrapConfig, Config, DockerConfig, IsolationConfig, MountConfig, NetworkConfig, PodmanConfig,
    TemplateConfig,
};
pub use docker::DockerIsolation;
pub use podman::PodmanIsolation;
pub use runtime::{RuntimeSpec, RuntimeStore};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::SystemTime;

/// Conventional executable search path used when an isolated request does not
/// provide its own PATH.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionRequest {
    pub command: Vec<OsString>,
    pub working_directory: PathBuf,
    pub mounts: Vec<Mount>,
    pub environment: BTreeMap<OsString, OsString>,
    pub network: bool,
    pub interactive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Mount {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub access: MountAccess,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MountAccess {
    #[default]
    #[serde(alias = "read", alias = "ro")]
    ReadOnly,
    #[serde(alias = "write", alias = "rw")]
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EffectivePolicy {
    pub working_directory: PathBuf,
    pub mounts: Vec<Mount>,
    pub network: bool,
    pub interactive: bool,
}

#[derive(Clone, Debug)]
pub struct ExecutionOutcome {
    pub exit: ProcessExit,
    pub evidence: ExecutionEvidence,
}

#[derive(Clone, Debug)]
pub struct ExecutionEvidence {
    pub isolation_backend: String,
    pub effective_policy: EffectivePolicy,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ProcessExit {
    Code(i32),
    Signaled,
}

impl ProcessExit {
    pub fn code(&self) -> i32 {
        match self {
            Self::Code(code) => *code,
            Self::Signaled => 128,
        }
    }
}

impl From<ExitStatus> for ProcessExit {
    fn from(status: ExitStatus) -> Self {
        status.code().map(Self::Code).unwrap_or(Self::Signaled)
    }
}

/// Standard streams passed to an isolation backend.
pub struct ExecutionIo {
    pub stdin: File,
    pub stdout: File,
    pub stderr: File,
}

impl ExecutionIo {
    pub fn inherited() -> Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                stdin: File::open("/dev/stdin")?,
                stdout: File::options().write(true).open("/dev/stdout")?,
                stderr: File::options().write(true).open("/dev/stderr")?,
            })
        }
        #[cfg(not(unix))]
        bail!("inherited standard streams are not supported on this platform")
    }
}

pub trait Isolation {
    fn run(&self, request: &ExecutionRequest, io: ExecutionIo) -> Result<ExecutionOutcome>;
}

/// Resolve mount sources and enforce Driva's portable policy.
pub fn validate_request(request: &ExecutionRequest) -> Result<ExecutionRequest> {
    if request.command.is_empty() || request.command[0].is_empty() {
        bail!("an executable command is required");
    }
    if !request.working_directory.is_absolute() {
        bail!(
            "isolated working directory must be absolute: {}",
            request.working_directory.display()
        );
    }

    let mut validated = request.clone();
    let mut destinations = HashSet::new();
    for mount in &mut validated.mounts {
        if !mount.destination.is_absolute() {
            bail!(
                "mount destination must be absolute: {}",
                mount.destination.display()
            );
        }
        if !destinations.insert(mount.destination.clone()) {
            bail!(
                "conflicting mount destination: {}",
                mount.destination.display()
            );
        }
        mount.source = canonicalize_mount(&mount.source)
            .with_context(|| format!("invalid mount source {}", mount.source.display()))?;
    }
    Ok(validated)
}

fn canonicalize_mount(path: &Path) -> Result<PathBuf> {
    let expanded = if path == Path::new("~") || path.starts_with("~/") {
        let home =
            std::env::var_os("HOME").context("HOME is not set; cannot expand mount source")?;
        PathBuf::from(home).join(path.strip_prefix("~").expect("prefix checked"))
    } else {
        path.to_path_buf()
    };
    Ok(expanded.canonicalize()?)
}

pub fn effective_policy(request: &ExecutionRequest) -> EffectivePolicy {
    EffectivePolicy {
        working_directory: request.working_directory.clone(),
        mounts: request.mounts.clone(),
        network: request.network,
        interactive: request.interactive,
    }
}

/// Validate once at the application boundary before calling the backend.
pub fn execute(
    backend: &dyn Isolation,
    request: &ExecutionRequest,
    io: ExecutionIo,
) -> Result<ExecutionOutcome> {
    backend.run(&validate_request(request)?, io)
}
