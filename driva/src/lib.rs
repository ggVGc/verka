//! Portable policy and execution interface for isolated commands.

mod bwrap;
mod config;
mod docker;
mod podman;
mod runtime;

pub use bwrap::BwrapIsolation;
pub use config::{
    BwrapConfig, Config, DockerConfig, IsolationConfig, MountConfig, MountKind, NetworkConfig,
    PodmanConfig, TemplateConfig,
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
    /// Start the sandboxed process in a new terminal session, detaching it from
    /// the controlling terminal. Backends that support it (Bubblewrap's
    /// `--new-session`) enable this by default to block TIOCSTI input
    /// injection; disabling it keeps the caller's session.
    pub new_session: bool,
}

/// A filesystem made available inside an isolated execution.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Mount {
    /// A host path exposed at an isolated destination.
    Bind {
        source: PathBuf,
        destination: PathBuf,
        access: MountAccess,
    },
    /// An empty, writable filesystem discarded after execution.
    Temporary { destination: PathBuf },
}

impl Mount {
    pub fn destination(&self) -> &Path {
        match self {
            Self::Bind { destination, .. } | Self::Temporary { destination } => destination,
        }
    }

    pub fn make_read_only(&mut self) {
        if let Self::Bind { access, .. } = self {
            *access = MountAccess::ReadOnly;
        }
    }
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
        if let Mount::Temporary { destination } = mount {
            *destination = expand_home(destination, "temporary mount destination")?;
        }
        let destination = mount.destination();
        if !destination.is_absolute() {
            bail!(
                "mount destination must be absolute: {}",
                destination.display()
            );
        }
        if !destinations.insert(destination.to_path_buf()) {
            bail!("conflicting mount destination: {}", destination.display());
        }
        if let Mount::Bind { source, .. } = mount {
            *source = canonicalize_mount(source)
                .with_context(|| format!("invalid mount source {}", source.display()))?;
        }
    }
    Ok(validated)
}

pub(crate) fn canonicalize_mount(path: &Path) -> Result<PathBuf> {
    let expanded = expand_home(path, "mount source")?;
    Ok(expanded.canonicalize()?)
}

pub(crate) fn expand_home(path: &Path, label: &str) -> Result<PathBuf> {
    if path == Path::new("~") || path.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .with_context(|| format!("HOME is not set; cannot expand {label}"))?;
        Ok(PathBuf::from(home).join(path.strip_prefix("~").expect("prefix checked")))
    } else {
        Ok(path.to_path_buf())
    }
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
