//! Blocking client for Styra's JSON protocol over a Unix domain socket.

use crate::protocol::{
    Answer, Contract, CreateSession, CreateWorkspace, DrivaOptions, Health, LaunchPolicy,
    LoadedInteraction, PlanSession, QueuedMessage, RenameSession, Request, Response, ResumeSession,
    SendMessage, SessionInfo, ShellInfo, StoredSession, TemplateSummary, Updates, WireResponse,
    WorkspaceLaunchChange,
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

    /// The Driva policy a `create_session` with these inputs would launch
    /// under. Creates nothing, so a client can show it before the operator's
    /// first message has started anything.
    pub fn plan_session(&self, request: &PlanSession) -> Result<DrivaOptions> {
        match self.request(Request::PlanSession(request.clone()))? {
            Response::SessionPlan(value) => Ok(value),
            other => unexpected("session_plan", other),
        }
    }

    /// The Driva templates a launch in this Workspace could name, so a client
    /// can offer the real set instead of asking for a remembered name.
    pub fn list_templates(&self, workspace_id: &str) -> Result<Vec<TemplateSummary>> {
        match self.request(Request::ListTemplates {
            workspace_id: workspace_id.to_owned(),
        })? {
            Response::Templates(value) => Ok(value),
            other => unexpected("templates", other),
        }
    }

    pub fn resume_session(&self, request: &ResumeSession) -> Result<SessionInfo> {
        match self.request(Request::ResumeSession(request.clone()))? {
            Response::SessionResumed(value) => Ok(value),
            other => unexpected("session_resumed", other),
        }
    }

    /// Convert a stored Session's native transcript to the other interactive
    /// provider's format and return the new sibling Session it was written
    /// to. The source Session is untouched.
    pub fn convert_session_provider(&self, id: &str) -> Result<SessionSummary> {
        match self.request(Request::ConvertSessionProvider { id: id.to_owned() })? {
            Response::SessionConverted(value) => Ok(value),
            other => unexpected("session_converted", other),
        }
    }

    /// Branch a stored Session into a new sibling Session, seeded with its
    /// history up to `at_ms` (the whole history when `None`), optionally
    /// under a different provider (the same one when `None`). The source
    /// Session is untouched.
    pub fn branch_session(
        &self,
        id: &str,
        at_ms: Option<u64>,
        provider: Option<crate::agent::Provider>,
    ) -> Result<SessionSummary> {
        match self.request(Request::BranchSession {
            id: id.to_owned(),
            at_ms,
            provider,
        })? {
            Response::SessionBranched(value) => Ok(value),
            other => unexpected("session_branched", other),
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

    /// Apply an edit to the server's latest Workspace launch policy and return
    /// the authoritative policy after the edit.
    pub fn change_workspace_launch(
        &self,
        workspace_id: &str,
        change: WorkspaceLaunchChange,
    ) -> Result<LaunchPolicy> {
        match self.request(Request::ChangeWorkspaceLaunch {
            workspace_id: workspace_id.to_owned(),
            change,
        })? {
            Response::WorkspaceLaunchUpdated(value) => Ok(value),
            other => unexpected("workspace_launch_updated", other),
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

    pub fn set_workspace_git_repository(
        &self,
        workspace_id: &str,
        git_repository: Option<&Path>,
    ) -> Result<WorkspaceSummary> {
        match self.request(Request::SetWorkspaceGitRepository {
            workspace_id: workspace_id.to_owned(),
            git_repository: git_repository.map(Path::to_path_buf),
        })? {
            Response::WorkspaceGitRepositoryUpdated(value) => Ok(value),
            other => unexpected("workspace_git_repository_updated", other),
        }
    }

    pub fn set_workspace_worktrees_enabled(
        &self,
        workspace_id: &str,
        enabled: bool,
    ) -> Result<WorkspaceSummary> {
        match self.request(Request::SetWorkspaceWorktreesEnabled {
            workspace_id: workspace_id.to_owned(),
            enabled,
        })? {
            Response::WorkspaceWorktreesUpdated(value) => Ok(value),
            other => unexpected("workspace_worktrees_updated", other),
        }
    }

    /// Fetch the server-owned launch policy without recording a Workspace
    /// access. The UI uses this to observe edits made by other clients.
    pub fn workspace_launch(&self, workspace_id: &str) -> Result<LaunchPolicy> {
        match self.request(Request::WorkspaceLaunch {
            workspace_id: workspace_id.to_owned(),
        })? {
            Response::WorkspaceLaunch(value) => Ok(value),
            other => unexpected("workspace_launch", other),
        }
    }

    /// Send one turn, however it is qualified.
    ///
    /// [`SendMessage`] is the unit a turn is described in — its text, the
    /// selection to run it under, the shape its reply must take — so a caller
    /// that wants two of those at once composes them here rather than looking
    /// for a method per combination.
    pub fn send_turn(&self, id: &str, message: SendMessage) -> Result<()> {
        match self.request(Request::SendMessage {
            id: id.to_owned(),
            message,
        })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    pub fn send_message(&self, id: &str, text: &str) -> Result<()> {
        self.send_turn(id, SendMessage::new(text))
    }

    pub fn send_message_with_selection(
        &self,
        id: &str,
        text: &str,
        selection: &crate::agent::Selection,
    ) -> Result<()> {
        self.send_turn(id, SendMessage::new(text).under(selection.clone()))
    }

    pub fn set_interaction_working_directory(&self, id: &str, directory: PathBuf) -> Result<()> {
        match self.request(Request::SetInteractionWorkingDirectory {
            id: id.to_owned(),
            directory,
        })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    /// Switch a live interaction onto another model straight away, rather than
    /// leaving it for the next message to carry. The server records it with the
    /// session, so reopening the session keeps the switch.
    pub fn set_session_selection(
        &self,
        id: &str,
        selection: &crate::agent::Selection,
    ) -> Result<()> {
        match self.request(Request::SetSessionSelection {
            id: id.to_owned(),
            selection: selection.clone(),
        })? {
            Response::Accepted => Ok(()),
            other => unexpected("accepted", other),
        }
    }

    /// Send a message asking its reply to come back in a named shape.
    ///
    /// The server frames the text with the contract's instructions and records
    /// the contract with the session; the answer is read separately with
    /// [`Self::turn_answer`] once the turn has completed, which the caller sees
    /// on the update stream as it would for any turn.
    pub fn send_typed_message(&self, id: &str, text: &str, contract: Contract) -> Result<()> {
        self.send_turn(id, SendMessage::new(text).asking_for(contract))
    }

    /// Parse the session's most recent agent message as a typed answer, under
    /// the contract its last typed turn was sent with.
    pub fn turn_answer(&self, id: &str) -> Result<Answer> {
        self.answer(id, None)
    }

    /// Parse that same message under a contract of the caller's choosing,
    /// whatever the session was sent with — how an answer is re-read as another
    /// shape, and how an untyped turn is typed after the fact.
    pub fn turn_answer_as(&self, id: &str, contract: Contract) -> Result<Answer> {
        self.answer(id, Some(contract))
    }

    fn answer(&self, id: &str, contract: Option<Contract>) -> Result<Answer> {
        match self.request(Request::TurnAnswer {
            id: id.to_owned(),
            contract,
        })? {
            Response::Answer(answer) => Ok(answer),
            other => unexpected("answer", other),
        }
    }

    /// Durably queue an operator message on the server without sending it,
    /// so it survives the client disconnecting before the interaction is idle
    /// enough to accept it. Returns the authoritative queue.
    pub fn queue_message(&self, id: &str, text: &str) -> Result<Vec<QueuedMessage>> {
        self.queue_turn(id, SendMessage::new(text))
    }

    /// Queue a turn as composed, so a contract chosen while the agent was busy
    /// survives the wait rather than being dropped at the moment the operator
    /// could least do anything about it.
    pub fn queue_turn(&self, id: &str, message: SendMessage) -> Result<Vec<QueuedMessage>> {
        match self.request(Request::QueueMessage {
            id: id.to_owned(),
            message,
        })? {
            Response::QueuedMessages(messages) => Ok(messages),
            other => unexpected("queued_messages", other),
        }
    }

    /// Ask the server to send the oldest queued message. The returned queue is
    /// the complete authoritative remainder for presentation.
    pub fn send_queued_message(
        &self,
        id: &str,
    ) -> Result<(Option<QueuedMessage>, Vec<QueuedMessage>)> {
        match self.request(Request::SendQueuedMessage { id: id.to_owned() })? {
            Response::SentQueuedMessage(message, queued) => Ok((message, queued)),
            other => unexpected("sent_queued_message", other),
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

    /// Stop an interaction and remove it from the server's list, leaving the
    /// Session as stored history like any other one on disk.
    pub fn close_interaction(&self, id: &str) -> Result<()> {
        match self.request(Request::CloseInteraction { id: id.to_owned() })? {
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
        self.updates_filtered(id, after, true)
    }

    pub fn load_interaction(&self, id: &str) -> Result<LoadedInteraction> {
        match self.request(Request::LoadInteraction { id: id.to_owned() })? {
            Response::InteractionLoaded(interaction) => Ok(interaction),
            other => unexpected("interaction_loaded", other),
        }
    }

    /// Updates without the verbatim wire lines, for a client that renders no
    /// raw view. Replaying a long interaction from zero is dominated by those
    /// lines, so not asking for them is the difference the preview pane feels.
    pub fn updates_without_raw(&self, id: &str, after: u64) -> Result<Updates> {
        self.updates_filtered(id, after, false)
    }

    fn updates_filtered(&self, id: &str, after: u64, raw: bool) -> Result<Updates> {
        match self.request(Request::Updates {
            id: id.to_owned(),
            after,
            raw,
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

    /// Read the server's plan-quota readings, oldest first. Empty until a
    /// provider has volunteered one — it is a live in-memory log, so it starts
    /// empty with the daemon rather than being loaded from the store.
    pub fn quota_log(&self) -> Result<Vec<crate::protocol::QuotaEvent>> {
        match self.request(Request::QuotaLog)? {
            Response::QuotaLog(value) => Ok(value),
            other => unexpected("quota_log", other),
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
        self.stored_session_filtered(id, true)
    }

    /// A stored session's decoded events only. The server then reads the
    /// journal once instead of twice and ships half the payload.
    pub fn stored_session_events(&self, id: &str) -> Result<StoredSession> {
        self.stored_session_filtered(id, false)
    }

    fn stored_session_filtered(&self, id: &str, raw: bool) -> Result<StoredSession> {
        match self.request(Request::StoredSession {
            id: id.to_owned(),
            raw,
        })? {
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
