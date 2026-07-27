//! Stable JSON contract shared by the Styra Unix-socket server and its clients.

use crate::agent::Selection;
use crate::event::AgentEvent;
use crate::types::{
    DrivaOptions, InteractionSummary, InteractionUpdate, RawLine, SessionSummary, WorkspaceSummary,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    SendMessage {
        id: String,
        message: SendMessage,
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
    Accepted,
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
    use crate::types::{InteractionUpdate, LogEntry};

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
}
