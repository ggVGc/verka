//! Durable execution attempts.
//!
//! An attempt is written before any external side effect, so a crash at any
//! point leaves a record from which the remaining work can be classified and
//! recovered. State advances by adding files under the attempt directory —
//! nothing is rewritten except the idempotent seal:
//!
//! ```text
//! .orka/attempts/<attempt-id>/
//!     attempt.toml     the durable attempt input (written at creation)
//!     workspace.toml   the planned workspace, before it is created
//!     prepared         marker: workspace creation completed
//!     request.toml     the exact execution spec, before the command starts
//!     events.raw.jsonl exact machine-readable agent stdout, for event-stream agents
//!     file-changes.v1.jsonl event-to-Git-checkpoint mappings
//!     accesses.v1.jsonl harness-observed project file reads
//!     diagnostics.log agent stderr, kept outside the event stream
//!     transcript.log   raw agent stdout, for plain-stdout agents
//!     evidence.toml    harness-observed exit and backend evidence
//!     seal.toml        final state: how the attempt concluded
//!     io/              the exchange directory mounted into the environment
//! ```
//!
//! The phase of an attempt is derived from which files exist, never stored.

use crate::agent::OutputFormat;
use crate::executor::{ExecutionArtifacts, ExecutionReport, ExecutionSpec};
use crate::input::AttemptInput;
use crate::workspace::PreparedWorkspace;
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The current attempt-record schema.
pub const ATTEMPT_SCHEMA: u32 = 2;
pub const SEAL_SCHEMA: u32 = 1;

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

/// Contents of `attempt.toml`: the durable input to this attempt — Linka's
/// authoritative work snapshot plus the prompt prose Orka owns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecord {
    pub schema: u32,
    pub id: AttemptId,
    /// Unix milliseconds when the attempt was created.
    pub created_at_ms: i64,
    pub input: AttemptInput,
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
    /// The graph moved between snapshot and submit; nothing was recorded.
    /// The structured conflicts are Linka's, persisted verbatim.
    StaleAtSubmit {
        conflicts: Vec<linka::SubmissionConflict>,
    },
    /// Failure evidence was recorded in the graph.
    FailureRecorded,
    /// Execution ended without a usable outcome; nothing was recorded.
    Interrupted { reason: String },
    /// Orka could no longer prove that the execution repository was the exact
    /// workspace it prepared. This is infrastructure failure: no node result
    /// or project output may be recorded from the attempt.
    WorkspaceIntegrityFailure { reason: String },
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

    pub fn contains(&self, id: &AttemptId) -> bool {
        self.attempt_dir(id).join("attempt.toml").is_file()
    }

    fn attempt_dir(&self, id: &AttemptId) -> PathBuf {
        self.root.join("attempts").join(&id.0)
    }

    pub fn transcript_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("transcript.log")
    }

    pub fn attempt_record_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("attempt.toml")
    }

    pub fn request_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("request.toml")
    }

    pub fn evidence_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("evidence.toml")
    }

    pub fn diagnostics_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("diagnostics.log")
    }

    pub fn raw_events_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("events.raw.jsonl")
    }

    pub fn file_changes_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("file-changes.v1.jsonl")
    }

    pub fn accesses_path(&self, id: &AttemptId) -> PathBuf {
        self.attempt_dir(id).join("accesses.v1.jsonl")
    }

    pub fn execution_artifacts(
        &self,
        id: &AttemptId,
        protocol: OutputFormat,
    ) -> ExecutionArtifacts {
        ExecutionArtifacts {
            transcript: self.transcript_path(id),
            diagnostics: self.diagnostics_path(id),
            raw_events: protocol.is_agent().then(|| self.raw_events_path(id)),
            file_changes: protocol
                .records_file_changes()
                .then(|| self.file_changes_path(id)),
            file_change_ref: protocol
                .records_file_changes()
                .then(|| format!("refs/orka/file-changes/{}", id.0)),
            accesses: self.accesses_path(id),
        }
    }

    /// The per-attempt exchange directory mounted into the isolated
    /// environment (prompt in, declared outcome out), created on demand.
    pub fn io_dir(&self, id: &AttemptId) -> Result<PathBuf> {
        let dir = self.attempt_dir(id).join("io");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating io directory for {id}"))?;
        Ok(dir)
    }

    /// Durably record a new attempt. Refuses an existing id: attempts are
    /// never reused.
    pub fn create(&self, id: &AttemptId, input: &AttemptInput) -> Result<AttemptRecord> {
        let dir = self.attempt_dir(id);
        if dir.exists() {
            bail!("attempt `{id}` already exists");
        }
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let record = AttemptRecord {
            schema: ATTEMPT_SCHEMA,
            id: id.clone(),
            created_at_ms: now_millis(),
            input: input.clone(),
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
            if existing.schema != SEAL_SCHEMA {
                bail!(
                    "attempt `{id}` seal uses unsupported schema {}",
                    existing.schema
                );
            }
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
            schema: SEAL_SCHEMA,
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
        if record.schema != ATTEMPT_SCHEMA {
            bail!(
                "attempt `{id}` uses unsupported schema {} (this build reads schema {ATTEMPT_SCHEMA})",
                record.schema
            );
        }
        let seal: Option<SealRecord> = read_toml_optional(&dir.join("seal.toml"))?;
        if let Some(seal) = &seal {
            if seal.schema != SEAL_SCHEMA {
                bail!(
                    "attempt `{id}` seal uses unsupported schema {}",
                    seal.schema
                );
            }
        }
        Ok(AttemptSnapshot {
            record,
            workspace: read_toml_optional(&dir.join("workspace.toml"))?,
            prepared: dir.join("prepared").exists(),
            request: read_toml_optional(&dir.join("request.toml"))?,
            evidence: read_toml_optional(&dir.join("evidence.toml"))?,
            seal,
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

    /// Inspect every attempt directory without stopping at the first damaged
    /// record. This catches the directory-only crash window during creation as
    /// well as malformed or internally contradictory lifecycle records.
    pub fn audit(&self) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        for id in self.list()? {
            let valid_name =
                id.0.strip_prefix("attempt-")
                    .is_some_and(|value| ulid::Ulid::from_string(value).is_ok());
            if !valid_name {
                problems.push(format!("{id}: invalid attempt directory name"));
            }
            let snapshot = match self.load(&id) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    problems.push(format!("{id}: unreadable attempt record: {error:#}"));
                    continue;
                }
            };
            if snapshot.record.id != id {
                problems.push(format!(
                    "{id}: attempt record identifies itself as {}",
                    snapshot.record.id
                ));
            }
            if snapshot.prepared && snapshot.workspace.is_none() {
                problems.push(format!(
                    "{id}: prepared marker exists without a workspace plan"
                ));
            }
            if snapshot.request.is_some() && !snapshot.prepared {
                problems.push(format!(
                    "{id}: execution request exists without a prepared marker"
                ));
            }
            if snapshot.evidence.is_some() && snapshot.request.is_none() {
                problems.push(format!(
                    "{id}: exit evidence exists without an execution request"
                ));
            }
        }
        Ok(problems)
    }

    /// Remove an unsealed attempt that never acquired exit evidence. This is
    /// used only after its unchanged workspace and branch have been rolled back.
    pub fn discard_without_evidence(&self, id: &AttemptId) -> Result<()> {
        let snapshot = self.load(id)?;
        if snapshot.evidence.is_some() || snapshot.seal.is_some() {
            bail!("refusing to discard durable attempt `{id}`");
        }
        let dir = self.attempt_dir(id);
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("discarding pre-evidence attempt `{id}`"))
    }

    /// Remove local recovery state after Linka has durably accepted the work
    /// conclusion and the attempt workspace is safely gone. Accepted evidence
    /// lives in Linka attachments; `.orka` is not an archive.
    pub fn discard_registered(&self, id: &AttemptId) -> Result<()> {
        let snapshot = self.load(id)?;
        let registered = snapshot.seal.as_ref().is_some_and(|seal| {
            matches!(
                seal.state,
                SealedState::Submitted { .. } | SealedState::FailureRecorded
            )
        });
        if !registered {
            bail!("refusing to discard unregistered attempt `{id}`");
        }
        std::fs::remove_dir_all(self.attempt_dir(id))
            .with_context(|| format!("discarding registered attempt `{id}`"))
    }

    fn require(&self, id: &AttemptId) -> Result<()> {
        if !self.attempt_dir(id).join("attempt.toml").is_file() {
            bail!("unknown attempt `{id}`");
        }
        Ok(())
    }
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text =
        toml::to_string_pretty(value).with_context(|| format!("serialising {}", path.display()))?;
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
    std::fs::rename(&temp, path).with_context(|| format!("committing {}", path.display()))?;
    Ok(())
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
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
mod tests;
