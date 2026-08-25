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
    InteractionUpdate, LaunchMount, LaunchPolicy, LogEntry, LogLevel, RawLine, SessionOrigin,
    SessionSummary, TemplateSummary, WorkspaceSummary,
};

// These external vocabularies are serialized inside protocol payloads. Re-export
// them here so the complete wire surface is discoverable from this module.
pub use crate::agent::{Effort, Provider, Selection as AgentSelection};
pub use crate::event::AgentEvent as ProtocolAgentEvent;
pub use crate::{Mount, MountAccess};

/// `serde` default for the protocol's opt-out booleans, whose absence must
/// mean "as before".
fn yes() -> bool {
    true
}

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
    /// This launch's own sandbox policy, layered over the Workspace's standing
    /// one (see [`LaunchPolicy::merge`]). Templates are names and mounts are
    /// requests; the server resolves both after merging.
    #[serde(default)]
    pub launch: LaunchPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional operator-facing name. When absent, the server derives one
    /// from `message` when possible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Ask what a new session in this Workspace *would* be launched under, without
/// creating one. Carries exactly the launch inputs of [`CreateSession`] that
/// shape the sandbox, so the answer is the policy that session would get.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSession {
    pub workspace_id: String,
    pub selection: Selection,
    #[serde(default)]
    pub launch: LaunchPolicy,
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
    pub launch: LaunchPolicy,
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
    /// Empty when the request asked for no raw lines.
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
    /// Report the Driva policy a `CreateSession` with these inputs would run
    /// under. Creates nothing and touches no session state.
    PlanSession(PlanSession),
    /// Name the Driva templates a session in this Workspace could be launched
    /// with: Driva's built-ins, overridden by any `driva.toml` the Workspace
    /// carries. Resolves the same set `templates` on a launch request is
    /// looked up in, so a client can offer exactly what would be accepted.
    ListTemplates {
        workspace_id: String,
    },
    ResumeSession(ResumeSession),
    /// Convert a stored Session's native provider transcript (Codex rollout or
    /// Claude project JSONL) to the other interactive provider's format,
    /// using Genta's session conversion. The source Session and its native
    /// transcript are left untouched; the result is a new sibling Session in
    /// the same Workspace, ready to resume under the other provider. Sugar
    /// for [`Request::BranchSession`] with `at_ms: None` and the other
    /// provider named.
    ConvertSessionProvider {
        id: String,
    },
    /// Branch a stored Session's native provider transcript into a new
    /// sibling Session in the same Workspace, seeded with its history up to
    /// `at_ms` (the whole history when absent), optionally under a different
    /// provider. The source Session, its native transcript, and its Styra
    /// journal are left untouched.
    BranchSession {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<Provider>,
    },
    RenameSession(RenameSession),
    UpdateSessionNotes(UpdateNotes),
    UpdateWorkspaceNotes(UpdateNotes),
    /// Replace a Workspace's standing sandbox policy: the templates, mounts and
    /// network permission every launch in it starts from. Applies to launches
    /// made after it, not to interactions already running under the old one.
    SetWorkspaceLaunch {
        workspace_id: String,
        launch: LaunchPolicy,
    },
    SendMessage {
        id: String,
        message: SendMessage,
    },
    /// Switch a live interaction onto another model, applied now and recorded
    /// with the session so reopening it keeps the switch. The provider cannot
    /// change; that needs a new session.
    SetSessionSelection {
        id: String,
        selection: Selection,
    },
    /// Change the directory used by later turns of a live interaction. The
    /// path is on the host and must stay inside the interaction's Workspace.
    SetInteractionWorkingDirectory {
        id: String,
        directory: PathBuf,
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
    /// Stop an interaction and drop the server's record of it, so the Session
    /// is only what is stored on disk: it no longer appears in the
    /// current-interactions list and can be resumed like any other history.
    CloseInteraction {
        id: String,
    },
    Updates {
        id: String,
        after: u64,
        /// Include `InteractionUpdate::Raw` wire lines. Clients with no raw
        /// view (a preview pane) pass `false` and skip the bulk of a long
        /// interaction's volume. Defaults to `true`, which is what a client
        /// that predates the field expects.
        #[serde(default = "yes")]
        raw: bool,
    },
    ListInteractions,
    ListSessions {
        workspace_id: String,
    },
    StoredSession {
        id: String,
        /// Include the journal's verbatim wire lines. Reconstructing them
        /// re-reads the whole journal, so a caller that only renders decoded
        /// events passes `false`. Defaults to `true` for older clients.
        #[serde(default = "yes")]
        raw: bool,
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
    SessionPlan(DrivaOptions),
    Templates(Vec<TemplateSummary>),
    SessionResumed(SessionInfo),
    SessionConverted(SessionSummary),
    SessionBranched(SessionSummary),
    SessionRenamed(SessionSummary),
    SessionNotesUpdated(SessionSummary),
    WorkspaceNotesUpdated(WorkspaceSummary),
    WorkspaceLaunchUpdated(WorkspaceSummary),
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
            raw: true,
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "updates");
        assert_eq!(json["data"]["id"], "s-1");
        assert_eq!(json["data"]["after"], 8);
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
        // A client that predates the raw opt-out still gets raw lines.
        assert_eq!(
            serde_json::from_str::<Request>(
                r#"{"operation":"updates","data":{"id":"s-1","after":8}}"#
            )
            .unwrap(),
            request
        );
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
            launch: LaunchPolicy::default(),
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
    fn planning_carries_the_launch_inputs_without_naming_a_session() {
        let request = Request::PlanSession(PlanSession {
            workspace_id: "w-1".into(),
            selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
            launch: LaunchPolicy {
                network: Some(true),
                templates: vec!["browser".into()],
                mounts: vec![LaunchMount {
                    source: PathBuf::from("/srv/data"),
                    destination: None,
                    writable: true,
                }],
                standalone: false,
            },
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "plan_session");
        assert_eq!(json["data"]["workspace_id"], "w-1");
        assert_eq!(json["data"]["launch"]["network"], true);
        assert_eq!(json["data"]["launch"]["templates"][0], "browser");
        assert_eq!(json["data"]["launch"]["mounts"][0]["source"], "/srv/data");
        assert_eq!(json["data"]["launch"]["mounts"][0]["writable"], true);
        assert!(json["data"]["launch"]["mounts"][0]
            .get("destination")
            .is_none());
        // An overlay that asks for nothing says nothing on the wire, so a
        // Workspace's standing policy is what a bare launch runs under.
        assert!(json["data"]["launch"].get("standalone").is_none());
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

        let response = Response::SessionPlan(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: true,
            mounts: Vec::new(),
        });
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["type"], "session_plan");
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
    }

    #[test]
    fn resume_names_only_the_existing_session_and_launch_policy() {
        let request = Request::ResumeSession(ResumeSession {
            id: "styra-1".into(),
            launch: LaunchPolicy {
                network: Some(true),
                templates: vec!["rust".into()],
                ..LaunchPolicy::default()
            },
        });
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "resume_session");
        assert_eq!(json["data"]["id"], "styra-1");
        assert_eq!(json["data"]["launch"]["templates"][0], "rust");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }

    /// A launch that names no policy of its own is the ordinary case — it runs
    /// under whatever the Workspace stands for — so the field is optional and
    /// its absence must read as an empty overlay rather than an error.
    #[test]
    fn a_launch_request_may_name_no_policy_of_its_own() {
        let request: Request = serde_json::from_str(
            r#"{"operation":"plan_session","data":{"workspace_id":"w-1",
                 "selection":{"provider":"codex","model":"gpt-5.6-sol","effort":"high"}}}"#,
        )
        .unwrap();
        let Request::PlanSession(plan) = request else {
            panic!("expected a plan request");
        };
        assert!(plan.launch.is_empty());
        assert_eq!(plan.launch.network, None);
    }

    #[test]
    fn a_workspace_carries_the_policy_its_launches_start_from() {
        let launch = LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![LaunchMount {
                source: PathBuf::from("/srv/corpus"),
                destination: Some(PathBuf::from("/mnt/corpus")),
                writable: false,
            }],
            standalone: false,
        };
        let request = Request::SetWorkspaceLaunch {
            workspace_id: "w-1".into(),
            launch: launch.clone(),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "set_workspace_launch");
        assert_eq!(json["data"]["launch"]["templates"][0], "rust");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

        let summary = WorkspaceSummary {
            id: "w-1".into(),
            name: None,
            notes: String::new(),
            host_path: PathBuf::from("/home/op/project"),
            path: PathBuf::from("/store/workspaces/w-1"),
            session_count: 0,
            age: "just now".into(),
            created_at_ms: 1,
            last_accessed_at_ms: 1,
            launch,
        };
        let response = Response::WorkspaceLaunchUpdated(summary);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["type"], "workspace_launch_updated");
        assert_eq!(
            json["data"]["launch"]["mounts"][0]["destination"],
            "/mnt/corpus"
        );
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
    }

    #[test]
    fn templates_are_listed_per_workspace_with_their_descriptions() {
        let request = Request::ListTemplates {
            workspace_id: "w-1".into(),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "list_templates");
        assert_eq!(json["data"]["workspace_id"], "w-1");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

        let response = Response::Templates(vec![TemplateSummary {
            name: "rust".into(),
            description: "Rust toolchain".into(),
        }]);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["type"], "templates");
        assert_eq!(json["data"][0]["name"], "rust");
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
    }

    #[test]
    fn convert_session_provider_names_only_the_source_session() {
        let request = Request::ConvertSessionProvider {
            id: "styra-1".into(),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "convert_session_provider");
        assert_eq!(json["data"]["id"], "styra-1");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }

    #[test]
    fn branch_session_names_an_optional_cutoff_and_provider() {
        let request = Request::BranchSession {
            id: "styra-1".into(),
            at_ms: Some(42),
            provider: Some(crate::agent::Provider::Claude),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["operation"], "branch_session");
        assert_eq!(json["data"]["at_ms"], 42);
        assert_eq!(json["data"]["provider"], "claude");
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

        // A branch that keeps the whole history under the same provider says
        // nothing beyond the source id.
        let bare: Request = serde_json::from_str(
            r#"{"operation":"branch_session","data":{"id":"styra-1"}}"#,
        )
        .unwrap();
        assert_eq!(
            bare,
            Request::BranchSession {
                id: "styra-1".into(),
                at_ms: None,
                provider: None,
            }
        );
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
