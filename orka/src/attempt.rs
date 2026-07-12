//! Durable execution attempts.
//!
//! An attempt is written before any external side effect, so a crash at any
//! point leaves a record from which the remaining work can be classified and
//! recovered. State advances by adding files under the attempt directory —
//! nothing is rewritten except the idempotent seal:
//!
//! ```text
//! .orka/attempts/<attempt-id>/
//!     attempt.toml     frozen graph input (written at creation)
//!     workspace.toml   the planned workspace, before it is created
//!     prepared         marker: workspace creation completed
//!     request.toml     the exact execution spec, before the command starts
//!     transcript.log   streamed agent stdout/stderr (Orka's transcript)
//!     evidence.toml    harness-observed exit and backend evidence
//!     seal.toml        final state: how the attempt concluded
//!     io/              the exchange directory mounted into the environment
//! ```
//!
//! The phase of an attempt is derived from which files exist, never stored.

use crate::ports::{ExecutionReport, ExecutionSpec, FrozenInput, PreparedWorkspace};
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttemptId(pub String);

impl AttemptId {
    /// Mint a fresh attempt identity. The caller mints before creating the
    /// record so branch and workspace names can derive from it.
    pub fn new() -> Self {
        Self(format!("attempt-{}", ulid::Ulid::new()))
    }
}

impl Default for AttemptId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AttemptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Contents of `attempt.toml`: the work frozen for this attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub schema: u32,
    pub id: AttemptId,
    /// Unix milliseconds when the attempt was created.
    pub created_at_ms: i64,
    pub frozen: FrozenInput,
}

/// Contents of `seal.toml`: how the attempt concluded. Sealing is terminal
/// and idempotent; a sealed attempt never changes state again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SealRecord {
    pub schema: u32,
    pub sealed_at_ms: i64,
    #[serde(flatten)]
    pub state: SealedState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SealedState {
    /// The graph accepted the result.
    Submitted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_commit: Option<String>,
    },
    /// The graph moved between freeze and submit; nothing was recorded.
    StaleAtSubmit { reasons: Vec<String> },
    /// Failure evidence was recorded in the graph.
    FailureRecorded,
    /// Execution ended without a usable outcome; nothing was recorded.
    Interrupted { reason: String },
    /// The command exited zero but declared no outcome.
    ContractViolation { reason: String },
}

/// Where an attempt stands, derived from which record files exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptPhase {
    /// Frozen input recorded; no workspace chosen yet.
    Created,
    /// A workspace was planned; whether it exists on disk is unconfirmed.
    WorkspacePlanned,
    /// Workspace preparation completed.
    Prepared,
    /// The execution request was recorded; the command may have started.
    Requested,
    /// Exit evidence was captured; the outcome is not yet submitted.
    Executed,
    /// Terminal.
    Sealed,
}

/// Everything durably known about one attempt.
#[derive(Clone, Debug)]
pub struct AttemptSnapshot {
    pub record: AttemptRecord,
    pub workspace: Option<PreparedWorkspace>,
    pub prepared: bool,
    pub request: Option<ExecutionSpec>,
    pub evidence: Option<ExecutionReport>,
    pub seal: Option<SealRecord>,
}

impl AttemptSnapshot {
    pub fn phase(&self) -> AttemptPhase {
        if self.seal.is_some() {
            AttemptPhase::Sealed
        } else if self.evidence.is_some() {
            AttemptPhase::Executed
        } else if self.request.is_some() {
            AttemptPhase::Requested
        } else if self.prepared {
            AttemptPhase::Prepared
        } else if self.workspace.is_some() {
            AttemptPhase::WorkspacePlanned
        } else {
            AttemptPhase::Created
        }
    }
}

/// The filesystem attempt store: Orka's own durable namespace, owned by Orka
/// alone (conventionally `.orka/` in the workbench, beside the graph store it
/// never reaches into).
pub struct FsAttemptStore {
    root: PathBuf,
}

impl FsAttemptStore {
    /// `root` is the store directory itself (e.g. `<workbench>/.orka`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn attempt_dir(&self, id: &AttemptId) -> PathBuf {
        self.root.join("attempts").join(&id.0)
    }

    pub fn transcript_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("transcript.log")
    }

    /// The per-attempt exchange directory mounted into the isolated
    /// environment (prompt in, declared outcome out), created on demand.
    pub fn io_dir(&self, id: &AttemptId) -> Result<PathBuf> {
        let dir = self.attempt_dir(id).join("io");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating io directory for {id}"))?;
        Ok(dir)
    }

    /// Durably record a new attempt. Refuses an existing id: attempts are
    /// never reused.
    pub fn create(&self, id: &AttemptId, frozen: &FrozenInput) -> Result<AttemptRecord> {
        let dir = self.attempt_dir(id);
        if dir.exists() {
            bail!("attempt `{id}` already exists");
        }
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let record = AttemptRecord {
            schema: 1,
            id: id.clone(),
            created_at_ms: now_millis(),
            frozen: frozen.clone(),
        };
        write_toml(&dir.join("attempt.toml"), &record)?;
        Ok(record)
    }

    /// Record the chosen workspace before creating it, so a crash mid-creation
    /// leaves the plan (branch name, path) discoverable.
    pub fn plan_workspace(&self, id: &AttemptId, workspace: &PreparedWorkspace) -> Result<()> {
        self.require(id)?;
        write_toml(&self.attempt_dir(id).join("workspace.toml"), workspace)
    }

    /// Record that workspace creation completed.
    pub fn mark_prepared(&self, id: &AttemptId) -> Result<()> {
        self.require(id)?;
        write_atomic(&self.attempt_dir(id).join("prepared"), b"")
    }

    /// Record the exact execution request before the command starts.
    pub fn record_request(&self, id: &AttemptId, spec: &ExecutionSpec) -> Result<()> {
        self.require(id)?;
        write_toml(&self.attempt_dir(id).join("request.toml"), spec)
    }

    /// Record harness-observed exit evidence.
    pub fn record_evidence(&self, id: &AttemptId, report: &ExecutionReport) -> Result<()> {
        self.require(id)?;
        write_toml(&self.attempt_dir(id).join("evidence.toml"), report)
    }

    /// Seal the attempt. Idempotent: re-sealing with the same state is a
    /// no-op; a different state is refused — sealed history never changes.
    pub fn seal(&self, id: &AttemptId, state: SealedState) -> Result<SealRecord> {
        self.require(id)?;
        let path = self.attempt_dir(id).join("seal.toml");
        if path.exists() {
            let existing: SealRecord = read_toml(&path)?;
            if existing.state == state {
                return Ok(existing);
            }
            bail!(
                "attempt `{id}` is already sealed as {:?}; refusing to reseal as {:?}",
                existing.state,
                state
            );
        }
        let record = SealRecord {
            schema: 1,
            sealed_at_ms: now_millis(),
            state,
        };
        write_toml(&path, &record)?;
        Ok(record)
    }

    /// Load everything durably recorded about an attempt. Unreadable record
    /// files are errors, never silently degraded phases.
    pub fn load(&self, id: &AttemptId) -> Result<AttemptSnapshot> {
        let dir = self.attempt_dir(id);
        let record: AttemptRecord = read_toml(&dir.join("attempt.toml"))
            .with_context(|| format!("unknown or unreadable attempt `{id}`"))?;
        Ok(AttemptSnapshot {
            record,
            workspace: read_toml_optional(&dir.join("workspace.toml"))?,
            prepared: dir.join("prepared").exists(),
            request: read_toml_optional(&dir.join("request.toml"))?,
            evidence: read_toml_optional(&dir.join("evidence.toml"))?,
            seal: read_toml_optional(&dir.join("seal.toml"))?,
        })
    }

    /// All recorded attempt ids, oldest first (ids are ULID-ordered).
    pub fn list(&self) -> Result<Vec<AttemptId>> {
        let dir = self.root.join("attempts");
        let mut ids = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ids.push(AttemptId(entry.file_name().to_string_lossy().into_owned()));
            }
        }
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(ids)
    }

    fn require(&self, id: &AttemptId) -> Result<()> {
        if !self.attempt_dir(id).join("attempt.toml").is_file() {
            bail!("unknown attempt `{id}`");
        }
        Ok(())
    }
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = toml::to_string_pretty(value)
        .with_context(|| format!("serialising {}", path.display()))?;
    write_atomic(path, text.as_bytes())
}

/// Write via a temp file and rename, so a crash never leaves a half-written
/// record that would read as corruption.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("no parent directory for {}", path.display()))?;
    let temp = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&temp, bytes).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path)
        .with_context(|| format!("committing {}", path.display()))?;
    Ok(())
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn read_toml_optional<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{DefinitionFingerprint, MountSpec, NodeId};
    use std::collections::BTreeMap;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (TempDir, FsAttemptStore) {
        let dir = std::env::temp_dir().join(format!("orka-attempt-test-{}", ulid::Ulid::new()));
        (TempDir(dir.clone()), FsAttemptStore::new(dir.join(".orka")))
    }

    fn frozen() -> FrozenInput {
        FrozenInput {
            node: NodeId("node-1".into()),
            definition: DefinitionFingerprint {
                metadata: "m".into(),
                description: "d".into(),
            },
            description: "Do the thing".into(),
            dependencies: vec![],
            input_commit: "c0ffee".into(),
        }
    }

    fn workspace() -> PreparedWorkspace {
        PreparedWorkspace {
            path: "/tmp/ws".into(),
            branch: "orka/attempts/attempt-x".into(),
            input_commit: "c0ffee".into(),
        }
    }

    fn spec() -> ExecutionSpec {
        ExecutionSpec {
            command: vec!["agent".into()],
            working_directory: "/workspace".into(),
            mounts: vec![MountSpec {
                source: "/tmp/ws".into(),
                destination: "/workspace".into(),
                writable: true,
            }],
            environment: BTreeMap::new(),
            network: false,
        }
    }

    fn report() -> ExecutionReport {
        ExecutionReport {
            backend: "fake".into(),
            backend_reference: None,
            exit_code: 0,
            started_at_ms: 1,
            finished_at_ms: 2,
        }
    }

    /// The lifecycle stops cleanly at every step: whatever was durably written
    /// before a crash classifies the attempt, and loading never guesses.
    #[test]
    fn phase_is_derived_from_the_files_present_at_each_step() {
        let (_temp, store) = store();
        let id = AttemptId::new();

        assert!(store.load(&id).is_err());
        store.create(&id, &frozen()).unwrap();
        assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Created);

        store.plan_workspace(&id, &workspace()).unwrap();
        assert_eq!(
            store.load(&id).unwrap().phase(),
            AttemptPhase::WorkspacePlanned
        );

        store.mark_prepared(&id).unwrap();
        assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Prepared);

        store.record_request(&id, &spec()).unwrap();
        assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Requested);

        store.record_evidence(&id, &report()).unwrap();
        assert_eq!(store.load(&id).unwrap().phase(), AttemptPhase::Executed);

        store
            .seal(&id, SealedState::Submitted { output_commit: Some("beef".into()) })
            .unwrap();
        let snapshot = store.load(&id).unwrap();
        assert_eq!(snapshot.phase(), AttemptPhase::Sealed);

        // The snapshot carries every frozen fact back out.
        assert_eq!(snapshot.record.frozen, frozen());
        assert_eq!(snapshot.workspace.unwrap(), workspace());
        assert_eq!(snapshot.request.unwrap(), spec());
        assert_eq!(snapshot.evidence.unwrap(), report());
        assert_eq!(
            snapshot.seal.unwrap().state,
            SealedState::Submitted { output_commit: Some("beef".into()) }
        );
    }

    #[test]
    fn attempts_are_never_reused_and_ids_list_in_order() {
        let (_temp, store) = store();
        let id = AttemptId::new();
        store.create(&id, &frozen()).unwrap();
        assert!(store.create(&id, &frozen()).is_err());

        let second = AttemptId::new();
        store.create(&second, &frozen()).unwrap();
        assert_eq!(store.list().unwrap(), vec![id, second]);
    }

    #[test]
    fn listing_an_uninitialised_store_is_empty_not_an_error() {
        let (_temp, store) = store();
        assert_eq!(store.list().unwrap(), vec![]);
    }

    #[test]
    fn sealing_is_idempotent_but_never_rewrites_history() {
        let (_temp, store) = store();
        let id = AttemptId::new();
        store.create(&id, &frozen()).unwrap();

        let first = store
            .seal(&id, SealedState::Interrupted { reason: "backend died".into() })
            .unwrap();
        let again = store
            .seal(&id, SealedState::Interrupted { reason: "backend died".into() })
            .unwrap();
        assert_eq!(first, again);

        let conflict = store.seal(&id, SealedState::FailureRecorded);
        assert!(conflict.is_err());
        assert_eq!(
            store.load(&id).unwrap().seal.unwrap().state,
            SealedState::Interrupted { reason: "backend died".into() }
        );
    }

    #[test]
    fn corrupt_records_are_errors_not_degraded_phases() {
        let (_temp, store) = store();
        let id = AttemptId::new();
        store.create(&id, &frozen()).unwrap();
        let dir = store.root().join("attempts").join(&id.0);
        std::fs::write(dir.join("evidence.toml"), "not toml [").unwrap();
        assert!(store.load(&id).is_err());
    }

    #[test]
    fn steps_require_a_created_attempt() {
        let (_temp, store) = store();
        let id = AttemptId::new();
        assert!(store.plan_workspace(&id, &workspace()).is_err());
        assert!(store.mark_prepared(&id).is_err());
        assert!(store.record_request(&id, &spec()).is_err());
        assert!(store.record_evidence(&id, &report()).is_err());
        assert!(store.seal(&id, SealedState::FailureRecorded).is_err());
    }
}
