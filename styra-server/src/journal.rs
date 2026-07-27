//! The append-only session journal and its replay.
//!
//! The journal is the fundamental record of a session: an ordered log of
//! source-tagged records. An agent record holds the verbatim line received on
//! the agent's stdout; an operator record holds a message the operator sent.
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
use crate::event::{decode_line, AgentEvent, Protocol};
use crate::types::{Direction, RawLine, SessionSummary};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct StoredSessionMeta {
    workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_session_id: Option<String>,
    #[serde(flatten)]
    agent: SessionMeta,
}

/// One line of the journal. Tagged by source so replay knows whether to decode
/// the record as an agent wire line or surface it as an operator message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
enum Record {
    /// A line received verbatim on the agent's stdout.
    Agent { at_ms: u64, raw: String },
    /// A message the operator sent to the agent.
    User { at_ms: u64, text: String },
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
    ) -> Result<(Self, String)> {
        crate::workspace::get(store_root, workspace_id)?;
        let id = new_session_id();
        let directory = crate::workspace::sessions_dir(store_root, workspace_id).join(&id);
        let journal = Self::create(&directory)?;
        write_session_meta(
            &directory,
            &SessionMeta::new(selection.clone(), profile.protocol),
            workspace_id,
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
        })
    }

    /// Record a message the operator sent to the agent.
    pub fn record_user_message(&mut self, text: &str) -> Result<()> {
        self.write(&Record::User {
            at_ms: now_ms(),
            text: text.to_owned(),
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

fn write_session_meta(directory: &Path, meta: &SessionMeta, workspace_id: &str) -> Result<()> {
    let path = directory.join(SESSION_META_FILE);
    let stored = StoredSessionMeta {
        workspace_id: workspace_id.to_owned(),
        provider_session_id: None,
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

fn list_sessions_at(dir: &Path, workspace_id: &str) -> Result<Vec<SessionSummary>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let now = now_ms();
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading an entry in {}", dir.display()))?;
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let selection = read_session_meta(&path)?.selection;
        let created_at_ms = session_created_at_ms(&id);
        sessions.push(SessionSummary {
            id,
            workspace_id: workspace_id.to_owned(),
            path,
            selection,
            age: humanize_age(now, created_at_ms),
            created_at_ms,
        });
    }
    sort_newest_first(&mut sessions);
    Ok(sessions)
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
    let meta_path = directory.join(SESSION_META_FILE);
    let json = serde_json::to_string_pretty(&stored).context("serializing session metadata")?;
    std::fs::write(&meta_path, json).with_context(|| format!("writing {}", meta_path.display()))
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
    let file_path = if path.is_dir() {
        path.join(JOURNAL_FILE)
    } else {
        path.to_path_buf()
    };
    let file = File::open(&file_path)
        .with_context(|| format!("opening journal {}", file_path.display()))?;
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.context("reading journal line")?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(&line) {
            Ok(Record::Agent { raw, .. }) => events.push(decode_line(protocol, &raw)),
            Ok(Record::User { text, .. }) => events.push(AgentEvent::UserMessage { text }),
            Err(error) => events.push(AgentEvent::Malformed {
                error: format!("unreadable journal record: {error}"),
            }),
        }
    }
    Ok(events)
}

/// Reconstruct the raw interaction from a stored journal: each agent record is
/// its verbatim line, each operator record the message text that was sent.
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
            Ok(Record::Agent { raw: text, .. }) => raw.push(RawLine {
                direction: Direction::FromAgent,
                text,
            }),
            Ok(Record::User { text, .. }) => raw.push(RawLine {
                direction: Direction::ToAgent,
                text,
            }),
            Err(_) => {}
        }
    }
    Ok(raw)
}

fn now_ms() -> u64 {
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
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("styra-journal-{tag}-{}", new_session_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn records_replay_in_receive_order_with_interleaved_turns() {
        let dir = temp_dir("order");
        {
            let mut journal = Journal::create(&dir).unwrap();
            journal.record_user_message("do the thing").unwrap();
            journal
                .record_agent_line(
                    r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
                )
                .unwrap();
            journal.record_user_message("thanks").unwrap();
        }

        let events = replay(&dir, Protocol::CodexJsonl).unwrap();
        assert_eq!(
            events,
            vec![
                AgentEvent::UserMessage {
                    text: "do the thing".into()
                },
                AgentEvent::AgentMessage {
                    text: "done".into()
                },
                AgentEvent::UserMessage {
                    text: "thanks".into()
                },
            ]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agent_lines_are_stored_verbatim() {
        let dir = temp_dir("verbatim");
        let raw = r#"{"type":"turn.completed","usage":{"input_tokens":5}}"#;
        {
            let mut journal = Journal::create(&dir).unwrap();
            journal.record_agent_line(raw).unwrap();
        }
        // Verbatim means the raw line round-trips: replaying it decodes to
        // exactly what decoding the original line directly produces.
        let events = replay(&dir, Protocol::CodexJsonl).unwrap();
        assert_eq!(events, vec![decode_line(Protocol::CodexJsonl, raw)]);

        std::fs::remove_dir_all(&dir).ok();
    }

    fn test_profile(name: &str, protocol: Protocol) -> Profile {
        Profile {
            name: name.into(),
            command: vec!["true".into()],
            protocol,
            mounts: Vec::new(),
            environment: Default::default(),
            network: false,
            message_format: crate::agent::MessageFormat::PlainLine,
            single_turn: false,
        }
    }

    #[test]
    fn workspace_sessions_are_nested_and_record_their_owner() {
        let root = temp_dir("workspace-session");
        let host = temp_dir("workspace-host");
        let workspace = crate::workspace::create(&root, &host, Some("work".into())).unwrap();
        let profile = test_profile("codex", Protocol::CodexJsonl);

        let selection = crate::agent::Selection::new(crate::agent::Provider::Codex);
        let (journal, id) =
            Journal::create_in_workspace(&root, &workspace.id, &profile, &selection).unwrap();
        let directory = journal.path().parent().unwrap();
        assert_eq!(
            directory,
            crate::workspace::sessions_dir(&root, &workspace.id).join(&id)
        );
        assert_eq!(read_session_workspace_id(directory).unwrap(), workspace.id);
        assert_eq!(read_provider_session_id(directory).unwrap(), None);
        store_provider_session_id(directory, "provider-1").unwrap();
        assert_eq!(
            read_provider_session_id(directory).unwrap().as_deref(),
            Some("provider-1")
        );

        let sessions = list_workspace_sessions(&root, &workspace.id).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].workspace_id, workspace.id);
        assert_eq!(
            sessions[0].selection,
            crate::agent::Selection::new(crate::agent::Provider::Codex)
        );
        assert_eq!(
            crate::workspace::get(&root, &workspace.id)
                .unwrap()
                .session_count,
            1
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&host).ok();
    }

    #[test]
    fn a_corrupt_record_is_surfaced_not_fatal() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(JOURNAL_FILE), "not a record\n").unwrap();
        let events = replay(&dir, Protocol::CodexJsonl).unwrap();
        assert!(matches!(events.as_slice(), [AgentEvent::Malformed { .. }]));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sessions_sort_newest_first_with_unknown_age_last() {
        let summary = |created_at_ms: Option<u64>| SessionSummary {
            id: format!("{created_at_ms:?}"),
            workspace_id: "w-1".into(),
            path: PathBuf::new(),
            selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
            age: String::new(),
            created_at_ms,
        };
        let mut sessions = vec![
            summary(Some(100)),
            summary(None),
            summary(Some(300)),
            summary(Some(200)),
        ];
        sort_newest_first(&mut sessions);
        let order: Vec<Option<u64>> = sessions.iter().map(|s| s.created_at_ms).collect();
        assert_eq!(order, vec![Some(300), Some(200), Some(100), None]);
    }

    #[test]
    fn session_created_at_ms_parses_the_leading_timestamp_field() {
        assert_eq!(session_created_at_ms("0000000123456-42-7"), Some(123456));
        assert_eq!(session_created_at_ms("not-an-id"), None);
        assert_eq!(session_created_at_ms(""), None);
    }

    #[test]
    fn humanize_age_buckets_elapsed_time() {
        assert_eq!(humanize_age(1_000, None), "unknown");
        assert_eq!(humanize_age(10_000, Some(9_500)), "just now");
        assert_eq!(humanize_age(200_000, Some(0)), "3m ago");
        assert_eq!(humanize_age(3_600_000 * 2, Some(0)), "2h ago");
        assert_eq!(humanize_age(86_400_000 * 5, Some(0)), "5d ago");
    }
}
