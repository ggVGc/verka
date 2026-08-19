//! The data vocabulary that crosses the Styra socket boundary.
//!
//! These are the types a client receives and renders: the live update stream
//! ([`InteractionUpdate`] and its parts), the captured Driva policy
//! ([`DrivaOptions`]), and the stored-session listing ([`SessionSummary`]).
//! They carry no behaviour tied to running an interaction — the server machinery
//! that produces them lives in [`crate::interaction`], [`crate::journal`], and
//! [`crate::server`]. Keeping them here lets a client depend on the interface
//! without pulling in the interaction runner.

use crate::agent::Selection;
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
    /// Operator-authored notes shared by every Session in the Workspace.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Canonical host directory mounted into Sessions in this Workspace.
    pub host_path: PathBuf,
    /// Directory holding `workspace.json` and this Workspace's Sessions.
    pub path: PathBuf,
    /// Number of durable Sessions currently stored in the Workspace.
    pub session_count: usize,
    /// Roughly how long ago the Workspace was created.
    pub age: String,
    /// Millisecond creation timestamp retained for display and tie-breaking.
    pub created_at_ms: u64,
    /// Millisecond timestamp of the most recent explicit access.
    #[serde(default)]
    pub last_accessed_at_ms: u64,
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
    /// The host directory used for subsequent agent turns.
    WorkingDirectoryChanged(PathBuf),
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

/// One extra host directory the operator asked to be bound into the sandbox,
/// on top of what the profile and the selected templates already grant.
///
/// This is a *request*, not a resolved mount: the source is whatever the
/// operator typed, and the server canonicalizes it (rejecting a path that does
/// not exist) before it becomes part of a launch. An absent `destination`
/// means "the same path inside the sandbox", matching Driva's own rule for a
/// bind mount with no destination.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchMount {
    pub source: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    /// Read-only unless the operator explicitly asked for write access, so the
    /// quiet default is the one that grants least.
    #[serde(default)]
    pub writable: bool,
}

/// A Driva execution template the server can offer, named and described, so a
/// client can present the real set rather than asking the operator to recall
/// template names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// What a live interaction is currently waiting on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionActivity {
    /// The agent is idle and waiting for the operator's next message.
    #[default]
    Pending,
    /// The agent is working on an operator message.
    Running,
    /// The agent is waiting for input while a background task is active.
    Background,
}

/// An interaction the server is currently running (this process's live sessions),
/// enough to list it and to reattach a client to it. Distinct from
/// [`SessionSummary`], which describes a session persisted in the store
/// whether or not it is still live.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionSummary {
    /// The session id, as used everywhere else on the wire.
    pub id: String,
    /// Optional operator-facing Session name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The Workspace containing the Session served by this Interaction.
    pub workspace_id: String,
    /// The provider, model, and effort the interaction is running.
    pub selection: Selection,
    /// The host directory bound as the agent's workspace, so a reattaching
    /// client can resolve changed-file previews.
    pub workspace: PathBuf,
    /// The Driva policy the interaction was launched under, for the driva view.
    pub driva: DrivaOptions,
    /// Whether the interaction's agent process is alive and still takes messages.
    pub accepting: bool,
    /// Whether the live interaction is working or waiting for user input.
    #[serde(default)]
    pub activity: InteractionActivity,
}

/// A stored session, enough to display and select it from a list — see
/// [`crate::journal::list_sessions`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// The session's directory name, and the id `--view` expects.
    pub id: String,
    /// Optional operator-facing name; the stable id remains the identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Operator-authored notes specific to this Session.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The owning Workspace.
    pub workspace_id: String,
    /// Its directory, ready to pass straight to `--view`.
    pub path: PathBuf,
    /// The provider, model, and effort that produced it.
    pub selection: Selection,
    /// Roughly how long ago it was created, e.g. "3h ago".
    pub age: String,
    /// The millisecond timestamp embedded in `id`, used to sort newest
    /// first; `None` for an id that doesn't match the expected shape.
    pub created_at_ms: Option<u64>,
}
