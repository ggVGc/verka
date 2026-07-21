//! The isolated-execution boundary.
//!
//! Running an agent command behind a concrete capability grant is genuinely
//! replaceable — the production adapter wraps Driva, tests substitute
//! [`crate::fakes::FakeExecutor`] — so it stays a narrow Orka-owned trait.
//! Every type crossing it is serde-serializable because the execution request
//! and its harness-observed report are persisted verbatim in attempt records.

use crate::agent::AgentProtocol;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One capability crossing the isolation boundary: a host path visible inside
/// the environment. Read-only unless explicitly writable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountSpec {
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub writable: bool,
}

/// The concrete command and capability grant chosen for one execution. This
/// is the whole grant: nothing is mounted, networked, or inherited implicitly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSpec {
    pub command: Vec<String>,
    #[serde(default)]
    pub protocol: AgentProtocol,
    /// Working directory inside the isolated environment.
    pub working_directory: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<MountSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub network: bool,
}

/// Durable destinations for all streams produced by one execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionArtifacts {
    pub transcript: PathBuf,
    pub diagnostics: PathBuf,
    pub raw_events: Option<PathBuf>,
    pub events: Option<PathBuf>,
    /// Durable, harness-observed project file accesses for this execution.
    pub accesses: PathBuf,
}

/// Harness-observed evidence of one finished execution. This is what Orka
/// trusts about backend, timing, and exit — never agent claims.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub backend: String,
    pub exit_code: i32,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
}

/// Running a command with a concrete filesystem and network capability grant.
pub trait IsolatedExecutor {
    /// Run the command to completion, streaming output to durable attempt
    /// artifacts. Driva only transports these streams; this adapter applies
    /// the Orka-selected agent protocol.
    fn run(&self, spec: &ExecutionSpec, artifacts: &ExecutionArtifacts) -> Result<ExecutionReport>;
}
