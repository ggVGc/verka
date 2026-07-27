//! The data vocabulary that crosses the Styra socket boundary.
//!
//! These are the types a client receives and renders: the live update stream
//! ([`InteractionUpdate`] and its parts), the captured Driva policy
//! ([`DrivaOptions`]), and the stored-session listing ([`SessionSummary`]).
//! They carry no behaviour tied to running an interaction — the server machinery
//! that produces them lives in [`crate::interaction`], [`crate::journal`], and
//! [`crate::server`]. Keeping them here lets a client depend on the interface
//! without pulling in the interaction runner.

use crate::event::AgentEvent;
use driva::Mount;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A durable Styra Workspace, which groups provider Sessions that operate on
/// the same host directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    /// The Workspace directory name and stable wire identifier.
    pub id: String,
    /// Optional operator-facing name. The host directory name is the display
    /// fallback when this is absent.
    pub name: Option<String>,
    /// Canonical host directory mounted into Sessions in this Workspace.
    pub host_path: PathBuf,
    /// Directory holding `workspace.json` and this Workspace's Sessions.
    pub path: PathBuf,
    /// Number of durable Sessions currently stored in the Workspace.
    pub session_count: usize,
    /// Roughly how long ago the Workspace was created.
    pub age: String,
    /// Millisecond creation timestamp used to sort newest first.
    pub created_at_ms: u64,
}

/// An update delivered from the interaction's threads to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum InteractionUpdate {
    /// A decoded agent event or an operator message, in occurrence order.
    Event(AgentEvent),
    /// One verbatim wire line, for the raw-interaction view.
    Raw(RawLine),
    /// A diagnostic message for the log view.
    Log(LogEntry),
    /// The agent process ended; no further events will arrive.
    Ended(InteractionEnd),
}

/// Severity of a [`LogEntry`], used to colour the log view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line in the log view: a Styra-internal note or a line of agent stderr.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Info,
            message: message.into(),
        }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Warn,
            message: message.into(),
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Error,
            message: message.into(),
        }
    }
}

/// Which way a wire line travelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// A line Styra wrote to the agent's stdin.
    ToAgent,
    /// A line received on the agent's stdout.
    FromAgent,
}

/// One verbatim line of the agent interaction, undecoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLine {
    pub direction: Direction,
    pub text: String,
}

/// How an interaction finished.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionEnd {
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

/// A human-facing summary of the Driva policy an interaction was launched with:
/// the isolation backend, the command it runs, and the mount/network policy
/// enforced around it. Captured once at spawn time from the same
/// `ExecutionRequest` Driva itself executes (see [`DrivaOptions::capture`] in
/// [`crate::interaction`]), so it can never drift from what is actually running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrivaOptions {
    pub isolation_backend: String,
    pub command: Vec<String>,
    pub working_directory: PathBuf,
    pub network: bool,
    pub mounts: Vec<Mount>,
}

/// An interaction the server is currently running (this process's live sessions),
/// enough to list it and to reattach a client to it. Distinct from
/// [`SessionSummary`], which describes a session persisted in the store
/// whether or not it is still live.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionSummary {
    /// The session id, as used everywhere else on the wire.
    pub id: String,
    /// The agent profile the interaction is running.
    pub profile: String,
    /// The host directory bound as the agent's workspace, so a reattaching
    /// client can resolve changed-file previews.
    pub workspace: PathBuf,
    /// The Driva policy the interaction was launched under, for the driva view.
    pub driva: DrivaOptions,
    /// Whether the interaction still takes messages: its agent process is alive and,
    /// for a single-turn profile, it has not spent its one turn yet.
    pub accepting: bool,
}

/// A stored session, enough to display and select it from a list — see
/// [`crate::journal::list_sessions`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// The session's directory name, and the id `--view` expects.
    pub id: String,
    /// The owning Workspace. `None` identifies a legacy flat-store Session.
    pub workspace_id: Option<String>,
    /// Its directory, ready to pass straight to `--view`.
    pub path: PathBuf,
    /// The agent that produced it, if recorded (see
    /// [`crate::journal::read_session_meta`]); `None` for a session that
    /// predates provenance tracking.
    pub profile: Option<String>,
    /// Roughly how long ago it was created, e.g. "3h ago".
    pub age: String,
    /// The millisecond timestamp embedded in `id`, used to sort newest
    /// first; `None` for an id that doesn't match the expected shape.
    pub created_at_ms: Option<u64>,
}
