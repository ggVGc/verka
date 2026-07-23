//! The append-only session journal and its replay.
//!
//! The journal is the fundamental record of a session: an ordered log of
//! source-tagged records. An agent record holds the verbatim line received on
//! the agent's stdout; an operator record holds a message the operator sent.
//! Append order is receive order, so the single file reconstructs the whole
//! session with agent and operator turns interleaved. Nothing rendered is
//! stored — [`replay`] reproduces events on demand through the protocol
//! decoder, exactly as a live session decodes them.

use crate::event::{decode_line, Protocol, StyraEvent};
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
    pub fn create_in_store(store_root: &Path) -> Result<(Self, String)> {
        let id = new_session_id();
        let directory = sessions_dir(store_root).join(&id);
        let journal = Self::create(&directory)?;
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

/// The sessions directory within a Styra store root (default store: `.styra`).
pub fn sessions_dir(store_root: &Path) -> PathBuf {
    store_root.join("sessions")
}

/// Decode a stored journal back into the ordered event list, reproducing agent
/// events through `protocol` and operator turns as [`StyraEvent::UserMessage`].
/// A journal directory or its file may be passed.
pub fn replay(path: &Path, protocol: Protocol) -> Result<Vec<StyraEvent>> {
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
            Ok(Record::User { text, .. }) => events.push(StyraEvent::UserMessage { text }),
            Err(error) => events.push(StyraEvent::Malformed {
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
                StyraEvent::UserMessage { text: "do the thing".into() },
                StyraEvent::AgentMessage { text: "done".into() },
                StyraEvent::UserMessage { text: "thanks".into() },
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

    #[test]
    fn create_in_store_makes_a_unique_session_directory() {
        let root = temp_dir("store");
        let (journal_a, id_a) = Journal::create_in_store(&root).unwrap();
        let (journal_b, id_b) = Journal::create_in_store(&root).unwrap();
        assert_ne!(id_a, id_b, "session ids must be unique");
        assert!(journal_a.path().exists());
        assert!(journal_b.path().exists());
        assert!(journal_a.path().starts_with(sessions_dir(&root)));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_corrupt_record_is_surfaced_not_fatal() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(JOURNAL_FILE), "not a record\n").unwrap();
        let events = replay(&dir, Protocol::CodexJsonl).unwrap();
        assert!(matches!(events.as_slice(), [StyraEvent::Malformed { .. }]));

        std::fs::remove_dir_all(&dir).ok();
    }
}
