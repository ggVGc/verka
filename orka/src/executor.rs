//! The isolated-execution boundary.
//!
//! Running an agent command behind a concrete capability grant is genuinely
//! replaceable — the production adapter wraps Driva, tests substitute
//! [`crate::fakes::FakeExecutor`] — so it stays a narrow Orka-owned trait.
//! Every type crossing it is serde-serializable because the execution request
//! and its harness-observed report are persisted verbatim in attempt records.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    /// Working directory inside the isolated environment.
    pub working_directory: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<MountSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub network: bool,
}

/// Harness-observed evidence of one finished execution. This is what Orka
/// trusts about backend, timing, and exit — never agent claims.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_reference: Option<String>,
    pub exit_code: i32,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
}

/// Running a command with a concrete filesystem and network capability grant.
pub trait IsolatedExecutor {
    /// Run the command to completion, streaming its combined stdout/stderr to
    /// `transcript` as it runs (so a crash retains the partial transcript).
    fn run(&self, spec: &ExecutionSpec, transcript: &Path) -> Result<ExecutionReport>;
}
