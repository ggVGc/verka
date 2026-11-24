//! The append-only session journal and its replay.
//!
//! The journal is the fundamental record of a session: an ordered log of
//! source-tagged records. An agent record holds the verbatim line received on
//! the agent's stdout (and, for copied branch history, the protocol that
//! produced it); an operator record holds a message the operator sent.
//! Append order is receive order, so the single file reconstructs the whole
//! session with agent and operator turns interleaved. Nothing rendered is
//! stored — [`replay`] reproduces events on demand through the protocol
//! decoder, exactly as a live session decodes them.
//!
//! Alongside the journal, one [`SessionMeta`] (genta's record of which agent
//! produced a session) is written once at session creation, so a stored
//! session can later be replayed — and understood by a human browsing the
//! store — without re-deriving its launch selection.

use crate::agent::{Profile, Selection, SessionMeta};
use crate::event::{decode_line, AgentEvent, BranchDirection, Protocol};
use crate::protocol::{
    CompletionState, ComposerState, Contract, Direction, Integration, QueuedMessage, RawLine,
    SessionOrigin, SessionSummary, COMPOSER_HISTORY_LIMIT,
};
use crate::worktree::Checkout;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredSessionMeta {
    workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<SessionOrigin>,
    /// The contract the session's most recent typed turn was sent under, so
    /// its answer can be parsed without the client restating the shape — and
    /// still can be after the interaction has ended and only the journal is
    /// left. `None` for a session whose turns were all untyped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contract: Option<Contract>,
    /// Whether an interaction of this Session that a plan window refuses
    /// should be resumed and asked again once the window turns over; see
    /// [`crate::quota`].
    ///
    /// Stored with the Session rather than held on the interaction alone
    /// because that is the one thing a retry destroys: it stops the refused
    /// interaction and resumes the Session as a new one, and the operator's
    /// standing "keep at it" has to outlive that or it would apply exactly
    /// once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    auto_retry: bool,
    /// Whether the work an interaction of this Session leaves in its checkout
    /// is committed each time it goes idle, as the operator last answered it.
    /// `None` until they do, and then the answer depends on where the Session
    /// works — see [`resolve_auto_commit`].
    ///
    /// Stored with the Session for the reason [`Self::auto_retry`] is: the
    /// operator's answer has to survive the interaction being resumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auto_commit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diff_branch: Option<String>,
    /// The linked checkout and branch this Session works in, for a Session
    /// launched with one. `None` means it works in the Workspace directory
    /// itself — or that it predates this field, which is why a reader that
    /// finds nothing here still looks on disk before concluding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout: Option<Checkout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    previous_branches: Vec<crate::protocol::SavedBranch>,
    /// Whether the operator has finished with this Session. A property of the
    /// Session rather than of whichever interaction happened to be open when
    /// it was set, so it outlives that interaction and is what the stored-
    /// sessions picker filters on. Cleared when the Session is resumed, unless
    /// it is [`CompletionState::Sealed`]: an interaction working on it again
    /// is not one the operator is done with, but a seal is not undone by that
    /// either.
    #[serde(default, deserialize_with = "deserialize_completion")]
    completed: CompletionState,
    #[serde(flatten)]
    agent: SessionMeta,
}

/// Decode a Session's completion state, accepting the JSON boolean that stood
/// in this field before completion became an enum. Sessions written by an
/// older Styra are still on disk, and the operator's completed ones are
/// exactly the ones that spelled it `true`: refusing to read them would make
/// the Sessions they had finished with the ones that stopped loading at all.
fn deserialize_completion<'de, D>(deserializer: D) -> std::result::Result<CompletionState, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Legacy(bool),
        State(CompletionState),
    }
    Ok(match Stored::deserialize(deserializer)? {
        Stored::Legacy(true) => CompletionState::Completed,
        Stored::Legacy(false) => CompletionState::Active,
        Stored::State(state) => state,
    })
}

/// One line of the journal. Tagged by source so replay knows whether to decode
/// the record as an agent wire line or surface it as an operator message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
enum Record {
    /// A line received verbatim on the agent's stdout.
    Agent {
        at_ms: u64,
        raw: String,
        /// A branched Session can contain history produced by a different
        /// provider than the one that will append to it. Old journals omit
        /// this field and continue to use the Session's current protocol.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol: Option<Protocol>,
    },
    /// A message the operator sent to the agent.
    User { at_ms: u64, text: String },
    /// A model (or effort) change the operator made mid-session. No provider
    /// puts this on the wire, so it is journaled in its own right; without it
    /// a replayed session would show the whole conversation as if it had run
    /// on the model it ended on.
    ModelChange {
        at_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    ProviderChange {
        at_ms: u64,
        from: String,
        selection: Selection,
    },
    /// A branch boundary, written into both sides when a Session is branched:
    /// the source gets a record naming the branch, the branch gets one naming
    /// the source. Like [`Record::ModelChange`] no provider puts this on the
    /// wire, and it belongs beside the messages — it is where a conversation
    /// was continued somewhere else.
    ///
    /// Carries only the other side's id, not its name: a name baked in here
    /// would go stale the moment that Session was renamed. A reader resolves
    /// the current name live, from the id, instead.
    Branch {
        at_ms: u64,
        direction: BranchDirection,
        session: String,
    },
}

/// A live, append-only handle to one session's journal file.
///
/// Methods take `&mut self`; the reader thread (agent lines) and the UI thread
/// (operator messages) share one journal behind a mutex so records are written
/// in true receive order.
pub struct Journal {
    file: File,
    path: PathBuf,
}

impl Journal {
    /// Open (creating, truncating) the journal for a new session directory.
    pub fn create(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)
            .with_context(|| format!("creating session directory {}", directory.display()))?;
        let path = directory.join(JOURNAL_FILE);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .with_context(|| format!("creating journal {}", path.display()))?;
        Ok(Self { file, path })
    }

    /// Reopen an existing session journal for a native provider resume.
    pub fn open(directory: &Path) -> Result<Self> {
        let path = directory.join(JOURNAL_FILE);
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .with_context(|| format!("opening journal {} for append", path.display()))?;
        Ok(Self { file, path })
    }

    /// Create a Session beneath its owning durable Workspace.
    pub fn create_in_workspace(
        store_root: &Path,
        workspace_id: &str,
        profile: &Profile,
        selection: &Selection,
        name: Option<String>,
    ) -> Result<(Self, String)> {
        crate::workspace::get(store_root, workspace_id)?;
        let id = new_session_id();
        let directory = crate::workspace::sessions_dir(store_root, workspace_id).join(&id);
        let journal = Self::create(&directory)?;
        write_session_meta(
            &directory,
            &SessionMeta::new(selection.clone(), profile.protocol),
            workspace_id,
            name,
        )?;
        Ok((journal, id))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record a verbatim agent line. The trailing newline is not part of the
    /// record; it is re-added on replay only implicitly by line splitting.
    pub fn record_agent_line(&mut self, raw: &str) -> Result<()> {
        self.write(&Record::Agent {
            at_ms: now_ms(),
            raw: raw.to_owned(),
            protocol: None,
        })
    }

    pub(crate) fn record_provider_change(
        &mut self,
        from: crate::agent::Provider,
        selection: &Selection,
    ) -> Result<()> {
        self.write(&Record::ProviderChange {
            at_ms: now_ms(),
            from: from.as_str().to_owned(),
            selection: selection.clone(),
        })
    }

    /// Seed a branch with the source journal's leading history. Agent records
    /// remember the protocol that produced them, so a provider-converted
    /// branch can replay its old lines and later append lines in its new
    /// protocol. Existing per-record overrides are retained when branching a
    /// Session that was itself converted before.
    pub(crate) fn copy_branch_from(
        &mut self,
        source: &Path,
        source_protocol: Protocol,
        through_ms: Option<u64>,
        history: crate::protocol::BranchHistory,
    ) -> Result<()> {
        let file_path = if source.is_dir() {
            source.join(JOURNAL_FILE)
        } else {
            source.to_path_buf()
        };
        let file = File::open(&file_path)
            .with_context(|| format!("opening source journal {}", file_path.display()))?;
        let mut selected_record = None;
        for line in BufReader::new(file).lines() {
            let line = line.context("reading source journal line")?;
            if line.trim().is_empty() {
                continue;
            }
            let record: Record = serde_json::from_str(&line)
                .with_context(|| format!("parsing source journal {}", file_path.display()))?;
            let at_ms = match &record {
                Record::Agent { at_ms, .. }
                | Record::User { at_ms, .. }
                | Record::ModelChange { at_ms, .. }
                | Record::ProviderChange { at_ms, .. }
                | Record::Branch { at_ms, .. } => *at_ms,
            };
            if through_ms.is_some_and(|cutoff| at_ms > cutoff) {
                // Branch markers can precede older inherited history, so a
                // newer marker does not mean we have passed the selected entry.
                if matches!(&record, Record::Branch { .. }) {
                    continue;
                }
                break;
            }
            let record = match record {
                Record::Agent {
                    at_ms,
                    raw,
                    protocol,
                } => Record::Agent {
                    at_ms,
                    raw,
                    protocol: protocol.or(Some(source_protocol)),
                },
                other @ (Record::User { .. }
                | Record::ModelChange { .. }
                | Record::ProviderChange { .. }
                | Record::Branch { .. }) => other,
            };
            // A marker is a boundary, not an entry the operator could have
            // selected, so branching only the selected entry never resolves
            // to one.
            let selectable = !matches!(
                record,
                Record::Branch { .. } | Record::ProviderChange { .. }
            );
            match history {
                crate::protocol::BranchHistory::ThroughSelected => self.write(&record)?,
                // A selected event's UI wire timestamp is exact for agent
                // records and just after its host-side User record. Retaining
                // the last record at or before the fixed branch point handles
                // both without guessing a time tolerance.
                crate::protocol::BranchHistory::SelectedOnly if selectable => {
                    selected_record = Some(record)
                }
                crate::protocol::BranchHistory::SelectedOnly => {}
            }
        }
        if let Some(record) = selected_record {
            self.write(&record)?;
        }
        Ok(())
    }

    /// Record a message the operator sent to the agent.
    pub fn record_user_message(&mut self, text: &str) -> Result<()> {
        self.write(&Record::User {
            at_ms: now_ms(),
            text: text.to_owned(),
        })
    }

    /// Record the operator moving the session onto a different model or
    /// reasoning effort. Each argument is `Some` only if that setting changed,
    /// so a replay can say what moved rather than restating the selection.
    pub fn record_model_change(&mut self, model: Option<&str>, effort: Option<&str>) -> Result<()> {
        self.write(&Record::ModelChange {
            at_ms: now_ms(),
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        })
    }

    /// Record a branch boundary linking this Session to `session`.
    pub fn record_branch(&mut self, direction: BranchDirection, session: &str) -> Result<()> {
        self.write(&Record::Branch {
            at_ms: now_ms(),
            direction,
            session: session.to_owned(),
        })
    }

    fn write(&mut self, record: &Record) -> Result<()> {
        let mut line = serde_json::to_string(record).context("serializing journal record")?;
        line.push('\n');
        self.file
            .write_all(line.as_bytes())
            .with_context(|| format!("appending to journal {}", self.path.display()))?;
        // Flush eagerly so a session that is killed still leaves a usable
        // journal; the volume is one small line per event.
        self.file.flush().ok();
        Ok(())
    }
}

const JOURNAL_FILE: &str = "journal.jsonl";
const SESSION_META_FILE: &str = "session.json";
const QUEUE_FILE: &str = "queue.json";
const COMPOSER_FILE: &str = "composer.json";
/// Where integration keeps its progress through a Session — see
/// `crate::integration`.
pub(crate) const INTEGRATION_FILE: &str = "integration.json";
const SNAPSHOTS_FILE: &str = "snapshots.jsonl";
const SNAPSHOT_DIFFS_DIRECTORY: &str = "snapshots";

/// When in a turn a [`Snapshot`] was taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMoment {
    /// Just before an operator message was sent to the agent. The default,
    /// because it was the only moment before the other was recorded.
    #[default]
    BeforeMessage,
    /// As the interaction stopped working, after any automatic commit of the
    /// turn's work.
    TurnEnded,
}

/// The state of an interaction's checkout at one moment of a turn: one line
/// of the Session's `snapshots.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Counts from 1 in append order, and names the diff file.
    pub sequence: u64,
    pub at_ms: u64,
    #[serde(default)]
    pub moment: SnapshotMoment,
    /// Which operator turn of the Session this belongs to, counting its
    /// operator messages from 1 — inherited ones included, for a branch —
    /// so a client can find a turn's snapshots from the messages it lists.
    /// A snapshot before a message belongs to the turn that message starts.
    /// `None` for a snapshot written before turns were counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// The checkout the agent was working in, on the host.
    pub checkout: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The commit checked out, or `None` on a branch with no commits yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The diff from `commit` to the working tree, relative to the Session
    /// directory, for a checkout `git status` reported changes in. `None`
    /// means it was clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<PathBuf>,
}

/// What a snapshot is taken of, before it is numbered and written.
pub struct SnapshotState {
    pub moment: SnapshotMoment,
    pub turn: Option<u64>,
    pub checkout: PathBuf,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub diff: Option<Vec<u8>>,
}

/// Append one snapshot to a Session's record of its checkout, writing its diff
/// beside it first, so an index line never names a diff that is not there.
///
/// Each diff is synced before its line is appended, and the line before this
/// returns: a snapshot is taken just before an agent is let loose on the
/// checkout, or just after, and is only worth anything if it survives what
/// happens next.
pub fn append_snapshot(path: &Path, state: SnapshotState) -> Result<Snapshot> {
    let directory = session_directory(path);
    let sequence = read_snapshots(&directory)?
        .last()
        .map_or(1, |last| last.sequence + 1);
    let diff = match state.diff {
        None => None,
        Some(bytes) => {
            let relative = Path::new(SNAPSHOT_DIFFS_DIRECTORY).join(format!("{sequence:06}.diff"));
            let diffs = directory.join(SNAPSHOT_DIFFS_DIRECTORY);
            std::fs::create_dir_all(&diffs)
                .with_context(|| format!("creating {}", diffs.display()))?;
            let target = directory.join(&relative);
            let mut file =
                File::create(&target).with_context(|| format!("writing {}", target.display()))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .with_context(|| format!("writing {}", target.display()))?;
            Some(relative)
        }
    };
    let snapshot = Snapshot {
        sequence,
        at_ms: now_ms(),
        moment: state.moment,
        turn: state.turn,
        checkout: state.checkout,
        branch: state.branch,
        commit: state.commit,
        diff,
    };
    let index = directory.join(SNAPSHOTS_FILE);
    let mut line = serde_json::to_string(&snapshot).context("serializing a snapshot")?;
    line.push('\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index)
        .with_context(|| format!("opening {}", index.display()))?;
    file.write_all(line.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("appending to {}", index.display()))?;
    Ok(snapshot)
}

/// Whether `state` is what the Session's last snapshot recorded: the same
/// checkout, branch and commit, and the same uncommitted work to the byte.
/// When it is, nothing has happened in the checkout since.
pub fn unchanged_since_last_snapshot(path: &Path, state: &SnapshotState) -> Result<bool> {
    let directory = session_directory(path);
    let Some(last) = read_snapshots(&directory)?.pop() else {
        return Ok(false);
    };
    if last.checkout != state.checkout || last.branch != state.branch || last.commit != state.commit
    {
        return Ok(false);
    }
    match (&last.diff, &state.diff) {
        (None, None) => Ok(true),
        (Some(stored), Some(diff)) => {
            let stored = directory.join(stored);
            let stored =
                std::fs::read(&stored).with_context(|| format!("reading {}", stored.display()))?;
            Ok(stored == *diff)
        }
        _ => Ok(false),
    }
}

/// The snapshots to read one turn's changes from: see [`turn_snapshots`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnSnapshots {
    /// The turn started from the first and ended on the second.
    Ended(Snapshot, Snapshot),
    /// The turn has not ended; the most recent snapshot is the one its
    /// changes so far are read from.
    Running(Snapshot),
}

/// The snapshots a turn started from and ended on, if both were taken, or
/// the most recent snapshot for a turn that has not ended yet.
///
/// The end is the turn's last `turn_ended` snapshot: an agent woken by its
/// own background task completes again without a message of the operator's,
/// and that work is still the turn's. The start is the snapshot just before
/// the turn's first: the one taken before the turn's message, or — when that
/// one was skipped because nothing had changed — the one the previous turn
/// ended on. Anything else there means snapshots were not being taken when the
/// turn started, and there is no start to compare with.
///
/// A turn with no end is running only if no later turn has been snapshotted;
/// otherwise its end was never recorded, and there is nothing to say.
pub fn turn_snapshots(path: &Path, turn: u64) -> Result<Option<TurnSnapshots>> {
    let snapshots = read_snapshots(path)?;
    let Some(end) = snapshots.iter().rposition(|snapshot| {
        snapshot.moment == SnapshotMoment::TurnEnded && snapshot.turn == Some(turn)
    }) else {
        let later = snapshots
            .iter()
            .any(|snapshot| snapshot.turn.is_some_and(|other| other > turn));
        return Ok((!later)
            .then(|| snapshots.last().cloned())
            .flatten()
            .map(TurnSnapshots::Running));
    };
    let first = snapshots
        .iter()
        .position(|snapshot| snapshot.turn == Some(turn))
        .expect("the turn's end is one of its snapshots");
    let start = match &snapshots[first] {
        own @ Snapshot {
            moment: SnapshotMoment::BeforeMessage,
            ..
        } => own,
        _ => match first.checked_sub(1) {
            Some(before) => &snapshots[before],
            None => return Ok(None),
        },
    };
    let starts_turn = match start.moment {
        SnapshotMoment::BeforeMessage => start.turn == Some(turn),
        SnapshotMoment::TurnEnded => turn > 1 && start.turn == Some(turn - 1),
    };
    Ok(starts_turn.then(|| TurnSnapshots::Ended(start.clone(), snapshots[end].clone())))
}

/// Every snapshot taken of a Session's checkout, oldest first. Absent file
/// means none were, not an error.
pub fn read_snapshots(path: &Path) -> Result<Vec<Snapshot>> {
    let index = session_directory(path).join(SNAPSHOTS_FILE);
    if !index.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(&index).with_context(|| format!("reading {}", index.display()))?;
    BufReader::new(file)
        .lines()
        .filter(|line| !matches!(line, Ok(line) if line.trim().is_empty()))
        .map(|line| {
            let line = line.with_context(|| format!("reading {}", index.display()))?;
            serde_json::from_str(&line).with_context(|| format!("parsing {}", index.display()))
        })
        .collect()
}

/// Read the operator messages a Session's interaction had queued but not yet
/// sent, so the queue survives a client disconnect (closing the Styra UI) or a
/// server restart. Absent file means an empty queue, not an error.
pub fn read_queued_messages(directory: &Path) -> Result<Vec<QueuedMessage>> {
    let path = directory.join(QUEUE_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Persist the current queue of not-yet-sent operator messages for a Session.
pub fn write_queued_messages(directory: &Path, messages: &[QueuedMessage]) -> Result<()> {
    let path = directory.join(QUEUE_FILE);
    let json = serde_json::to_string_pretty(messages).context("serializing the message queue")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

/// Read what the operator had in a Session's message box, and the messages
/// they sent from it. Absent file means an empty box, not an error.
pub fn read_composer(path: &Path) -> Result<ComposerState> {
    let path = session_directory(path).join(COMPOSER_FILE);
    if !path.exists() {
        return Ok(ComposerState::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Replace a Session's stored message box, keeping only the newest
/// [`COMPOSER_HISTORY_LIMIT`] sent messages. Written aside and renamed into
/// place: it is rewritten as the operator types, and a crash mid-write must
/// not cost them what they had.
pub fn write_composer(path: &Path, composer: &ComposerState) -> Result<()> {
    let directory = session_directory(path);
    let mut composer = composer.clone();
    let excess = composer
        .history
        .len()
        .saturating_sub(COMPOSER_HISTORY_LIMIT);
    composer.history.drain(..excess);
    let path = directory.join(COMPOSER_FILE);
    let temporary = directory.join("composer.json.tmp");
    let json = serde_json::to_string_pretty(&composer).context("serializing the message box")?;
    std::fs::write(&temporary, json).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, &path).with_context(|| format!("replacing {}", path.display()))
}

/// The Session directory `path` names: itself, or the one holding the file.
fn session_directory(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    }
}

fn write_session_meta(
    directory: &Path,
    meta: &SessionMeta,
    workspace_id: &str,
    name: Option<String>,
) -> Result<()> {
    let path = directory.join(SESSION_META_FILE);
    let stored = StoredSessionMeta {
        workspace_id: workspace_id.to_owned(),
        name,
        tags: Vec::new(),
        provider_session_id: None,
        origin: None,
        contract: None,
        auto_retry: false,
        auto_commit: None,
        diff_branch: None,
        checkout: None,
        previous_branches: Vec::new(),
        completed: CompletionState::Active,
        agent: meta.clone(),
    };
    let json = serde_json::to_string_pretty(&stored).context("serializing session metadata")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

/// List Sessions belonging to one durable Workspace, newest first.
pub fn list_workspace_sessions(
    store_root: &Path,
    workspace_id: &str,
) -> Result<Vec<SessionSummary>> {
    crate::workspace::get(store_root, workspace_id)?;
    list_sessions_at(
        &crate::workspace::sessions_dir(store_root, workspace_id),
        workspace_id,
    )
}

/// Summarize the one Session stored in `path`, without touching its siblings.
/// Looking a single session up is the common case (a picker preview, a resume)
/// and must not cost a scan of the whole Workspace.
pub fn session_summary_at(path: &Path, workspace_id: &str) -> Result<SessionSummary> {
    let id = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let meta = read_stored_session_meta(path)?;
    let branched_from = meta.checkout.and_then(|checkout| checkout.branched_from);
    let integration = integration_at(
        path,
        branched_from
            .as_ref()
            .and_then(|point| point.branch.as_deref()),
    );
    let created_at_ms = session_created_at_ms(&id);
    let last_event_at_ms = last_event_at_ms(path).or(created_at_ms);
    Ok(SessionSummary {
        id,
        name: meta.name,
        first_prompt: first_prompt_at(path)?,
        tags: meta.tags,
        workspace_id: workspace_id.to_owned(),
        path: path.to_path_buf(),
        selection: meta.agent.selection,
        age: humanize_age(now_ms(), created_at_ms),
        created_at_ms,
        last_event_at_ms,
        last_event_age: humanize_age(now_ms(), last_event_at_ms),
        origin: meta.origin,
        branched_from,
        completed: meta.completed,
        integration,
    })
}

/// Read whether integration has landed a stored Session's work — see
/// [`SessionSummary::integration`].
pub fn read_session_integration(path: &Path) -> Option<Integration> {
    let meta = read_stored_session_meta(path).ok()?;
    let branched_from = meta.checkout.and_then(|checkout| checkout.branched_from);
    integration_at(
        path,
        branched_from.and_then(|point| point.branch).as_deref(),
    )
}

/// The squash commit the Session's integration record names, if one has
/// landed. `source` stands in for the branch on a record written before the
/// branch was kept in it: integration always lands on the branch the
/// Session's own was made from.
fn integration_at(path: &Path, source: Option<&str>) -> Option<Integration> {
    /// The part of integration's progress record that says where the work
    /// went; the rest is integration's own business.
    #[derive(Deserialize)]
    struct Landed {
        commit: Option<String>,
        #[serde(default)]
        branch: Option<String>,
    }
    let bytes = std::fs::read(path.join(INTEGRATION_FILE)).ok()?;
    let landed: Landed = serde_json::from_slice(&bytes).ok()?;
    Some(Integration {
        commit: landed.commit?,
        branch: landed.branch.or_else(|| source.map(str::to_owned)),
    })
}

/// Read only as far as the first operator record: list filtering needs the
/// conversation's subject, but listing every Session must not replay its full
/// journal or decode provider traffic.
fn first_prompt_at(path: &Path) -> Result<Option<String>> {
    let journal = path.join(JOURNAL_FILE);
    let file = match File::open(&journal) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", journal.display())),
    };
    for line in BufReader::new(file).lines() {
        let line = line.context("reading journal line")?;
        if let Ok(Record::User { text, .. }) = serde_json::from_str(&line) {
            return Ok(Some(text));
        }
    }
    Ok(None)
}

/// The first operator message the Session wrote itself: for a branch, the
/// first one after its branch point rather than the inherited opening prompt
/// its copied history starts with. A Session that was never branched has no
/// branch point, and answers with its first prompt.
///
/// A branch's own history starts after the last marker saying where it came
/// from (see `branch_session`); markers saying a child branched off this
/// Session are not boundaries of its own.
pub fn own_first_prompt_at(path: &Path) -> Result<Option<String>> {
    let journal = path.join(JOURNAL_FILE);
    let file = match File::open(&journal) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", journal.display())),
    };
    let mut prompt = None;
    for line in BufReader::new(file).lines() {
        let line = line.context("reading journal line")?;
        match serde_json::from_str(&line) {
            Ok(Record::Branch {
                direction: BranchDirection::From,
                ..
            }) => prompt = None,
            Ok(Record::User { text, .. }) if prompt.is_none() => prompt = Some(text),
            _ => {}
        }
    }
    Ok(prompt)
}

/// Summarize `session_id` in `workspace_id`, or `None` when that Workspace
/// stores no such Session.
pub fn find_workspace_session(
    store_root: &Path,
    workspace_id: &str,
    session_id: &str,
) -> Result<Option<SessionSummary>> {
    let path = crate::workspace::sessions_dir(store_root, workspace_id).join(session_id);
    if !path.is_dir() {
        return Ok(None);
    }
    session_summary_at(&path, workspace_id).map(Some)
}

pub(crate) fn list_sessions_at(dir: &Path, workspace_id: &str) -> Result<Vec<SessionSummary>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading an entry in {}", dir.display()))?;
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        sessions.push(session_summary_at(&entry.path(), workspace_id)?);
    }
    sort_newest_first(&mut sessions);
    Ok(sessions)
}

/// When the Session's journal was last written, in milliseconds since the
/// epoch. The journal is appended to for every event, so its write time is
/// the Session's last activity without reading a byte of it.
fn last_event_at_ms(directory: &Path) -> Option<u64> {
    let modified = std::fs::metadata(directory.join(JOURNAL_FILE))
        .ok()?
        .modified()
        .ok()?;
    let since_epoch = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(since_epoch.as_millis()).ok()
}

/// Recover the latest sent operator message when resuming a Session. Agent
/// output, focus changes, and messages still in the queue do not count.
pub(crate) fn last_user_message_at_ms(directory: &Path) -> Result<u64> {
    let file = File::open(directory.join(JOURNAL_FILE))
        .context("opening journal for latest operator message")?;
    let mut latest = 0;
    for line in BufReader::new(file).lines() {
        if let Ok(Record::User { at_ms, .. }) = serde_json::from_str::<Record>(&line?) {
            latest = at_ms;
        }
    }
    Ok(latest)
}

/// Whether the Session's journal stops partway through a turn: the operator's
/// last message, or the last turn the agent started, has no ending after it.
///
/// Read when a restart restores a row that was still live. The mirrored row
/// cannot answer this — it is written when the list changes, not each time a
/// turn starts or ends, and a killed server never writes it at all — but the
/// journal is appended as each line arrives, so it says how far the turn got
/// whatever took the server down.
pub(crate) fn ends_mid_turn(directory: &Path) -> Result<bool> {
    let protocol = read_session_meta(directory)?.protocol;
    let file = File::open(directory.join(JOURNAL_FILE))
        .context("opening journal for the last turn's ending")?;
    let mut mid_turn = false;
    for line in BufReader::new(file).lines() {
        match serde_json::from_str::<Record>(&line?) {
            Ok(Record::User { .. }) => mid_turn = true,
            Ok(Record::Agent {
                raw,
                protocol: record_protocol,
                ..
            }) => match decode_line(record_protocol.unwrap_or(protocol), &raw) {
                AgentEvent::TurnStarted => mid_turn = true,
                AgentEvent::TurnCompleted { .. } => mid_turn = false,
                _ => {}
            },
            _ => {}
        }
    }
    Ok(mid_turn)
}

fn sort_newest_first(sessions: &mut [SessionSummary]) {
    sessions.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
}

/// Parse the millisecond timestamp [`new_session_id`] embeds as the leading
/// field of a session id, for display and sorting. An id that doesn't match
/// that shape (hand-crafted, or from some future format) parses to `None`
/// rather than failing the whole listing.
fn session_created_at_ms(id: &str) -> Option<u64> {
    id.split('-').next()?.parse().ok()
}

/// A coarse, human-readable age bucket. `now_ms` and `created_at_ms` are both
/// milliseconds since the epoch; passing them in rather than reading the
/// clock keeps this pure and testable.
fn humanize_age(now_ms: u64, created_at_ms: Option<u64>) -> String {
    let Some(created_at_ms) = created_at_ms else {
        return "unknown".into();
    };
    let elapsed_secs = now_ms.saturating_sub(created_at_ms) / 1000;
    if elapsed_secs < 60 {
        "just now".into()
    } else if elapsed_secs < 3_600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3_600)
    } else {
        format!("{}d ago", elapsed_secs / 86_400)
    }
}

/// Read back which agent produced a stored Session.
pub fn read_session_meta(path: &Path) -> Result<SessionMeta> {
    Ok(read_stored_session_meta(path)?.agent)
}

/// Return the durable Workspace owning this Session.
pub fn read_session_workspace_id(path: &Path) -> Result<String> {
    Ok(read_stored_session_meta(path)?.workspace_id)
}

/// Return the Session's operator-facing name from its durable metadata.
///
/// Names belong to Sessions, not to whichever live or restored Interaction
/// currently presents them. Keeping this read here lets those projections
/// resolve the current value instead of retaining another mutable copy.
pub fn read_session_name(path: &Path) -> Result<Option<String>> {
    Ok(read_stored_session_meta(path)?.name)
}

/// Read the operator-assigned tags for a stored Session.
pub fn read_session_tags(path: &Path) -> Result<Vec<String>> {
    Ok(read_stored_session_meta(path)?.tags)
}

/// Read the provider's native identity for a stored Session.
pub fn read_provider_session_id(path: &Path) -> Result<Option<String>> {
    Ok(read_stored_session_meta(path)?.provider_session_id)
}

/// Persist the native identity reported by the provider.
pub fn store_provider_session_id(path: &Path, provider_session_id: &str) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    match stored.provider_session_id.as_deref() {
        Some(existing) if existing == provider_session_id => return Ok(()),
        Some(existing) => anyhow::bail!(
            "session already belongs to provider conversation {existing:?}, not {provider_session_id:?}"
        ),
        None => stored.provider_session_id = Some(provider_session_id.to_owned()),
    }
    write_stored_session_meta(&directory, &stored)
}

/// Record that a freshly created Session was branched from another one,
/// rather than launched fresh. Set once, right after creation — a Session's
/// origin does not change afterwards.
pub fn store_session_origin(path: &Path, origin: SessionOrigin) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    stored.origin = Some(origin);
    write_stored_session_meta(&directory, &stored)
}

/// The checkout a Session was given, as the Session itself records it.
///
/// `None` for a Session that works in its Workspace directory — and for one
/// launched before this was stored, whose checkout is still found by the scan
/// in [`crate::worktree::existing_checkout`]. Callers that must not miss an
/// old Session's checkout ask through
/// [`crate::server::ServerState::session_checkout`], which tries both.
pub fn read_session_checkout(path: &Path) -> Result<Option<Checkout>> {
    Ok(read_stored_session_meta(path)?.checkout)
}

/// Record the checkout a Session works in, at the moment one is made for it.
///
/// Set once: a Session keeps the checkout it was given for as long as it
/// exists, and a second one would mean its uncommitted work had been left
/// somewhere the Session no longer names.
pub fn store_session_checkout(path: &Path, checkout: &Checkout) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    match &stored.checkout {
        Some(existing) if existing == checkout => return Ok(()),
        // A Session whose worktree was cleaned up kept its branch and lost its
        // directory. Checking that same branch out again completes the record
        // rather than replacing one checkout with another, so it is allowed
        // where any other change is not.
        Some(existing) if existing.path.is_none() && existing.branch == checkout.branch => {
            stored.checkout = Some(checkout.clone())
        }
        Some(existing) => anyhow::bail!(
            "session already works in {}, not {}",
            existing.describe(),
            checkout.describe()
        ),
        None => stored.checkout = Some(checkout.clone()),
    }
    write_stored_session_meta(&directory, &stored)
}

/// Previous Git refs and their original targets survive worktree cleanup and restarts.
pub fn read_session_branch_history(path: &Path) -> Result<Vec<crate::protocol::SavedBranch>> {
    Ok(read_stored_session_meta(path)?.previous_branches)
}

/// Save a copy of the branch being left and replace the checkout in one
/// metadata write. `restored`, when returning to a saved copy, leaves the
/// history: its contents are now the checkout's own.
pub fn store_retargeted_checkout(
    path: &Path,
    previous: &Checkout,
    saved: crate::protocol::SavedBranch,
    checkout: &Checkout,
    restored: Option<&str>,
) -> Result<()> {
    let mut stored = read_stored_session_meta(path)?;
    anyhow::ensure!(
        stored.checkout.as_ref() == Some(previous),
        "the session checkout changed during retargeting"
    );
    stored.previous_branches.retain(|entry| {
        entry.branch != saved.branch
            && entry.branch != checkout.branch
            && Some(entry.branch.as_str()) != restored
    });
    stored.previous_branches.push(saved);
    stored.checkout = Some(checkout.clone());
    write_stored_session_meta(path, &stored)
}

/// Update the base only after verifying a rebase of this same checkout.
pub fn store_rebased_checkout(path: &Path, checkout: &Checkout) -> Result<()> {
    let mut stored = read_stored_session_meta(path)?;
    let existing = stored
        .checkout
        .as_ref()
        .context("session has no checkout")?;
    anyhow::ensure!(
        existing.path == checkout.path && existing.branch == checkout.branch,
        "the session checkout changed during rebase"
    );
    stored.checkout = Some(checkout.clone());
    write_stored_session_meta(path, &stored)
}

/// Record that a Session's checkout has been removed, keeping the branch it
/// worked on.
///
/// The one edit [`store_session_checkout`] refuses to make itself, and the
/// reason it refuses: a Session must not be pointed at a *different*
/// directory, because its uncommitted work would be left in the old one. This
/// is the opposite case — the directory is gone precisely because nothing was
/// left in it — and the branch, which is where the work actually is, stays
/// exactly as it was.
///
/// Returns the checkout as it now stands, or `None` for a Session that had no
/// checkout to begin with.
pub fn clear_session_worktree(path: &Path) -> Result<Option<Checkout>> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    let Some(checkout) = stored.checkout.as_ref().map(Checkout::without_worktree) else {
        return Ok(None);
    };
    if stored.checkout.as_ref() == Some(&checkout) {
        return Ok(Some(checkout));
    }
    stored.checkout = Some(checkout.clone());
    write_stored_session_meta(&directory, &stored)?;
    Ok(Some(checkout))
}

/// Read the contract a Session's most recent typed turn was sent under.
pub fn read_session_contract(path: &Path) -> Result<Option<Contract>> {
    Ok(read_stored_session_meta(path)?.contract)
}

/// Record the contract a turn was just sent under, replacing any earlier one.
///
/// Last-turn-wins: a session can mix typed and untyped turns freely, and the
/// answer a client asks for is the one to the question it just asked.
pub fn store_session_contract(path: &Path, contract: Contract) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    if stored.contract == Some(contract) {
        return Ok(());
    }
    stored.contract = Some(contract);
    write_stored_session_meta(&directory, &stored)
}

/// Whether a refused interaction of this Session should be asked again once
/// the plan window that refused it turns over.
pub fn read_session_auto_retry(path: &Path) -> Result<bool> {
    Ok(read_stored_session_meta(path)?.auto_retry)
}

/// Record the operator's answer to "keep at it after a rate limit". Kept with
/// the Session so it survives the resume a retry itself performs.
pub fn store_session_auto_retry(path: &Path, auto_retry: bool) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    if stored.auto_retry == auto_retry {
        return Ok(());
    }
    stored.auto_retry = auto_retry;
    write_stored_session_meta(&directory, &stored)
}

/// The comparison branch saved for this interaction’s diff view.
pub fn read_session_diff_branch(path: &Path) -> Result<Option<String>> {
    Ok(read_stored_session_meta(path)?.diff_branch)
}

pub fn store_session_diff_branch(path: &Path, branch: Option<String>) -> Result<()> {
    let directory = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let mut stored = read_stored_session_meta(directory)?;
    stored.diff_branch = branch;
    write_stored_session_meta(directory, &stored)
}

/// Whether an interaction of this Session commits what each turn leaves
/// behind in its checkout, as the operator answered it — `None` if they never
/// have.
pub fn read_session_auto_commit(path: &Path) -> Result<Option<bool>> {
    Ok(read_stored_session_meta(path)?.auto_commit)
}

/// Whether a Session's turns are committed: the operator's answer if they gave
/// one, and otherwise whether it works in a linked checkout of its own.
///
/// A checkout of its own is a branch nobody else is writing to, so committing
/// every turn there costs the operator nothing they did not ask for. The
/// Workspace directory is usually the operator's own checkout, whose history
/// is theirs to write; it is committed to only when they say so.
pub fn resolve_auto_commit(stored: Option<bool>, in_linked_checkout: bool) -> bool {
    stored.unwrap_or(in_linked_checkout)
}

/// Record whether this Session's turns are committed as they end. Kept with
/// the Session so a resumed interaction carries on doing what the last one did.
pub fn store_session_auto_commit(path: &Path, auto_commit: bool) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    if stored.auto_commit == Some(auto_commit) {
        return Ok(());
    }
    stored.auto_commit = Some(auto_commit);
    write_stored_session_meta(&directory, &stored)
}

/// Whether the operator has marked this Session as finished with.
pub fn read_session_completed(path: &Path) -> Result<CompletionState> {
    Ok(read_stored_session_meta(path)?.completed)
}

/// The Session this one was branched off, if it was.
pub fn read_session_origin(path: &Path) -> Result<Option<String>> {
    Ok(read_stored_session_meta(path)?
        .origin
        .map(|origin| origin.session_id))
}

/// Record the operator's completion state for a Session. Kept with the
/// Session, not the interaction, so it survives the interaction stopping and
/// is what the stored-sessions picker filters on.
pub fn store_session_completed(path: &Path, completed: CompletionState) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    if stored.completed == completed {
        return Ok(());
    }
    stored.completed = completed;
    write_stored_session_meta(&directory, &stored)
}

/// Which Sessions in one Workspace were branched from which, and what each
/// one's stored completion is — what a Session's completion as reported to a
/// client is derived from.
///
/// A Session the operator finished with is not finished while work continues
/// in a branch of it: branching from a completed Session, or resuming a
/// branch further down, is the operator working on that line of conversation
/// again. That is never written back to the parent. It is read off the
/// branches each time the parent's completion is reported, so the parent
/// cannot be left saying "completed" after a branch was reopened, nor "active"
/// after the last branch was finished, by some change that forgot to update it.
#[derive(Debug, Default)]
pub struct Lineage {
    branches: std::collections::HashMap<String, Vec<String>>,
    completed: std::collections::HashMap<String, CompletionState>,
}

impl Lineage {
    /// Read every Session stored in `dir`, one Workspace's sessions
    /// directory. Only their metadata is read, not the journals a full
    /// [`SessionSummary`] looks into. A Session whose metadata cannot be read
    /// is left out: it cannot be shown as working either.
    pub fn read(dir: &Path) -> Self {
        let mut lineage = Self::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return lineage;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let Ok(meta) = read_stored_session_meta(&entry.path()) else {
                continue;
            };
            let id = entry.file_name().to_string_lossy().into_owned();
            lineage.add(id, meta.origin.as_ref(), meta.completed);
        }
        lineage
    }

    /// The lineage of `sessions`, already listed.
    pub fn of(sessions: &[SessionSummary]) -> Self {
        let mut lineage = Self::default();
        for session in sessions {
            lineage.add(
                session.id.clone(),
                session.origin.as_ref(),
                session.completed,
            );
        }
        lineage
    }

    fn add(&mut self, id: String, origin: Option<&SessionOrigin>, completed: CompletionState) {
        if let Some(origin) = origin {
            self.branches
                .entry(origin.session_id.clone())
                .or_default()
                .push(id.clone());
        }
        self.completed.insert(id, completed);
    }

    /// Session `id`'s completion as reported, given the `stored` one: a
    /// Session marked [`CompletionState::Completed`] or
    /// [`CompletionState::Abandoned`] is [`CompletionState::Active`] while any
    /// Session branched from it, however far down, is. A sealed Session stays
    /// sealed: nothing makes that one active again.
    pub fn completion(&self, id: &str, stored: CompletionState) -> CompletionState {
        if !reopened_by_branches(stored) {
            return stored;
        }
        let mut pending = vec![id];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            for branch in self.branches.get(id).into_iter().flatten() {
                // Walked through finished branches too: a finished branch can
                // have an active branch of its own.
                if self.completed.get(branch) == Some(&CompletionState::Active) {
                    return CompletionState::Active;
                }
                pending.push(branch);
            }
        }
        stored
    }

    /// Replace each of `sessions`' stored completion with the reported one.
    pub fn report(&self, sessions: &mut [SessionSummary]) {
        for session in sessions {
            session.completed = self.completion(&session.id, session.completed);
        }
    }
}

/// Whether a Session's branches decide its reported completion — see
/// [`Lineage::completion`].
fn reopened_by_branches(stored: CompletionState) -> bool {
    matches!(
        stored,
        CompletionState::Completed | CompletionState::Abandoned
    )
}

/// [`Lineage`]s read as one request needs them, each Workspace's at most once
/// however many of its Sessions the request reports on, and none at all for a
/// request whose Sessions all have a completion no branch can change.
pub struct Lineages<'a> {
    store_root: &'a Path,
    read: std::collections::HashMap<String, Lineage>,
}

impl<'a> Lineages<'a> {
    pub fn new(store_root: &'a Path) -> Self {
        Self {
            store_root,
            read: std::collections::HashMap::new(),
        }
    }

    /// [`Lineage::completion`] for Session `id` in `workspace_id`.
    pub fn completion(
        &mut self,
        workspace_id: &str,
        id: &str,
        stored: CompletionState,
    ) -> CompletionState {
        if !reopened_by_branches(stored) {
            return stored;
        }
        let store_root = self.store_root;
        self.read
            .entry(workspace_id.to_owned())
            .or_insert_with(|| {
                Lineage::read(&crate::workspace::sessions_dir(store_root, workspace_id))
            })
            .completion(id, stored)
    }
}

/// Preserve each old wire line's decoder before changing the session protocol.
pub(crate) fn prepare_provider_change(path: &Path, protocol: Protocol) -> Result<()> {
    let temporary = path.join("provider-change");
    let mut copied = Journal::create(&temporary)?;
    copied.copy_branch_from(
        path,
        protocol,
        None,
        crate::protocol::BranchHistory::ThroughSelected,
    )?;
    std::fs::rename(copied.path(), path.join(JOURNAL_FILE))?;
    std::fs::remove_dir(&temporary)?;
    Ok(())
}

pub(crate) fn store_session_provider(
    path: &Path,
    selection: &Selection,
    protocol: Protocol,
    native_id: &str,
) -> Result<()> {
    let mut stored = read_stored_session_meta(path)?;
    stored.agent.selection = selection.clone();
    stored.agent.protocol = protocol;
    stored.provider_session_id = Some(native_id.to_owned());
    write_stored_session_meta(path, &stored)
}

/// Record the selection a Session is now running under, so an operator who
/// switched models mid-session and later reopens it resumes on the model they
/// chose rather than the one it launched with.
pub fn store_session_selection(path: &Path, selection: &crate::agent::Selection) -> Result<()> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let mut stored = read_stored_session_meta(&directory)?;
    if stored.agent.selection == *selection {
        return Ok(());
    }
    stored.agent.selection = selection.clone();
    write_stored_session_meta(&directory, &stored)
}

/// Set or clear a Session's operator-facing name and return its normalized value.
pub fn store_session_name(path: &Path, name: Option<&str>) -> Result<Option<String>> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let name = normalize_session_name(name)?;
    let mut stored = read_stored_session_meta(&directory)?;
    stored.name.clone_from(&name);
    write_stored_session_meta(&directory, &stored)?;
    Ok(name)
}

/// Replace a Session's tags and return their normalized form.
pub fn store_session_tags(path: &Path, tags: &[String]) -> Result<Vec<String>> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let tags = normalize_session_tags(tags)?;
    let mut stored = read_stored_session_meta(&directory)?;
    stored.tags.clone_from(&tags);
    write_stored_session_meta(&directory, &stored)?;
    Ok(tags)
}

pub fn normalize_session_tags(tags: &[String]) -> Result<Vec<String>> {
    let mut unique = BTreeSet::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().any(char::is_control) {
            anyhow::bail!("tags must not contain control characters");
        }
        if tag.chars().count() > 40 {
            anyhow::bail!("tags must be at most 40 characters");
        }
        unique.insert(tag.to_owned());
    }
    Ok(unique.into_iter().collect())
}

pub fn normalize_session_name(name: Option<&str>) -> Result<Option<String>> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    if name.chars().any(char::is_control) {
        anyhow::bail!("session name must not contain control characters");
    }
    if name.chars().count() > 80 {
        anyhow::bail!("session name must be at most 80 characters");
    }
    Ok(Some(name.to_owned()))
}

/// A deterministic initial name made from the first non-empty prompt line.
pub fn name_from_message(message: Option<&str>) -> Option<String> {
    let words = message?.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.is_empty() {
        return None;
    }
    let mut name: String = words.chars().take(60).collect();
    if words.chars().count() > 60 {
        name.push('…');
    }
    Some(name)
}

fn write_stored_session_meta(directory: &Path, stored: &StoredSessionMeta) -> Result<()> {
    let meta_path = directory.join(SESSION_META_FILE);
    let temporary = directory.join("session.json.tmp");
    let json = serde_json::to_string_pretty(stored).context("serializing session metadata")?;
    std::fs::write(&temporary, json).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, &meta_path)
        .with_context(|| format!("replacing {}", meta_path.display()))
}

fn read_stored_session_meta(path: &Path) -> Result<StoredSessionMeta> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let meta_path = directory.join(SESSION_META_FILE);
    let text = std::fs::read_to_string(&meta_path)
        .with_context(|| format!("reading {}", meta_path.display()))?;
    let meta: StoredSessionMeta =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", meta_path.display()))?;
    Ok(meta)
}

/// Decode a stored journal back into the ordered event list, reproducing agent
/// events through `protocol` and operator turns as [`AgentEvent::UserMessage`].
/// A journal directory or its file may be passed.
pub fn replay(path: &Path, protocol: Protocol) -> Result<Vec<AgentEvent>> {
    Ok(replay_records(path, protocol, false)?.events)
}

pub(crate) struct ReplayedJournal {
    pub events: Vec<AgentEvent>,
    pub raw: Vec<RawLine>,
    pub last_reported_cwd: Option<String>,
}

/// Replay everything a restored interaction needs from one journal read.
pub(crate) fn replay_restored(path: &Path, protocol: Protocol) -> Result<ReplayedJournal> {
    replay_records(path, protocol, true)
}

fn replay_records(path: &Path, protocol: Protocol, include_raw: bool) -> Result<ReplayedJournal> {
    let file_path = if path.is_dir() {
        path.join(JOURNAL_FILE)
    } else {
        path.to_path_buf()
    };
    let file = File::open(&file_path)
        .with_context(|| format!("opening journal {}", file_path.display()))?;
    let mut events = Vec::new();
    let mut raw_lines = Vec::new();
    let mut last_reported_cwd = None;
    for line in BufReader::new(file).lines() {
        let line = line.context("reading journal line")?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Agent {
                raw,
                at_ms,
                protocol: record_protocol,
            }) => {
                let record_protocol = record_protocol.unwrap_or(protocol);
                let event = decode_line(record_protocol, &raw);
                if include_raw {
                    if raw.contains("cwd") {
                        if let Some(reported) = crate::event::reported_cwd(record_protocol, &raw) {
                            last_reported_cwd = Some(reported);
                        }
                    }
                    raw_lines.push(RawLine {
                        direction: Direction::FromAgent,
                        text: raw,
                        at_ms,
                    });
                }
                events.push(event);
            }
            Ok(Record::User { text, at_ms }) => {
                if include_raw {
                    raw_lines.push(RawLine {
                        direction: Direction::ToAgent,
                        text: text.clone(),
                        at_ms,
                    });
                }
                events.push(AgentEvent::UserMessage { text });
            }
            Ok(Record::Branch {
                direction, session, ..
            }) => events.push(AgentEvent::Branched {
                direction,
                session,
                // The marker carries only the id; a client resolves the
                // current name live instead of trusting one baked in here.
                name: None,
            }),
            Ok(Record::ProviderChange {
                from, selection, ..
            }) => {
                events.push(AgentEvent::ProviderChanged {
                    from,
                    to: selection.provider.as_str().to_owned(),
                    model: selection.model,
                    effort: selection.effort.as_str().to_owned(),
                });
            }
            Ok(Record::ModelChange { model, effort, .. }) => {
                events.push(AgentEvent::ModelChanged { model, effort })
            }
            Err(error) => {
                events.push(AgentEvent::Malformed {
                    error: format!("unreadable journal record: {error}"),
                });
                if include_raw {
                    raw_lines.push(RawLine {
                        direction: Direction::FromAgent,
                        text: line,
                        at_ms: 0,
                    });
                }
            }
        }
    }
    Ok(ReplayedJournal {
        events,
        raw: raw_lines,
        last_reported_cwd,
    })
}

/// Reconstruct the raw interaction from a stored journal: each agent record is
/// its verbatim line, each operator record the message text that was sent.
/// The operator message the branch point `cutoff` lands on, if it lands on
/// one: the last journal record at or before it, when that record is the
/// operator's own message.
///
/// A branch point is a host timestamp, and the host writes an operator message
/// to the agent *before* the provider stamps its own copy of it into its
/// native transcript. Comparing timestamps alone therefore puts the selected
/// message just past the cutoff and drops it from the branch. Its text is what
/// identifies it across the two clocks — see
/// [`crate::server::branch_native_session`].
pub fn operator_message_at(path: &Path, cutoff: u64) -> Result<Option<String>> {
    let file_path = if path.is_dir() {
        path.join(JOURNAL_FILE)
    } else {
        path.to_path_buf()
    };
    let file = File::open(&file_path)
        .with_context(|| format!("opening journal {}", file_path.display()))?;
    let mut selected = None;
    for line in BufReader::new(file).lines() {
        let line = line.context("reading journal line")?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        match record {
            Record::User { at_ms, text } if at_ms <= cutoff => selected = Some(text),
            Record::Agent { at_ms, .. } | Record::ModelChange { at_ms, .. } if at_ms <= cutoff => {
                selected = None
            }
            // A marker is not an entry the operator could have selected, so it
            // does not displace the message before it.
            Record::Branch { .. } | Record::ProviderChange { .. } => {}
            _ => break,
        }
    }
    Ok(selected)
}

pub fn replay_raw(path: &Path) -> Result<Vec<RawLine>> {
    let file_path = if path.is_dir() {
        path.join(JOURNAL_FILE)
    } else {
        path.to_path_buf()
    };
    let file = File::open(&file_path)
        .with_context(|| format!("opening journal {}", file_path.display()))?;
    let mut raw = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.context("reading journal line")?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Agent {
                raw: text, at_ms, ..
            }) => raw.push(RawLine {
                direction: Direction::FromAgent,
                text,
                at_ms,
            }),
            Ok(Record::User { text, at_ms }) => raw.push(RawLine {
                direction: Direction::ToAgent,
                text,
                at_ms,
            }),
            // The raw view is the wire; a model change is a host-side record
            // with no line of its own, and the control request it produces is
            // journaled separately when the provider takes one.
            Ok(Record::ModelChange { .. })
            | Ok(Record::ProviderChange { .. })
            | Ok(Record::Branch { .. }) => {}
            // A record too damaged to parse still holds whatever was written
            // to the journal, and [`replay`] surfaces it as a `Malformed`
            // event, so it keeps a line here too: the raw view is where an
            // operator would go to see what the event could not decode, and
            // dropping it would leave the two replays describing different
            // journals. It has no timestamp to recover, and a zero `at_ms` is
            // what marks a line as no branch point.
            Err(_) => raw.push(RawLine {
                direction: Direction::FromAgent,
                text: line,
                at_ms: 0,
            }),
        }
    }
    Ok(raw)
}

/// The last working directory the agent named on the wire in this journal, as
/// it named it.
///
/// Where an agent is working is live session metadata: it is reported on the
/// wire and nothing writes it down in its own right, so the only durable record
/// of a Session that moved out of the directory it was launched in is the lines
/// it said so on. Folding them here is what lets a reopened Session say where
/// it was working, the way folding its model reports is what lets it say what
/// it last ran on (`replayed_selection`, in [`crate::server`]).
///
/// The path is the agent's own — a sandbox path, and one no provider is trusted
/// to have chosen. The caller translates and vets it through the Workspace
/// mount with [`crate::interaction::host_working_directory`] before it goes
/// anywhere near a client.
///
/// A record carrying its own protocol is read with that one: branched history
/// can hold lines from the provider it was copied from, which name their cwd in
/// a different place on the line.
pub fn last_reported_cwd(path: &Path, protocol: Protocol) -> Result<Option<String>> {
    let file_path = if path.is_dir() {
        path.join(JOURNAL_FILE)
    } else {
        path.to_path_buf()
    };
    let file = File::open(&file_path)
        .with_context(|| format!("opening journal {}", file_path.display()))?;
    let mut cwd = None;
    for line in BufReader::new(file).lines() {
        let line = line.context("reading journal line")?;
        // Every provider spells the field `cwd`, so a line without those three
        // characters cannot carry one. Worth checking first: this walks whole
        // journals, and the two JSON parses below are the cost.
        if !line.contains("cwd") {
            continue;
        }
        let Ok(Record::Agent {
            raw,
            protocol: record_protocol,
            ..
        }) = serde_json::from_str::<Record>(&line)
        else {
            continue;
        };
        if let Some(reported) =
            crate::event::reported_cwd(record_protocol.unwrap_or(protocol), &raw)
        {
            cwd = Some(reported);
        }
    }
    Ok(cwd)
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A sortable, collision-resistant-enough session id: millisecond timestamp,
/// process id, and a process-local counter. Not cryptographic; unique per host.
fn new_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:013}-{}-{}", now_ms(), std::process::id(), seq)
}

#[cfg(test)]
mod tests;
