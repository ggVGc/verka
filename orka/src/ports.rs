//! Orka's two narrow dependencies, as traits owned by Orka.
//!
//! Production adapters implement these over the Linka and Driva libraries;
//! tests substitute the fakes in [`crate::fakes`]. Every type crossing a port
//! is defined here, so orchestration logic never names a Linka or Driva type
//! and never reaches into either application's on-disk representation.
//!
//! The types are serde-serializable because frozen inputs and execution
//! reports are persisted verbatim in durable attempt records.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// --- work graph -------------------------------------------------------------

/// An opaque node identity in the work graph.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One unit of ready work, as listed for selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItem {
    pub id: NodeId,
    pub title: String,
}

/// The version identity of a node definition — two opaque content hashes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionFingerprint {
    pub metadata: String,
    pub description: String,
}

/// The version identity of a recorded result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultFingerprint {
    pub metadata: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A pinned reference to content in an artifact system (for git: a commit).
/// Stored with full fidelity so a submission compares exactly what freeze saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPin {
    pub scheme: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub repository: String,
    pub id: String,
}

/// A dependency's state as frozen for one attempt: exactly what the graph
/// pinned, plus the prose the agent may be given as context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenDependency {
    pub id: NodeId,
    pub definition: DefinitionFingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultFingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ArtifactPin>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub result_notes: String,
}

/// Everything an attempt is built against, captured at selection time. A
/// submission is accepted only while the graph still matches these versions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenInput {
    pub node: NodeId,
    pub definition: DefinitionFingerprint,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<FrozenDependency>,
    /// The exact project commit the work starts from.
    pub input_commit: String,
}

/// What the attempt concluded, as reported for graph submission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkOutcome {
    Succeeded {
        /// Project-relative paths of the declared outputs; empty for
        /// graph-only work.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        notes: String,
    },
    Failed {
        notes: String,
    },
}

/// A version-checked result submission.
#[derive(Clone, Debug)]
pub struct Submission {
    pub frozen: FrozenInput,
    pub outcome: WorkOutcome,
    /// The working tree holding the declared outputs (an execution worktree).
    /// `None` for outcomes that assert no output provenance.
    pub workspace: Option<PathBuf>,
}

/// The graph's answer to a submission. `Stale` is an answer, not an error:
/// the frozen input no longer describes the graph, and the attempt must not
/// silently complete the changed work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmitOutcome {
    Accepted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_commit: Option<String>,
    },
    Stale {
        reasons: Vec<String>,
    },
}

/// Reading ready work and pinned context; submitting version-checked results.
pub trait WorkGraph {
    /// Nodes whose work can be performed now, in selection order.
    fn select_ready(&self) -> Result<Vec<WorkItem>>;

    /// Capture the node's definition, dependency results, and project input
    /// version for a durable attempt.
    fn freeze(&self, id: &NodeId) -> Result<FrozenInput>;

    /// Record the outcome, refusing (as [`SubmitOutcome::Stale`]) when the
    /// graph no longer matches the submission's frozen input.
    fn submit(&self, submission: &Submission) -> Result<SubmitOutcome>;
}

// --- isolated executor --------------------------------------------------------

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

// --- workspace ---------------------------------------------------------------

/// An isolated working tree prepared for one attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedWorkspace {
    pub path: PathBuf,
    /// The candidate branch the workspace is checked out on.
    pub branch: String,
    pub input_commit: String,
}

/// What cleanup observed. A dirty workspace is retained, never discarded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupOutcome {
    Removed,
    RetainedDirty,
    AlreadyAbsent,
}

/// Preparing and cleaning isolated per-attempt working trees.
pub trait WorkspaceManager {
    /// Create a fresh working tree at `input_commit` on a candidate branch
    /// named for `attempt`. Fails if the workspace already exists.
    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace>;

    /// Remove a workspace whose attempt is sealed. Refuses to discard
    /// uncommitted changes, reporting `RetainedDirty` instead.
    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome>;
}
