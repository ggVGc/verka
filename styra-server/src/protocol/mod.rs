//! Stable JSON contract shared by the Styra Unix-socket server and its clients.
//!
//! This module is the single entry point for everything that crosses the
//! Styra client/server boundary. See `protocol/README.md` for the transport
//! framing and behavioral rules.

use crate::agent::Selection;
use crate::event::AgentEvent;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod transport;
mod types;

pub use transport::{read_message, read_message_limited, write_message, MAX_REQUEST_BYTES};
pub use types::{
    Direction, DrivaOptions, InteractionActivity, InteractionEnd, InteractionSummary,
    InteractionUpdate, LogEntry, LogLevel, RawLine, SessionSummary, WorkspaceSummary,
};

// These external vocabularies are serialized inside protocol payloads. Re-export
// them here so the complete wire surface is discoverable from this module.
pub use crate::agent::{Effort, Provider, Selection as AgentSelection};
pub use crate::event::AgentEvent as ProtocolAgentEvent;
pub use crate::{Mount, MountAccess};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub service: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspace {
    pub host_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSession {
    pub workspace_id: String,
    pub selection: Selection,
    #[serde(default)]
    pub network: bool,
    /// Named Driva execution templates (see `driva templates`), applied as an
    /// additive overlay on top of the profile's own mounts, environment, and
    /// network policy. Later names in the list take precedence on conflict.
    #[serde(default)]
    pub templates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional operator-facing name. When absent, the server derives one
    /// from `message` when possible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameSession {
    pub id: String,
    /// `None` (or whitespace-only text) clears the name.
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateNotes {
    pub id: String,
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSession {
    pub id: String,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub templates: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub workspace_id: String,
    pub selection: Selection,
    pub workspace: PathBuf,
    pub journal_path: PathBuf,
    pub driva: DrivaOptions,
    /// Update cursor immediately after any journal history used to initialize
    /// this interaction. The client which already rendered that journal starts
    /// here; a later attachment starts at zero to receive the full history.
    #[serde(default)]
    pub updates_after: u64,
    /// Operator messages the interaction has queued but not yet sent, carried
    /// over from a previous attachment (e.g. one stopped before the interaction
    /// went idle), so a fresh client hydrates its input queue instead of
    /// silently dropping it.
    #[serde(default)]
    pub queued: Vec<String>,
}

/// Host-side tmux endpoint for the shell owned by a live session's sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellInfo {
    pub tmux: PathBuf,
    pub socket: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessage {
    pub text: String,
    /// Per-turn Codex selection. Older clients omit this and retain the
    /// interaction's current defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Selection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencedUpdate {
    pub sequence: u64,
    pub update: InteractionUpdate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Updates {
    pub updates: Vec<SequencedUpdate>,
    /// Cursor to pass as `after` on the next request.
    pub next: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredSession {
    pub summary: SessionSummary,
    pub events: Vec<AgentEvent>,
    pub raw: Vec<RawLine>,
}

/// One JSON request sent as a single line over the Unix socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Request {
    Health,
    CreateWorkspace(CreateWorkspace),
    ListWorkspaces,
    Workspace {
        id: String,
    },
    CreateSession(CreateSession),
    ResumeSession(ResumeSession),
    RenameSession(RenameSession),
    UpdateSessionNotes(UpdateNotes),
    UpdateWorkspaceNotes(UpdateNotes),
    SendMessage {
        id: String,
        message: SendMessage,
    },
    /// Persist an operator message in the session's durable input queue
    /// without sending it yet, so it survives the client disconnecting before
    /// the interaction is idle enough to accept it.
    QueueMessage {
        id: String,
        message: SendMessage,
    },
    /// Pop the oldest durably queued message, if any, for the client to send.
    TakeQueuedMessage {
        id: String,
    },
    /// Read back the session's durably queued, not-yet-sent messages.
    QueuedMessages {
        id: String,
    },
    /// Discard the session's durably queued messages.
    ClearQueuedMessages {
        id: String,
    },
    InterruptInteraction {
        id: String,
    },
    StopInteraction {
        id: String,
    },
    Updates {
        id: String,
        after: u64,
    },
    ListInteractions,
    ListSessions {
        workspace_id: String,
    },
    StoredSession {
        id: String,
    },
    Shell {
        id: String,
    },
    /// Ask the server to remove its socket and exit. Any live interactions it owns die
    /// with it, so this is the deliberate counterpart to the daemon outliving
    /// its clients.
    Shutdown,
}

/// Versioned request envelope. Flattening keeps `operation` at the top level.
/// Successful response payload. The variant must match the request operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Health(Health),
    WorkspaceCreated(WorkspaceSummary),
    Workspaces(Vec<WorkspaceSummary>),
    Workspace(WorkspaceSummary),
    SessionCreated(SessionInfo),
    SessionResumed(SessionInfo),
    SessionRenamed(SessionSummary),
    SessionNotesUpdated(SessionSummary),
    WorkspaceNotesUpdated(WorkspaceSummary),
    Accepted,
    Queued(usize),
    TakenQueuedMessage(Option<String>),
    QueuedMessages(Vec<String>),
    Updates(Updates),
    Interactions(Vec<InteractionSummary>),
    StoredSessions(Vec<SessionSummary>),
    StoredSession(StoredSession),
    Shell(ShellInfo),
}

/// Response envelope returned for every syntactically valid connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WireResponse {
    Ok { response: Response },
    Error { error: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_stream_has_an_explicit_cursor_and_tagged_payload() {
        let response = Updates {
            updates: vec![SequencedUpdate {
                sequence: 4,
                update: InteractionUpdate::Log(LogEntry::info("ready")),
            }],
            next: 4,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["next"], 4);
        assert_eq!(json["updates"][0]["sequence"], 4);
        assert_eq!(json["updates"][0]["update"]["type"], "log");
        assert_eq!(json["updates"][0]["update"]["data"]["message"], "ready");
        assert_eq!(serde_json::from_value::<Updates>(json).unwrap(), response);
    }

    #[test]
    fn requests_are_self_describing_json_messages() {
        let request = Request::Updates {
            id: "s-1".into(),
            after: 8,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "updates");
        assert_eq!(json["data"]["id"], "s-1");
        assert_eq!(json["data"]["after"], 8);
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
        assert!(
            serde_json::from_str::<Request>(r#"{"api_version":"v3","operation":"health"}"#)
                .is_err()
        );
    }

    #[test]
    fn session_creation_carries_a_structured_agent_selection() {
        let request = Request::CreateSession(CreateSession {
            workspace_id: "w-1".into(),
            selection: crate::agent::Selection {
                provider: crate::agent::Provider::Claude,
                model: "claude-opus-5".into(),
                effort: crate::agent::Effort::XHigh,
            },
            network: false,
            templates: Vec::new(),
            message: None,
            name: None,
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["data"]["selection"]["provider"], "claude");
        assert_eq!(json["data"]["selection"]["model"], "claude-opus-5");
        assert_eq!(json["data"]["selection"]["effort"], "xhigh");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }

    #[test]
    fn resume_names_only_the_existing_session_and_launch_policy() {
        let request = Request::ResumeSession(ResumeSession {
            id: "styra-1".into(),
            network: true,
            templates: vec!["rust".into()],
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "resume_session");
        assert_eq!(json["data"]["id"], "styra-1");
        assert_eq!(json["data"]["templates"][0], "rust");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }

    #[test]
    fn rename_session_carries_a_nullable_display_name() {
        let request = Request::RenameSession(RenameSession {
            id: "styra-1".into(),
            name: Some("Fix session picker".into()),
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "rename_session");
        assert_eq!(json["data"]["id"], "styra-1");
        assert_eq!(json["data"]["name"], "Fix session picker");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }
}
