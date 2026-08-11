//! Styra's Unix-socket server and server-owned interaction manager.

use crate::agent::{MountSpec, SandboxLayout, Selection};
use crate::interaction::{Interaction, InteractionSpec, ResolvedTemplate, SandboxBroker};
use crate::journal::{self, Journal};
use crate::protocol::{
    CreateSession, CreateWorkspace, Health, Request, Response, ResumeSession, SequencedUpdate,
    SessionInfo, ShellInfo, StoredSession, Updates, WireResponse, MAX_REQUEST_BYTES,
};
use crate::protocol::{
    DrivaOptions, InteractionActivity, InteractionSummary, InteractionUpdate, SessionSummary,
};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct ServerState {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    store_root: PathBuf,
    /// The socket the server is bound to, removed on an explicit shutdown so
    /// the next client sees no stale socket to trip over.
    socket: PathBuf,
    layout: SandboxLayout,
    interactions: Mutex<HashMap<String, Arc<ManagedInteraction>>>,
    /// Set by a [`Request::Shutdown`]; the connection thread checks it after
    /// acknowledging and then exits the process.
    shutdown: AtomicBool,
}

struct ManagedInteraction {
    interaction: Interaction,
    updates: Arc<Mutex<Vec<SequencedUpdate>>>,
    accepting_messages: Arc<AtomicBool>,
    activity: Arc<Mutex<InteractionActivity>>,
    /// Captured at spawn so the interaction can be listed and reattached to without
    /// re-deriving them: the agent selection, host workspace, and launch policy.
    workspace_id: String,
    name: Mutex<Option<String>>,
    /// What the interaction is running under *now*: the operator can switch
    /// model mid-session, and every such switch is mirrored to `session.json`
    /// so reattaching or resuming picks up the switch rather than the launch.
    selection: Mutex<Selection>,
    workspace: PathBuf,
    driva: DrivaOptions,
    shell: ShellInfo,
    /// Operator messages not yet sent to the agent, durably mirrored into
    /// `session_path` on every mutation so the queue survives the operator
    /// closing the Styra UI (or the daemon restarting) before it drains.
    queue: Mutex<std::collections::VecDeque<String>>,
    /// The session's durable directory: its journal, metadata and queue.
    session_path: PathBuf,
}

fn update_finishes_background(update: &InteractionUpdate) -> bool {
    matches!(update, InteractionUpdate::Event(event) if event.finishes_background_task())
}

impl ManagedInteraction {
    fn summary(&self) -> InteractionSummary {
        InteractionSummary {
            id: self.interaction.session_id().to_owned(),
            name: self
                .name
                .lock()
                .expect("session name lock poisoned")
                .clone(),
            workspace_id: self.workspace_id.clone(),
            selection: self.selection(),
            workspace: self.workspace.clone(),
            driva: self.driva.clone(),
            accepting: self.accepting_messages.load(Ordering::Acquire),
            activity: *self
                .activity
                .lock()
                .expect("interaction activity lock poisoned"),
        }
    }
}

impl ManagedInteraction {
    fn selection(&self) -> Selection {
        self.selection
            .lock()
            .expect("interaction selection lock poisoned")
            .clone()
    }

    /// Move the interaction onto `selection`: apply it to the running agent,
    /// then record it, so both this attachment and the next one agree on what
    /// the session is running.
    ///
    /// Only the model can change. The provider defines the process itself, and
    /// Claude Code fixes reasoning effort for the life of the session, so those
    /// are rejected rather than silently dropped.
    fn set_selection(&self, selection: Selection) -> Result<()> {
        let mut current = self
            .selection
            .lock()
            .expect("interaction selection lock poisoned");
        if selection == *current {
            return Ok(());
        }
        if selection.provider != current.provider {
            anyhow::bail!("changing agent provider requires a new session");
        }
        if selection.provider == crate::agent::Provider::Claude
            && selection.effort != current.effort
        {
            anyhow::bail!("Claude Code fixes reasoning effort for the life of a session");
        }
        self.interaction.set_selection(&selection)?;
        journal::store_session_selection(&self.session_path, &selection)?;
        *current = selection;
        Ok(())
    }

    fn send(&self, text: &str) -> Result<()> {
        if !self.accepting_messages.load(Ordering::Acquire) {
            anyhow::bail!(
                "session {} is not accepting messages",
                self.interaction.session_id()
            );
        }
        self.interaction.send(text)?;
        Ok(())
    }

    fn stop(&self) {
        self.accepting_messages.store(false, Ordering::Release);
        self.interaction.stop();
    }

    fn persist_queue(&self, queue: &std::collections::VecDeque<String>) -> Result<()> {
        let messages: Vec<String> = queue.iter().cloned().collect();
        journal::write_queued_messages(&self.session_path, &messages)
    }

    fn queue_message(&self, text: &str) -> Result<usize> {
        let mut queue = self.queue.lock().expect("interaction queue lock poisoned");
        queue.push_back(text.to_owned());
        self.persist_queue(&queue)?;
        Ok(queue.len())
    }

    fn take_queued_message(&self) -> Result<Option<String>> {
        let mut queue = self.queue.lock().expect("interaction queue lock poisoned");
        let next = queue.pop_front();
        if next.is_some() {
            self.persist_queue(&queue)?;
        }
        Ok(next)
    }

    fn clear_queue(&self) -> Result<usize> {
        let mut queue = self.queue.lock().expect("interaction queue lock poisoned");
        let count = queue.len();
        queue.clear();
        self.persist_queue(&queue)?;
        Ok(count)
    }

    fn queued_messages(&self) -> Vec<String> {
        self.queue
            .lock()
            .expect("interaction queue lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

impl ServerState {
    pub fn new(store_root: PathBuf, socket: PathBuf) -> Self {
        Self {
            inner: Arc::new(ServerInner {
                store_root,
                socket,
                layout: SandboxLayout::default(),
                interactions: Mutex::new(HashMap::new()),
                shutdown: AtomicBool::new(false),
            }),
        }
    }

    pub fn store_root(&self) -> &Path {
        &self.inner.store_root
    }

    /// If a client asked the server to shut down, remove the socket and exit.
    /// Called by a connection thread only after its acknowledgement has been
    /// flushed, so the requester learns the daemon is going down.
    fn shutdown_if_requested(&self) {
        if self.inner.shutdown.load(Ordering::Acquire) {
            std::fs::remove_file(&self.inner.socket).ok();
            std::process::exit(0);
        }
    }

    fn create_session(&self, request: CreateSession) -> Result<SessionInfo> {
        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &request.workspace_id)?;
        let workspace = owning_workspace.host_path;
        let selection = request.selection;
        let name = journal::normalize_session_name(request.name.as_deref())?
            .or_else(|| journal::name_from_message(request.message.as_deref()));
        let mut profile = crate::agent::resolve_profile(&selection, &self.inner.layout)?;
        profile.network = profile.network || request.network;
        let template = resolve_templates(&workspace, &request.templates)?;
        // Resolve host tooling before creating durable session state, so a
        // missing tmux or broker executable cannot leave an empty journal.
        let tmux = genta::agent::resolve_executable(Path::new("tmux"))
            .context("tmux is required for Styra session shells")?;
        let (journal, id) = Journal::create_in_workspace(
            &self.inner.store_root,
            &request.workspace_id,
            &profile,
            &selection,
            name.clone(),
        )?;
        let journal_path = journal.path().to_path_buf();
        let diagnostics = journal_path
            .parent()
            .unwrap_or(&self.inner.store_root)
            .join("diagnostics.log");
        let spec = InteractionSpec {
            profile,
            resume_provider_session_id: None,
            working_directory: self.inner.layout.workspace.clone(),
            workspace: MountSpec {
                source: workspace.clone(),
                destination: self.inner.layout.workspace.clone(),
                writable: true,
            },
            temporary_mounts: Vec::new(),
            template,
            broker: Some(self.prepare_broker(&id, tmux)?),
        };
        let driva = DrivaOptions::capture(&spec, "bwrap");
        let prepared_broker = spec.broker.as_ref().expect("broker was prepared");
        let shell = ShellInfo {
            tmux: prepared_broker.tmux.clone(),
            socket: prepared_broker.control.source.join("tmux.sock"),
        };
        let backend = Box::new(driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: Some(PathBuf::from("/")),
        });
        let (interaction, receiver) =
            match Interaction::spawn(spec, backend, journal, id.clone(), diagnostics) {
                Ok(spawned) => spawned,
                Err(error) => {
                    if let Some(control) = shell.socket.parent() {
                        std::fs::remove_dir_all(control).ok();
                    }
                    return Err(error);
                }
            };
        let updates = Arc::new(Mutex::new(Vec::new()));
        let accepting_messages = Arc::new(AtomicBool::new(true));
        let activity = Arc::new(Mutex::new(InteractionActivity::Pending));
        let managed = Arc::new(ManagedInteraction {
            interaction,
            updates: Arc::clone(&updates),
            accepting_messages: Arc::clone(&accepting_messages),
            activity: Arc::clone(&activity),
            workspace_id: request.workspace_id.clone(),
            name: Mutex::new(name.clone()),
            selection: Mutex::new(selection.clone()),
            workspace: workspace.clone(),
            driva: driva.clone(),
            shell,
            queue: Mutex::new(std::collections::VecDeque::new()),
            session_path: journal_path
                .parent()
                .unwrap_or(&self.inner.store_root)
                .to_path_buf(),
        });
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                let mut background_work = false;
                let mut background_polls = HashSet::new();
                while let Ok(update) = receiver.recv() {
                    match &update {
                        InteractionUpdate::Event(event) if event.starts_background_task() => {
                            background_work = true;
                            *activity.lock().expect("interaction activity lock poisoned") =
                                InteractionActivity::Running;
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolStarted {
                            id,
                            name,
                            ..
                        }) if matches!(
                            name.as_str(),
                            "TaskOutput" | "TaskGet" | "task_output" | "task_get"
                        ) =>
                        {
                            background_polls.insert(id.clone());
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::UserMessage {
                            ..
                        }) => {
                            *activity.lock().expect("interaction activity lock poisoned") =
                                InteractionActivity::Running;
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::TurnCompleted {
                            ..
                        }) => {
                            *activity.lock().expect("interaction activity lock poisoned") =
                                if background_work {
                                    InteractionActivity::Background
                                } else {
                                    InteractionActivity::Pending
                                };
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolCompleted {
                            id,
                            ..
                        }) if background_polls.remove(id) => {
                            if update_finishes_background(&update) {
                                background_work = false;
                                *activity.lock().expect("interaction activity lock poisoned") =
                                    InteractionActivity::Pending;
                            }
                        }
                        _ => {}
                    }
                    if matches!(update, InteractionUpdate::Ended(_)) {
                        accepting_messages.store(false, Ordering::Release);
                    }
                    let mut history = updates.lock().expect("interaction update lock poisoned");
                    let sequence = history.len() as u64 + 1;
                    history.push(SequencedUpdate { sequence, update });
                }
            })
            .context("starting the interaction update collector")?;
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .insert(id.clone(), Arc::clone(&managed));

        if let Some(message) = request
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            if let Err(error) = managed.send(message) {
                managed.stop();
                self.inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned")
                    .remove(&id);
                return Err(error);
            }
        }

        Ok(SessionInfo {
            id,
            name,
            workspace_id: request.workspace_id,
            selection,
            workspace,
            journal_path,
            driva,
            updates_after: 0,
            queued: Vec::new(),
        })
    }

    fn resume_session(&self, request: ResumeSession) -> Result<SessionInfo> {
        if self
            .inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .get(&request.id)
            .is_some_and(|managed| managed.accepting_messages.load(Ordering::Acquire))
        {
            anyhow::bail!("session {:?} already has a live interaction", request.id);
        }

        let summary = self.stored_summary(&request.id)?;
        let provider_session_id = journal::read_provider_session_id(&summary.path)?
            .with_context(|| {
                format!(
                    "session {:?} has no stored provider session id; it can be viewed but not resumed",
                    request.id
                )
            })?;
        ensure_native_session_exists(summary.selection.provider, &provider_session_id)?;
        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &summary.workspace_id)?;
        let workspace = owning_workspace.host_path;
        let selection = summary.selection;
        let mut profile = crate::agent::resolve_profile(&selection, &self.inner.layout)?;
        profile.resume(selection.provider, &provider_session_id)?;
        profile.network = profile.network || request.network;
        let template = resolve_templates(&workspace, &request.templates)?;

        let tmux = genta::agent::resolve_executable(Path::new("tmux"))
            .context("tmux is required for Styra session shells")?;
        // Seed the live update stream before the provider starts. Clients
        // attaching from cursor zero then receive the stored conversation and
        // all subsequent native-resume traffic as one sequence. The client
        // initiating this resume already displays the journal, so it starts
        // after this explicit boundary.
        let seeded_updates = replayed_session_updates(&summary.path, profile.protocol)?;
        let updates_after = seeded_updates.len() as u64;
        let journal = Journal::open(&summary.path)?;
        let journal_path = journal.path().to_path_buf();
        let diagnostics = summary.path.join("diagnostics.log");
        let spec = InteractionSpec {
            profile,
            resume_provider_session_id: Some(provider_session_id),
            working_directory: self.inner.layout.workspace.clone(),
            workspace: MountSpec {
                source: workspace.clone(),
                destination: self.inner.layout.workspace.clone(),
                writable: true,
            },
            temporary_mounts: Vec::new(),
            template,
            broker: Some(self.prepare_broker(&request.id, tmux)?),
        };
        let driva = DrivaOptions::capture(&spec, "bwrap");
        let prepared_broker = spec.broker.as_ref().expect("broker was prepared");
        let shell = ShellInfo {
            tmux: prepared_broker.tmux.clone(),
            socket: prepared_broker.control.source.join("tmux.sock"),
        };
        let backend = Box::new(driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: Some(PathBuf::from("/")),
        });
        let (interaction, receiver) =
            Interaction::spawn(spec, backend, journal, request.id.clone(), diagnostics)?;
        let updates = Arc::new(Mutex::new(seeded_updates));
        let accepting_messages = Arc::new(AtomicBool::new(true));
        let activity = Arc::new(Mutex::new(InteractionActivity::Pending));
        // A resumed Session may carry over messages that were durably queued
        // on a previous attachment (one stopped before the interaction went
        // idle enough to send them), so reload them rather than starting empty.
        let queued = journal::read_queued_messages(&summary.path)?;
        let managed = Arc::new(ManagedInteraction {
            interaction,
            updates: Arc::clone(&updates),
            accepting_messages: Arc::clone(&accepting_messages),
            activity: Arc::clone(&activity),
            workspace_id: summary.workspace_id.clone(),
            name: Mutex::new(summary.name.clone()),
            selection: Mutex::new(selection.clone()),
            workspace: workspace.clone(),
            driva: driva.clone(),
            shell,
            queue: Mutex::new(queued.into_iter().collect()),
            session_path: summary.path.clone(),
        });
        let id = request.id.clone();
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                let mut background_work = false;
                let mut background_polls = HashSet::new();
                while let Ok(update) = receiver.recv() {
                    match &update {
                        InteractionUpdate::Event(event) if event.starts_background_task() => {
                            background_work = true;
                            *activity.lock().expect("interaction activity lock poisoned") =
                                InteractionActivity::Running;
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolStarted {
                            id,
                            name,
                            ..
                        }) if matches!(
                            name.as_str(),
                            "TaskOutput" | "TaskGet" | "task_output" | "task_get"
                        ) =>
                        {
                            background_polls.insert(id.clone());
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::UserMessage {
                            ..
                        }) => {
                            *activity.lock().expect("interaction activity lock poisoned") =
                                InteractionActivity::Running;
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::TurnCompleted {
                            ..
                        }) => {
                            *activity.lock().expect("interaction activity lock poisoned") =
                                if background_work {
                                    InteractionActivity::Background
                                } else {
                                    InteractionActivity::Pending
                                };
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolCompleted {
                            id,
                            ..
                        }) if background_polls.remove(id) => {
                            if update_finishes_background(&update) {
                                background_work = false;
                                *activity.lock().expect("interaction activity lock poisoned") =
                                    InteractionActivity::Pending;
                            }
                        }
                        _ => {}
                    }
                    if matches!(update, InteractionUpdate::Ended(_)) {
                        accepting_messages.store(false, Ordering::Release);
                    }
                    let mut history = updates.lock().expect("interaction update lock poisoned");
                    let sequence = history.len() as u64 + 1;
                    history.push(SequencedUpdate { sequence, update });
                }
            })
            .context("starting the resumed interaction update collector")?;
        let queued = managed.queued_messages();
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .insert(request.id.clone(), managed);

        Ok(SessionInfo {
            id: request.id,
            name: summary.name,
            workspace_id: summary.workspace_id,
            selection,
            workspace,
            journal_path,
            driva,
            updates_after,
            queued,
        })
    }

    fn interaction(&self, id: &str) -> Result<Arc<ManagedInteraction>> {
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .get(id)
            .cloned()
            .with_context(|| format!("no live interaction for session {id:?}"))
    }

    fn prepare_broker(&self, id: &str, tmux: PathBuf) -> Result<SandboxBroker> {
        let control_root = self
            .inner
            .socket
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sandboxes");
        let control = control_root.join(id);
        std::fs::create_dir_all(&control)
            .with_context(|| format!("creating sandbox control directory {}", control.display()))?;
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o700)).with_context(
            || {
                format!(
                    "restricting sandbox control directory {}",
                    control.display()
                )
            },
        )?;
        // The server is deliberately long-lived, while its installed binary
        // may be replaced by a rebuild or upgrade. Do not hand Driva that
        // potentially stale pathname: keep an executable copy owned by this
        // interaction. On Linux `/proc/self/exe` remains readable even after
        // the original directory entry has been unlinked.
        let executable = control.join("styra-broker");
        let running_executable = std::env::current_exe()
            .ok()
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("/proc/self/exe"));
        std::fs::copy(&running_executable, &executable).with_context(|| {
            format!(
                "staging the Styra sandbox broker from {} to {}",
                running_executable.display(),
                executable.display()
            )
        })?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .with_context(|| {
                format!("making sandbox broker {} executable", executable.display())
            })?;
        Ok(SandboxBroker {
            executable: PathBuf::from("/tmp/styra/control/styra-broker"),
            tmux,
            control: MountSpec {
                source: control,
                destination: PathBuf::from("/tmp/styra/control"),
                writable: true,
            },
            socket: PathBuf::from("/tmp/styra/control/tmux.sock"),
        })
    }

    fn shell(&self, id: &str) -> Result<ShellInfo> {
        let interaction = self.interaction(id)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if interaction.shell.socket.exists() {
                return Ok(interaction.shell.clone());
            }
            if !interaction.accepting_messages.load(Ordering::Acquire) {
                anyhow::bail!("session {id:?} has ended; its sandbox shell is no longer running");
            }
            if Instant::now() >= deadline {
                anyhow::bail!("session {id:?} sandbox shell did not become ready");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stored_summary(&self, id: &str) -> Result<SessionSummary> {
        for workspace in crate::workspace::list(&self.inner.store_root)? {
            if let Some(session) =
                journal::list_workspace_sessions(&self.inner.store_root, &workspace.id)?
                    .into_iter()
                    .find(|session| session.id == id)
            {
                return Ok(session);
            }
        }
        anyhow::bail!("stored session {id:?} was not found")
    }

    fn handle(&self, request: Request) -> Result<Response> {
        match request {
            Request::Health => Ok(Response::Health(Health {
                service: "styra".into(),
            })),
            Request::CreateWorkspace(CreateWorkspace { host_path, name }) => {
                Ok(Response::WorkspaceCreated(crate::workspace::create(
                    &self.inner.store_root,
                    &host_path,
                    name,
                )?))
            }
            Request::ListWorkspaces => Ok(Response::Workspaces(crate::workspace::list(
                &self.inner.store_root,
            )?)),
            Request::Workspace { id } => Ok(Response::Workspace(crate::workspace::access(
                &self.inner.store_root,
                &id,
            )?)),
            Request::CreateSession(request) => {
                Ok(Response::SessionCreated(self.create_session(request)?))
            }
            Request::ResumeSession(request) => {
                Ok(Response::SessionResumed(self.resume_session(request)?))
            }
            Request::RenameSession(request) => {
                let summary = self.stored_summary(&request.id)?;
                let name = journal::store_session_name(&summary.path, request.name.as_deref())?;
                if let Some(interaction) = self
                    .inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned")
                    .get(&request.id)
                {
                    *interaction.name.lock().expect("session name lock poisoned") = name;
                }
                Ok(Response::SessionRenamed(self.stored_summary(&request.id)?))
            }
            Request::UpdateSessionNotes(request) => {
                let summary = self.stored_summary(&request.id)?;
                journal::store_session_notes(&summary.path, request.notes)?;
                Ok(Response::SessionNotesUpdated(
                    self.stored_summary(&request.id)?,
                ))
            }
            Request::UpdateWorkspaceNotes(request) => Ok(Response::WorkspaceNotesUpdated(
                crate::workspace::store_notes(&self.inner.store_root, &request.id, request.notes)?,
            )),
            Request::SendMessage { id, message } => {
                let interaction = self.interaction(&id)?;
                // A client that names a selection on the turn is switching the
                // session onto it, not just this message: adopt it durably and
                // then send under whatever the session now runs.
                if let Some(selection) = message.selection {
                    interaction.set_selection(selection)?;
                }
                interaction
                    .interaction
                    .send_with_selection(&message.text, Some(&interaction.selection()))?;
                Ok(Response::Accepted)
            }
            Request::SetSessionSelection { id, selection } => {
                crate::agent::validate_selection(&selection)?;
                self.interaction(&id)?.set_selection(selection)?;
                Ok(Response::Accepted)
            }
            Request::QueueMessage { id, message } => Ok(Response::Queued(
                self.interaction(&id)?.queue_message(&message.text)?,
            )),
            Request::TakeQueuedMessage { id } => Ok(Response::TakenQueuedMessage(
                self.interaction(&id)?.take_queued_message()?,
            )),
            Request::QueuedMessages { id } => Ok(Response::QueuedMessages(
                self.interaction(&id)?.queued_messages(),
            )),
            Request::ClearQueuedMessages { id } => {
                Ok(Response::Queued(self.interaction(&id)?.clear_queue()?))
            }
            Request::InterruptInteraction { id } => {
                self.interaction(&id)?.interaction.interrupt()?;
                Ok(Response::Accepted)
            }
            Request::StopInteraction { id } => {
                self.interaction(&id)?.stop();
                Ok(Response::Accepted)
            }
            Request::CloseInteraction { id } => {
                let interaction = self.interaction(&id)?;
                interaction.stop();
                // Queued messages would otherwise be waiting for an interaction
                // that no longer exists, exactly as pausing discards them.
                interaction.clear_queue()?;
                self.inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned")
                    .remove(&id);
                Ok(Response::Accepted)
            }
            Request::Updates { id, after } => {
                let interaction = self.interaction(&id)?;
                let all = interaction
                    .updates
                    .lock()
                    .expect("interaction update lock poisoned");
                let updates = all
                    .iter()
                    .filter(|update| update.sequence > after)
                    .cloned()
                    .collect();
                let next = all.last().map(|update| update.sequence).unwrap_or(after);
                Ok(Response::Updates(Updates { updates, next }))
            }
            Request::ListInteractions => {
                let interactions = self
                    .inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned");
                let mut summaries: Vec<InteractionSummary> = interactions
                    .values()
                    .map(|managed| managed.summary())
                    .collect();
                // Newest first: the id embeds a millisecond timestamp, so a
                // descending id sort orders interactions by creation time.
                summaries.sort_by(|a, b| b.id.cmp(&a.id));
                Ok(Response::Interactions(summaries))
            }
            Request::ListSessions { workspace_id } => Ok(Response::StoredSessions(
                journal::list_workspace_sessions(self.store_root(), &workspace_id)?,
            )),
            Request::StoredSession { id } => {
                let summary = self.stored_summary(&id)?;
                let meta = journal::read_session_meta(&summary.path)?;
                let events = journal::replay(&summary.path, meta.protocol)?;
                let raw = journal::replay_raw(&summary.path)?;
                Ok(Response::StoredSession(StoredSession {
                    summary,
                    events,
                    raw,
                }))
            }
            Request::Shell { id } => Ok(Response::Shell(self.shell(&id)?)),
            // Flag the shutdown; the connection thread acts on it once this
            // acknowledgement has gone back over the wire.
            Request::Shutdown => {
                self.inner.shutdown.store(true, Ordering::Release);
                Ok(Response::Accepted)
            }
        }
    }
}

fn replayed_session_updates(
    path: &Path,
    protocol: crate::event::Protocol,
) -> Result<Vec<SequencedUpdate>> {
    let events = journal::replay(path, protocol)?;
    let raw = journal::replay_raw(path)?;
    let mut updates = Vec::with_capacity(events.len() + raw.len());
    for event in events {
        // App-server control traffic is carried by the raw view but omitted
        // from the live event list, matching normal Interaction behavior.
        if !matches!(event, crate::event::AgentEvent::Unknown { .. }) {
            push_sequenced(&mut updates, InteractionUpdate::Event(event));
        }
    }
    for line in raw {
        push_sequenced(&mut updates, InteractionUpdate::Raw(line));
    }
    Ok(updates)
}

fn push_sequenced(updates: &mut Vec<SequencedUpdate>, update: InteractionUpdate) {
    let sequence = updates.len() as u64 + 1;
    updates.push(SequencedUpdate { sequence, update });
}

/// Fail before launching a sandbox when the provider has already discarded
/// the conversation Styra was asked to resume. The journal remains usable for
/// viewing regardless.
fn ensure_native_session_exists(
    provider: crate::agent::Provider,
    provider_session_id: &str,
) -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME is required to locate provider sessions")?;
    let home = PathBuf::from(home);
    let roots: Vec<PathBuf> = match provider {
        crate::agent::Provider::Codex => vec![
            home.join(".codex/sessions"),
            home.join(".codex/archived_sessions"),
        ],
        crate::agent::Provider::Claude => vec![home.join(".claude/projects")],
        crate::agent::Provider::CodexExec => {
            anyhow::bail!("provider codex-exec does not support resuming sessions")
        }
    };
    if roots
        .iter()
        .any(|root| tree_has_session(root, provider_session_id))
    {
        return Ok(());
    }
    anyhow::bail!(
        "{} session {:?} does not exist anymore; the Styra transcript is still available read-only",
        provider.as_str(),
        provider_session_id
    )
}

fn tree_has_session(root: &Path, provider_session_id: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.filter_map(|entry| entry.ok()).any(|entry| {
        let path = entry.path();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            tree_has_session(&path, provider_session_id)
        } else {
            entry
                .file_name()
                .to_string_lossy()
                .contains(provider_session_id)
        }
    })
}

/// Resolve and merge the named Driva templates against a `driva.toml` in the
/// operator's workspace, if any (falling back to Driva's built-ins), in the
/// order given: later names take precedence on conflicting settings, mirroring
/// `driva run --template` layering.
fn resolve_templates(workspace: &Path, names: &[String]) -> Result<Option<ResolvedTemplate>> {
    if names.is_empty() {
        return Ok(None);
    }
    let driva_config = {
        let candidate = workspace.join("driva.toml");
        if candidate.exists() {
            driva::Config::load(&candidate)?
        } else {
            driva::Config::default()
        }
    };
    let mut merged: Option<driva::TemplateConfig> = None;
    for name in names {
        let later = driva_config
            .template(name)
            .with_context(|| format!("unknown driva template {name:?}"))?;
        match &mut merged {
            Some(current) => current.overlay(later),
            None => merged = Some(later),
        }
    }
    ResolvedTemplate::resolve(merged.expect("non-empty names produces a merged template")).map(Some)
}

/// Serve socket connections until the listener fails or the process exits.
pub fn serve(listener: UnixListener, state: ServerState) -> Result<()> {
    for connection in listener.incoming() {
        let stream = connection.context("accepting a Styra client")?;
        let state = state.clone();
        std::thread::Builder::new()
            .name("styra-client".into())
            .spawn(move || {
                if let Err(error) = serve_connection(stream, &state) {
                    eprintln!("styra-server client error: {error:#}");
                }
            })
            .context("starting a Styra client thread")?;
    }
    Ok(())
}

fn serve_connection(mut stream: UnixStream, state: &ServerState) -> Result<()> {
    let wire = match crate::protocol::read_message_limited(
        &mut BufReader::new(&stream),
        MAX_REQUEST_BYTES,
    )
    .and_then(|request| state.handle(request))
    {
        Ok(response) => WireResponse::Ok { response },
        Err(error) => WireResponse::Error {
            error: format!("{error:#}"),
        },
    };
    crate::protocol::write_message(&mut stream, &wire).context("writing the Styra response")?;
    // The ack is on its way to the client; only now is it safe to exit.
    state.shutdown_if_requested();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("styra-server-{tag}-{}.sock", std::process::id(),))
    }

    #[test]
    fn socket_health_reports_the_service() {
        let socket = temp_path("health");
        std::fs::remove_file(&socket).ok();
        let listener = UnixListener::bind(&socket).unwrap();
        let store = socket.with_extension("store");
        let state = ServerState::new(store.clone(), socket.clone());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, &state).unwrap();
        });

        let health = Client::new(&socket).health().unwrap();
        assert_eq!(health.service, "styra");

        server.join().unwrap();
        std::fs::remove_file(socket).ok();
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn stored_ids_are_resolved_from_the_store_listing() {
        let store =
            std::env::temp_dir().join(format!("styra-server-id-test-{}", std::process::id()));
        std::fs::remove_dir_all(&store).ok();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));
        let error = state.stored_summary("../../etc").unwrap_err();
        assert!(error.to_string().contains("was not found"));
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn broker_is_staged_in_the_sandbox_control_mount() {
        let root =
            std::env::temp_dir().join(format!("styra-server-broker-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let socket = root.join("styra.sock");
        let state = ServerState::new(root.join("store"), socket);

        let broker = state
            .prepare_broker("session-1", PathBuf::from("/usr/bin/tmux"))
            .unwrap();

        assert_eq!(
            broker.executable,
            PathBuf::from("/tmp/styra/control/styra-broker")
        );
        let staged = broker.control.source.join("styra-broker");
        let metadata = std::fs::metadata(&staged).unwrap();
        assert!(metadata.len() > 0);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_legacy_session_without_a_provider_id_is_viewable_but_not_resumable() {
        let store =
            std::env::temp_dir().join(format!("styra-server-legacy-test-{}", std::process::id()));
        let host = store.with_extension("host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));
        let workspace = crate::workspace::create(&store, &host, Some("legacy".into())).unwrap();
        let session_dir =
            crate::workspace::sessions_dir(&store, &workspace.id).join("0000000000001-1-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("journal.jsonl"),
            concat!(
                "{\"source\":\"user\",\"at_ms\":1,\"text\":\"old question\"}\n",
                "{\"source\":\"agent\",\"at_ms\":2,\"raw\":\"{\\\"method\\\":\\\"item/completed\\\",\\\"params\\\":{\\\"item\\\":{\\\"type\\\":\\\"agentMessage\\\",\\\"id\\\":\\\"m\\\",\\\"text\\\":\\\"old answer\\\"}}}\"}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            session_dir.join("session.json"),
            serde_json::json!({
                "workspace_id": workspace.id,
                "selection": {
                    "provider": "codex",
                    "model": "gpt-5.6-sol",
                    "effort": "high"
                },
                "protocol": "codex-app-server"
            })
            .to_string(),
        )
        .unwrap();

        assert!(matches!(
            state
                .handle(Request::StoredSession {
                    id: "0000000000001-1-1".into()
                })
                .unwrap(),
            Response::StoredSession(_)
        ));
        let replayed =
            replayed_session_updates(&session_dir, crate::event::Protocol::CodexAppServer).unwrap();
        assert_eq!(
            replayed
                .iter()
                .map(|update| update.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(matches!(
            &replayed[0].update,
            InteractionUpdate::Event(
                crate::event::AgentEvent::UserMessage { text }
            ) if text == "old question"
        ));
        assert!(matches!(
            &replayed[1].update,
            InteractionUpdate::Event(
                crate::event::AgentEvent::AgentMessage { text }
            ) if text == "old answer"
        ));
        assert!(replayed[2..]
            .iter()
            .all(|update| matches!(update.update, InteractionUpdate::Raw(_))));
        let error = state
            .resume_session(ResumeSession {
                id: "0000000000001-1-1".into(),
                network: false,
                templates: Vec::new(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("can be viewed but not resumed"));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    #[test]
    fn native_session_lookup_detects_removal() {
        let root = std::env::temp_dir().join(format!("styra-native-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/rollout-provider-7.jsonl"), "").unwrap();
        assert!(tree_has_session(&root, "provider-7"));
        assert!(!tree_has_session(&root, "provider-gone"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn workspace_api_creates_lists_and_scopes_sessions() {
        let store = std::env::temp_dir().join(format!(
            "styra-server-workspace-test-{}",
            std::process::id()
        ));
        let host = store.with_extension("host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));

        let created = match state
            .handle(Request::CreateWorkspace(CreateWorkspace {
                host_path: host.clone(),
                name: Some("api test".into()),
            }))
            .unwrap()
        {
            Response::WorkspaceCreated(workspace) => workspace,
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(created.name.as_deref(), Some("api test"));
        assert!(matches!(
            state.handle(Request::ListWorkspaces).unwrap(),
            Response::Workspaces(workspaces) if workspaces == vec![created.clone()]
        ));
        assert!(matches!(
            state
                .handle(Request::ListSessions {
                    workspace_id: created.id,
                })
                .unwrap(),
            Response::StoredSessions(sessions) if sessions.is_empty()
        ));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }
}
