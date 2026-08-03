//! Blocking client for Styra's JSON protocol over a Unix domain socket.

use crate::protocol::{
    CreateSession, CreateWorkspace, Health, RenameSession, Request, Response, ResumeSession,
    SendMessage,
    SessionInfo, ShellInfo, StoredSession, Updates, WireResponse,
};
use crate::protocol::{InteractionSummary, SessionSummary, WorkspaceSummary};
use anyhow::{bail, Context, Result};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub fn health(&self) -> Result<Health> {
        match self.request(Request::Health)? {
            Response::Health(value) => Ok(value),
            other => unexpected("health", other),
        }
    }

    pub fn create_session(&self, request: &CreateSession) -> Result<SessionInfo> {
        match self.request(Request::CreateSession(request.clone()))? {
            Response::SessionCreated(value) => Ok(value),
            other => unexpected("session_created", other),
        }
    }

    pub fn resume_session(&self, request: &ResumeSession) -> Result<SessionInfo> {
        match self.request(Request::ResumeSession(request.clone()))? {
            Response::SessionResumed(value) => Ok(value),
            other => unexpected("session_resumed", other),
        }
    }

    pub fn rename_session(&self, id: &str, name: Option<&str>) -> Result<SessionSummary> {
        match self.request(Request::RenameSession(RenameSession {
            id: id.to_owned(),
            name: name.map(str::to_owned),
        }))? {
            Response::SessionRenamed(value) => Ok(value),
            other => unexpected("session_renamed", other),
        }
    }

    pub fn create_workspace(&self, request: &CreateWorkspace) -> Result<WorkspaceSummary> {
        match self.request(Request::CreateWorkspace(request.clone()))? {
            Response::WorkspaceCreated(value) => Ok(value),
            other => unexpected("workspace_created", other),
        }
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceSummary>> {
        match self.request(Request::ListWorkspaces)? {
            Response::Workspaces(value) => Ok(value),
            other => unexpected("workspaces", other),
        }
    }

    pub fn workspace(&self, id: &str) -> Result<WorkspaceSummary> {
        match self.request(Request::Workspace { id: id.to_owned() })? {
            Response::Workspace(value) => Ok(value),
            other => unexpected("workspace", other),
        }
    }

    pub fn send_message(&self, id: &str, text: &str) -> Result<()> {
        match self.request(Request::SendMessage {
            id: id.to_owned(),
            message: SendMessage {
                text: text.to_owned(),
            },
        })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    /// Durably queue an operator message on the server without sending it,
    /// so it survives the client disconnecting before the interaction is idle
    /// enough to accept it. Returns the new queue length.
    pub fn queue_message(&self, id: &str, text: &str) -> Result<usize> {
        match self.request(Request::QueueMessage {
            id: id.to_owned(),
            message: SendMessage {
                text: text.to_owned(),
            },
        })? {
            Response::Queued(count) => Ok(count),
            other => unexpected("queued", other),
        }
    }

    /// Pop the oldest durably queued message, if any.
    pub fn take_queued_message(&self, id: &str) -> Result<Option<String>> {
        match self.request(Request::TakeQueuedMessage { id: id.to_owned() })? {
            Response::TakenQueuedMessage(message) => Ok(message),
            other => unexpected("taken_queued_message", other),
        }
    }

    /// Read back the session's durably queued, not-yet-sent messages.
    pub fn queued_messages(&self, id: &str) -> Result<Vec<String>> {
        match self.request(Request::QueuedMessages { id: id.to_owned() })? {
            Response::QueuedMessages(messages) => Ok(messages),
            other => unexpected("queued_messages", other),
        }
    }

    /// Discard the session's durably queued messages. Returns how many were
    /// cleared.
    pub fn clear_queued_messages(&self, id: &str) -> Result<usize> {
        match self.request(Request::ClearQueuedMessages { id: id.to_owned() })? {
            Response::Queued(count) => Ok(count),
            other => unexpected("queued", other),
        }
    }

    pub fn stop_interaction(&self, id: &str) -> Result<()> {
        match self.request(Request::StopInteraction { id: id.to_owned() })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    pub fn interrupt_interaction(&self, id: &str) -> Result<()> {
        match self.request(Request::InterruptInteraction { id: id.to_owned() })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    pub fn updates(&self, id: &str, after: u64) -> Result<Updates> {
        match self.request(Request::Updates {
            id: id.to_owned(),
            after,
        })? {
            Response::Updates(value) => Ok(value),
            other => unexpected("updates", other),
        }
    }

    pub fn list_interactions(&self) -> Result<Vec<InteractionSummary>> {
        match self.request(Request::ListInteractions)? {
            Response::Interactions(value) => Ok(value),
            other => unexpected("interactions", other),
        }
    }

    pub fn list_sessions(&self, workspace_id: &str) -> Result<Vec<SessionSummary>> {
        match self.request(Request::ListSessions {
            workspace_id: workspace_id.to_owned(),
        })? {
            Response::StoredSessions(value) => Ok(value),
            other => unexpected("stored_sessions", other),
        }
    }

    pub fn stored_session(&self, id: &str) -> Result<StoredSession> {
        match self.request(Request::StoredSession { id: id.to_owned() })? {
            Response::StoredSession(value) => Ok(value),
            other => unexpected("stored_session", other),
        }
    }

    pub fn shell(&self, id: &str) -> Result<ShellInfo> {
        match self.request(Request::Shell { id: id.to_owned() })? {
            Response::Shell(value) => Ok(value),
            other => unexpected("shell", other),
        }
    }

    /// Ask the server to shut down. It acknowledges before exiting, so a
    /// successful return means the daemon received the request and is on its
    /// way out (any live interactions it owns go with it).
    pub fn shutdown(&self) -> Result<()> {
        match self.request(Request::Shutdown)? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    fn request(&self, request: Request) -> Result<Response> {
        let mut stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("connecting to Styra socket {}", self.socket.display()))?;
        crate::protocol::write_message(&mut stream, &request)
            .context("writing the Styra request")?;

        let response = crate::protocol::read_message(&mut BufReader::new(stream))
            .context("reading the Styra response")?;
        match response {
            WireResponse::Ok { response } => Ok(response),
            WireResponse::Error { error } => bail!("Styra server: {error}"),
        }
    }
}

fn unexpected<T>(expected: &str, actual: Response) -> Result<T> {
    bail!("Styra protocol error: expected {expected} response, got {actual:?}")
}
