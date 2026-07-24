//! Stable JSON contract shared by the Styra Unix-socket server and its clients.

use crate::event::AgentEvent;
use crate::types::{DrivaOptions, RawLine, SessionSummary, SessionUpdate};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const API_VERSION: &str = "v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub service: String,
    pub api_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSession {
    pub profile: String,
    pub workspace: PathBuf,
    #[serde(default)]
    pub network: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub profile: String,
    pub workspace: PathBuf,
    pub journal_path: PathBuf,
    pub driva: DrivaOptions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessage {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencedUpdate {
    pub sequence: u64,
    pub update: SessionUpdate,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
}

/// One JSON request sent as a single line over the Unix socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "data", rename_all = "snake_case")]
pub enum Request {
    Health,
    CreateSession(CreateSession),
    SendMessage { id: String, message: SendMessage },
    StopSession { id: String },
    Updates { id: String, after: u64 },
    ListStoredSessions,
    StoredSession { id: String },
    Transcript { id: String },
}

/// Versioned request envelope. Flattening keeps `operation` at the top level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireRequest {
    pub api_version: String,
    #[serde(flatten)]
    pub request: Request,
}

/// Successful response payload. The variant must match the request operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Health(Health),
    SessionCreated(SessionInfo),
    Accepted,
    Updates(Updates),
    StoredSessions(Vec<SessionSummary>),
    StoredSession(StoredSession),
    Transcript(Transcript),
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
    use crate::types::{LogEntry, SessionUpdate};

    #[test]
    fn update_stream_has_an_explicit_cursor_and_tagged_payload() {
        let response = Updates {
            updates: vec![SequencedUpdate {
                sequence: 4,
                update: SessionUpdate::Log(LogEntry::info("ready")),
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
        let request = WireRequest {
            api_version: API_VERSION.into(),
            request: Request::Updates {
                id: "s-1".into(),
                after: 8,
            },
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["api_version"], "v1");
        assert_eq!(json["operation"], "updates");
        assert_eq!(json["data"]["id"], "s-1");
        assert_eq!(json["data"]["after"], 8);
        assert_eq!(
            serde_json::from_value::<WireRequest>(json).unwrap(),
            request
        );
    }
}
