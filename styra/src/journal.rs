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
//! store — without depending on an operator re-supplying the same profile.

use crate::agent::{Profile, SessionMeta};
use crate::event::{decode_line, Protocol, AgentEvent};
use crate::session::{Direction, RawLine};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

    /// Create a fresh session directory under `store_root` and open its
    /// journal, returning the generated session id alongside the handle.
    /// `profile` is the agent launching this session; its provenance is
    /// written once as [`SessionMeta`] beside the journal.
    pub fn create_in_store(store_root: &Path, profile: &Profile) -> Result<(Self, String)> {
        let id = new_session_id();
        let directory = sessions_dir(store_root).join(&id);
        let journal = Self::create(&directory)?;
        write_session_meta(&directory, &SessionMeta::for_profile(profile))?;
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

/// The sessions directory within a Styra store root (default store: `.styra`).
pub fn sessions_dir(store_root: &Path) -> PathBuf {
    store_root.join("sessions")
}

fn write_session_meta(directory: &Path, meta: &SessionMeta) -> Result<()> {
    let path = directory.join(SESSION_META_FILE);
    let json = serde_json::to_string_pretty(meta).context("serializing session metadata")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

/// A stored session, enough to display and select it from a list — see
/// [`list_sessions`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    /// The session's directory name, and the id `--view` expects.
    pub id: String,
    /// Its directory, ready to pass straight to `--view`.
    pub path: PathBuf,
    /// The agent that produced it, if recorded (see [`read_session_meta`]);
    /// `None` for a session that predates provenance tracking.
    pub profile: Option<String>,
    /// Roughly how long ago it was created, e.g. "3h ago".
    pub age: String,
    /// The millisecond timestamp embedded in `id`, used to sort newest
    /// first; `None` for an id that doesn't match the expected shape.
    pub created_at_ms: Option<u64>,
}

/// List every session stored under `store_root`, newest first. An absent
/// sessions directory (nothing has ever been recorded) is an empty list, not
/// an error.
pub fn list_sessions(store_root: &Path) -> Result<Vec<SessionSummary>> {
    let dir = sessions_dir(store_root);
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
        let profile = read_session_meta(&path)?.map(|meta| meta.profile);
        let created_at_ms = session_created_at_ms(&id);
        sessions.push(SessionSummary {
            id,
            path,
            profile,
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

/// Read back which agent produced a stored session, if recorded. A journal
/// directory or its file may be passed. Sessions created before this sidecar
/// existed have none, so `Ok(None)` — not an error — means "unknown".
pub fn read_session_meta(path: &Path) -> Result<Option<SessionMeta>> {
    let directory = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let meta_path = directory.join(SESSION_META_FILE);
    if !meta_path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&meta_path)
        .with_context(|| format!("reading {}", meta_path.display()))?;
    let meta = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", meta_path.display()))?;
    Ok(Some(meta))
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
                AgentEvent::UserMessage { text: "do the thing".into() },
                AgentEvent::AgentMessage { text: "done".into() },
                AgentEvent::UserMessage { text: "thanks".into() },
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
            single_turn: true,
        }
    }

    #[test]
    fn create_in_store_makes_a_unique_session_directory() {
        let root = temp_dir("store");
        let profile = test_profile("codex-exec", Protocol::CodexJsonl);
        let (journal_a, id_a) = Journal::create_in_store(&root, &profile).unwrap();
        let (journal_b, id_b) = Journal::create_in_store(&root, &profile).unwrap();
        assert_ne!(id_a, id_b, "session ids must be unique");
        assert!(journal_a.path().exists());
        assert!(journal_b.path().exists());
        assert!(journal_a.path().starts_with(sessions_dir(&root)));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn create_in_store_persists_which_agent_produced_the_session() {
        let root = temp_dir("provenance");
        let profile = test_profile("claude:opus", Protocol::ClaudeJsonl);
        let (journal, _id) = Journal::create_in_store(&root, &profile).unwrap();

        let meta = read_session_meta(journal.path()).unwrap();
        assert_eq!(
            meta,
            Some(SessionMeta { profile: "claude:opus".into(), protocol: Protocol::ClaudeJsonl })
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_session_without_stored_meta_reads_as_unknown_not_an_error() {
        // Sessions created before this sidecar existed have no session.json;
        // that must read as "unknown", not fail.
        let dir = temp_dir("no-meta");
        Journal::create(&dir).unwrap();

        assert_eq!(read_session_meta(&dir).unwrap(), None);

        std::fs::remove_dir_all(&dir).ok();
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
    fn list_sessions_is_empty_when_the_store_has_no_sessions_yet() {
        let root = temp_dir("list-empty");
        assert_eq!(list_sessions(&root).unwrap(), Vec::new());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_sessions_finds_created_sessions_with_their_profile() {
        let root = temp_dir("list");
        let codex = test_profile("codex", Protocol::CodexJsonl);
        let claude = test_profile("claude:opus", Protocol::ClaudeJsonl);
        let (_journal_a, id_a) = Journal::create_in_store(&root, &codex).unwrap();
        let (_journal_b, id_b) = Journal::create_in_store(&root, &claude).unwrap();

        let sessions = list_sessions(&root).unwrap();
        assert_eq!(sessions.len(), 2);
        let by_id = |id: &str| sessions.iter().find(|s| s.id == id).unwrap();
        assert_eq!(by_id(&id_a).profile.as_deref(), Some("codex"));
        assert_eq!(by_id(&id_b).profile.as_deref(), Some("claude:opus"));
        assert!(sessions.iter().all(|s| s.created_at_ms.is_some()));
        assert!(sessions.iter().all(|s| s.path.is_dir()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_sessions_reports_an_unknown_profile_for_sessions_without_metadata() {
        // Sessions created before the session.json sidecar existed have none;
        // they should still be listed, just without a profile.
        let root = temp_dir("list-no-meta");
        Journal::create(&sessions_dir(&root).join("legacy-session")).unwrap();

        let sessions = list_sessions(&root).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "legacy-session");
        assert_eq!(sessions[0].profile, None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sessions_sort_newest_first_with_unknown_age_last() {
        let summary = |created_at_ms: Option<u64>| SessionSummary {
            id: format!("{created_at_ms:?}"),
            path: PathBuf::new(),
            profile: None,
            age: String::new(),
            created_at_ms,
        };
        let mut sessions =
            vec![summary(Some(100)), summary(None), summary(Some(300)), summary(Some(200))];
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
