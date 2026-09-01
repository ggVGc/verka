//! Styra's Unix-socket server and server-owned interaction manager.

use crate::agent::{MountSpec, SandboxLayout, Selection};
use crate::interaction::{Interaction, InteractionSpec, ResolvedTemplate, SandboxBroker};
use crate::journal::{self, Journal};
use crate::protocol::{
    Answer, Contract, DrivaOptions, InteractionActivity, InteractionSummary, InteractionUpdate,
    LaunchMount, LaunchPolicy, QueuedMessage, SendMessage, SessionOrigin, SessionSummary,
    TemplateSummary,
};
use crate::protocol::{
    CreateSession, CreateWorkspace, Health, Request, Response, ResumeSession, SequencedUpdate,
    SessionInfo, ShellInfo, StoredSession, Updates, WireResponse, MAX_REQUEST_BYTES,
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

/// Stands in for the session id in a planned launch: the directory it names is
/// only created once the session exists.
const PENDING_SESSION_ID: &str = "<pending>";

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
    queue: Mutex<std::collections::VecDeque<QueuedMessage>>,
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
            last_message: self.last_message(),
        }
    }

    /// The last thing the agent said, as one clipped line. Scanned from the
    /// tail so a long-running interaction pays for the trailing tool traffic
    /// only, not for its whole history.
    fn last_message(&self) -> Option<String> {
        let updates = self.updates.lock().expect("interaction updates poisoned");
        updates
            .iter()
            .rev()
            .find_map(|sequenced| match &sequenced.update {
                InteractionUpdate::Event(crate::event::AgentEvent::AgentMessage { text }) => {
                    Some(one_line(text))
                }
                _ => None,
            })
    }
}

/// Collapse a message to a single display line: whitespace runs (including the
/// newlines of a multi-paragraph answer) become single spaces, and the result
/// is clipped so one verbose agent cannot cost every listing its bandwidth.
fn one_line(text: &str) -> String {
    const LIMIT: usize = 200;
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flattened.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}…", &flattened[..end]),
        None => flattened,
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
        self.interaction.set_selection(&selection, &current)?;
        journal::store_session_selection(&self.session_path, &selection)?;
        *current = selection;
        Ok(())
    }

    fn set_working_directory(&self, requested: PathBuf) -> Result<()> {
        if *self
            .activity
            .lock()
            .expect("interaction activity lock poisoned")
            != InteractionActivity::Pending
        {
            anyhow::bail!(
                "wait for the current turn to finish before changing its working directory"
            );
        }
        let host_directory = if requested.is_absolute() {
            requested
        } else {
            self.workspace.join(requested)
        }
        .canonicalize()
        .context("working directory must exist")?;
        let relative = host_directory.strip_prefix(&self.workspace).map_err(|_| {
            anyhow::anyhow!("working directory must be inside this interaction's Workspace")
        })?;
        let sandbox_directory = self.driva.working_directory.join(relative);
        self.interaction.set_working_directory(sandbox_directory)?;
        self.interaction.report_working_directory(host_directory);
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

    fn persist_queue(&self, queue: &std::collections::VecDeque<QueuedMessage>) -> Result<()> {
        let messages: Vec<QueuedMessage> = queue.iter().cloned().collect();
        journal::write_queued_messages(&self.session_path, &messages)
    }

    /// Queue a message as it was composed, contract and all: it is sent later
    /// but asked for now, and the shape belongs to the question.
    fn queue_message(&self, message: &SendMessage) -> Result<usize> {
        let mut queue = self.queue.lock().expect("interaction queue lock poisoned");
        queue.push_back(QueuedMessage::new(&message.text).asking_for(message.contract));
        self.persist_queue(&queue)?;
        Ok(queue.len())
    }

    fn take_queued_message(&self) -> Result<Option<QueuedMessage>> {
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

    fn queued_messages(&self) -> Vec<QueuedMessage> {
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
        // The Workspace's standing policy with this launch's own over it. Done
        // here rather than in the client so every launch path resolves the same
        // way, and a client cannot launch under something the plan did not show.
        let launch = LaunchPolicy::merge(&owning_workspace.launch, &request.launch);
        let mut profile = crate::agent::resolve_profile(&selection, &self.inner.layout)?;
        profile.network = profile.network || launch.grants_network();
        let template = resolve_templates(&workspace, &launch.templates)?;
        let extra_mounts = resolve_launch_mounts(&launch.mounts)?;
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
            extra_mounts,
            template,
            broker: Some(self.prepare_broker(&id, tmux)?),
        };
        let driva = DrivaOptions::capture(&spec, "bwrap");
        let prepared_broker = spec.broker.as_ref().expect("broker was prepared");
        let shell = ShellInfo {
            tmux: prepared_broker.tmux.clone(),
            socket: prepared_broker.control.source.join("tmux.sock"),
        };
        // A policy Driva would refuse is rejected here, before anything is
        // spawned, and takes the same control-directory cleanup a failed spawn
        // does so a rejected launch leaves nothing behind.
        if let Err(error) = ensure_distinct_destinations(&driva) {
            if let Some(control) = shell.socket.parent() {
                std::fs::remove_dir_all(control).ok();
            }
            return Err(error);
        }
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
                // Once the provider reports its background-task set, that
                // count alone drives `background_work`; the tool-call
                // heuristics below are a fallback for quieter providers.
                let mut background_count_known = false;
                let mut background_polls = HashSet::new();
                while let Ok(update) = receiver.recv() {
                    match &update {
                        InteractionUpdate::Event(event)
                            if event.background_tasks_running().is_some() =>
                        {
                            let running = event
                                .background_tasks_running()
                                .expect("guard checked the running count is present");
                            background_count_known = true;
                            background_work = running > 0;
                            if !background_work {
                                let mut activity =
                                    activity.lock().expect("interaction activity lock poisoned");
                                if *activity == InteractionActivity::Background {
                                    *activity = InteractionActivity::Pending;
                                }
                            }
                        }
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
                            if !background_count_known && update_finishes_background(&update) {
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
            // The seed message is a turn like any other, so a contract on it is
            // framed and recorded exactly as `SendMessage` does. Doing it here
            // is what makes a typed one-shot a single request.
            let message = match request.contract {
                Some(contract) => {
                    journal::store_session_contract(&managed.session_path, contract)?;
                    crate::contract::frame(message, contract)
                }
                None => message.to_owned(),
            };
            if let Err(error) = managed.send(&message) {
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

    /// Describe the sandbox a [`Self::create_session`] with these inputs would
    /// launch, without creating a session, a journal, or a control directory.
    /// It resolves the profile, template overlay and mounts the same way the
    /// real launch does, so what the operator is shown before their first
    /// message is what they will get. The one thing it cannot name is the
    /// session id, so the broker control mount carries a placeholder for the
    /// directory the launch will make.
    fn plan_session(&self, request: crate::protocol::PlanSession) -> Result<DrivaOptions> {
        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &request.workspace_id)?;
        let workspace = owning_workspace.host_path;
        let launch = LaunchPolicy::merge(&owning_workspace.launch, &request.launch);
        let mut profile = crate::agent::resolve_profile(&request.selection, &self.inner.layout)?;
        profile.network = profile.network || launch.grants_network();
        let template = resolve_templates(&workspace, &launch.templates)?;
        let extra_mounts = resolve_launch_mounts(&launch.mounts)?;
        let tmux = genta::agent::resolve_executable(Path::new("tmux"))
            .context("tmux is required for Styra session shells")?;
        let spec = InteractionSpec {
            profile,
            resume_provider_session_id: None,
            working_directory: self.inner.layout.workspace.clone(),
            workspace: MountSpec {
                source: workspace,
                destination: self.inner.layout.workspace.clone(),
                writable: true,
            },
            temporary_mounts: Vec::new(),
            extra_mounts,
            template,
            broker: Some(self.describe_broker(PENDING_SESSION_ID, tmux)),
        };
        let options = DrivaOptions::capture(&spec, "bwrap");
        // Planning is also where a policy gets checked: an operator editing
        // mounts before launch learns that two of them collide now, from the
        // view they are editing, rather than from a failed launch later.
        ensure_distinct_destinations(&options)?;
        Ok(options)
    }

    /// The Driva templates a launch in this Workspace could name.
    fn list_templates(&self, workspace_id: &str) -> Result<Vec<TemplateSummary>> {
        let owning_workspace = crate::workspace::get(&self.inner.store_root, workspace_id)?;
        Ok(workspace_driva_config(&owning_workspace.host_path)?
            .effective_templates()
            .into_iter()
            .map(|(name, template)| TemplateSummary {
                name,
                description: template.description,
            })
            .collect())
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
        let launch = LaunchPolicy::merge(&owning_workspace.launch, &request.launch);
        let mut profile = crate::agent::resolve_profile(&selection, &self.inner.layout)?;
        profile.resume(selection.provider, &provider_session_id)?;
        profile.network = profile.network || launch.grants_network();
        let template = resolve_templates(&workspace, &launch.templates)?;
        let extra_mounts = resolve_launch_mounts(&launch.mounts)?;

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
            extra_mounts,
            template,
            broker: Some(self.prepare_broker(&request.id, tmux)?),
        };
        let driva = DrivaOptions::capture(&spec, "bwrap");
        let prepared_broker = spec.broker.as_ref().expect("broker was prepared");
        let shell = ShellInfo {
            tmux: prepared_broker.tmux.clone(),
            socket: prepared_broker.control.source.join("tmux.sock"),
        };
        if let Err(error) = ensure_distinct_destinations(&driva) {
            if let Some(control) = shell.socket.parent() {
                std::fs::remove_dir_all(control).ok();
            }
            return Err(error);
        }
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
                // Once the provider reports its background-task set, that
                // count alone drives `background_work`; the tool-call
                // heuristics below are a fallback for quieter providers.
                let mut background_count_known = false;
                let mut background_polls = HashSet::new();
                while let Ok(update) = receiver.recv() {
                    match &update {
                        InteractionUpdate::Event(event)
                            if event.background_tasks_running().is_some() =>
                        {
                            let running = event
                                .background_tasks_running()
                                .expect("guard checked the running count is present");
                            background_count_known = true;
                            background_work = running > 0;
                            if !background_work {
                                let mut activity =
                                    activity.lock().expect("interaction activity lock poisoned");
                                if *activity == InteractionActivity::Background {
                                    *activity = InteractionActivity::Pending;
                                }
                            }
                        }
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
                            if !background_count_known && update_finishes_background(&update) {
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

    /// Convert a stored Session's native transcript to the other interactive
    /// provider, keeping its whole history. Sugar over [`Self::branch_session`]
    /// for the common case, kept as its own operation so a caller does not
    /// need to name the general one just to flip providers.
    fn convert_session_provider(&self, id: &str) -> Result<SessionSummary> {
        let summary = self.stored_summary(id)?;
        let to_provider = other_interactive_provider(summary.selection.provider)?;
        self.branch_session(id, None, Some(to_provider))
            .with_context(|| {
                format!(
                    "converting session {id:?} from {} to {}",
                    summary.selection.provider.as_str(),
                    to_provider.as_str()
                )
            })
    }

    /// Branch a stored Session into a new sibling Session in the same
    /// Workspace, seeded with its history up to `at_ms` (the whole history
    /// when `None`), optionally under a different provider. The source
    /// Session, its native transcript, and its Styra journal are left
    /// untouched — a branch is always a fresh copy, never a live reference,
    /// so nothing the source does afterwards is visible on the branch and
    /// nothing the branch does is visible on the source.
    ///
    /// The branch always gets a fresh native provider session id, even when
    /// the provider does not change: Genta's own conversion only generates
    /// one when the format changes, but a same-provider branch still needs
    /// its own id, or the provider's own `--resume` lookup — which searches
    /// its whole session tree by id — could not tell the two apart.
    fn branch_session(
        &self,
        id: &str,
        at_ms: Option<u64>,
        provider: Option<crate::agent::Provider>,
    ) -> Result<SessionSummary> {
        let summary = self.stored_summary(id)?;
        let from_provider = summary.selection.provider;
        let to_provider = provider.unwrap_or(from_provider);
        if !crate::agent::PROVIDERS.contains(&to_provider) {
            anyhow::bail!(
                "provider {:?} is not an interactive provider Styra can branch into",
                to_provider.as_str()
            );
        }
        let provider_session_id = journal::read_provider_session_id(&summary.path)?
            .with_context(|| {
                format!("session {id:?} has no stored provider session id; there is no native transcript to branch from")
            })?;
        let source_path = find_native_session_file(from_provider, &provider_session_id)?;
        let source = std::fs::read_to_string(&source_path)
            .with_context(|| format!("reading {}", source_path.display()))?;

        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &summary.workspace_id)?;
        let cwd = owning_workspace.host_path;
        let keep_messages = at_ms
            .map(|cutoff| messages_up_to(&source, native_session_format(from_provider), cutoff))
            .transpose()?;

        let new_native_id = uuid::Uuid::new_v4().to_string();
        let options = genta::session::ConversionOptions {
            id: Some(new_native_id.clone()),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            keep_messages,
            ..Default::default()
        };
        let branched = genta::session::convert(
            &source,
            native_session_format(from_provider),
            native_session_format(to_provider),
            &options,
        )?;

        let destination = native_session_destination(to_provider, &new_native_id, &cwd)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&destination, &branched)
            .with_context(|| format!("writing {}", destination.display()))?;

        // A same-provider branch is a checkpoint: it keeps running under the
        // exact model and effort the source had, not the provider's defaults.
        let selection = if to_provider == from_provider {
            summary.selection.clone()
        } else {
            Selection::new(to_provider)
        };
        let profile = crate::agent::resolve_profile(&selection, &self.inner.layout)?;
        let (mut journal, new_id) = Journal::create_in_workspace(
            &self.inner.store_root,
            &summary.workspace_id,
            &profile,
            &selection,
            summary.name.clone(),
        )?;
        let source_protocol = journal::read_session_meta(&summary.path)?.protocol;
        journal.copy_prefix_from(&summary.path, source_protocol, at_ms)?;
        let directory = journal
            .path()
            .parent()
            .context("a freshly created session journal has a parent directory")?;
        journal::store_provider_session_id(directory, &new_native_id)?;
        if !summary.notes.is_empty() {
            journal::store_session_notes(directory, summary.notes.clone())?;
        }
        journal::store_session_origin(
            directory,
            SessionOrigin {
                session_id: id.to_owned(),
                provider: from_provider,
                at_ms,
            },
        )?;

        self.stored_summary(&new_id)
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

    /// Where a session's broker lives and how it is mounted, named but not yet
    /// created. Splitting this from [`Self::prepare_broker`] lets the launch
    /// policy be described — for a session that does not exist yet — without
    /// staging anything on disk for it.
    fn describe_broker(&self, id: &str, tmux: PathBuf) -> SandboxBroker {
        let control_root = self
            .inner
            .socket
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sandboxes");
        SandboxBroker {
            executable: PathBuf::from("/tmp/styra/control/styra-broker"),
            tmux,
            control: MountSpec {
                source: control_root.join(id),
                destination: PathBuf::from("/tmp/styra/control"),
                writable: true,
            },
            socket: PathBuf::from("/tmp/styra/control/tmux.sock"),
        }
    }

    fn prepare_broker(&self, id: &str, tmux: PathBuf) -> Result<SandboxBroker> {
        let broker = self.describe_broker(id, tmux);
        let control = broker.control.source.clone();
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
        Ok(broker)
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

    /// Parse a session's most recent agent message as a typed answer.
    ///
    /// A live interaction is answered from the updates it has already
    /// collected, which are the same events the interface is rendering; a
    /// session with no live interaction is answered by replaying its journal.
    /// Both paths reach the same decoded [`AgentEvent`]s, so an answer does not
    /// depend on whether the session that produced it is still running.
    fn turn_answer(&self, id: &str, contract: Option<Contract>) -> Result<Answer> {
        let live = self
            .inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .get(id)
            .cloned();
        let (events, session_path) = match live {
            Some(interaction) => {
                let events = interaction
                    .updates
                    .lock()
                    .expect("interaction update lock poisoned")
                    .iter()
                    .filter_map(|update| match &update.update {
                        InteractionUpdate::Event(event) => Some(event.clone()),
                        _ => None,
                    })
                    .collect();
                (events, interaction.session_path.clone())
            }
            None => {
                let summary = self.stored_summary(id)?;
                let meta = journal::read_session_meta(&summary.path)?;
                (journal::replay(&summary.path, meta.protocol)?, summary.path)
            }
        };
        // An explicit contract re-reads an existing answer as another shape,
        // which is also the only way to type a turn that was sent untyped.
        let contract = match contract {
            Some(contract) => contract,
            None => journal::read_session_contract(&session_path)?.with_context(|| {
                format!("session {id:?} has no typed turn to answer; name a contract to parse its last message as one")
            })?,
        };
        crate::contract::answer_from_events(&events, contract)
    }

    fn stored_summary(&self, id: &str) -> Result<SessionSummary> {
        // Probe each Workspace for this exact id rather than listing every
        // Session in it: the directory name *is* the id, so a stat per
        // Workspace replaces reading every stored session's metadata.
        for workspace in crate::workspace::list(&self.inner.store_root)? {
            if let Some(session) =
                journal::find_workspace_session(&self.inner.store_root, &workspace.id, id)?
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
            Request::WorkspaceLaunch { workspace_id } => Ok(Response::WorkspaceLaunch(
                crate::workspace::launch(&self.inner.store_root, &workspace_id)?,
            )),
            Request::CreateSession(request) => {
                Ok(Response::SessionCreated(self.create_session(request)?))
            }
            Request::PlanSession(request) => Ok(Response::SessionPlan(self.plan_session(request)?)),
            Request::ListTemplates { workspace_id } => {
                Ok(Response::Templates(self.list_templates(&workspace_id)?))
            }
            Request::ResumeSession(request) => {
                Ok(Response::SessionResumed(self.resume_session(request)?))
            }
            Request::ConvertSessionProvider { id } => Ok(Response::SessionConverted(
                self.convert_session_provider(&id)?,
            )),
            Request::BranchSession {
                id,
                at_ms,
                provider,
            } => Ok(Response::SessionBranched(
                self.branch_session(&id, at_ms, provider)?,
            )),
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
            Request::ChangeWorkspaceLaunch {
                workspace_id,
                change,
            } => Ok(Response::WorkspaceLaunchUpdated(
                crate::workspace::change_launch(&self.inner.store_root, &workspace_id, change)?,
            )),
            Request::SendMessage { id, message } => {
                let interaction = self.interaction(&id)?;
                // A client that names a selection on the turn is switching the
                // session onto it, not just this message: adopt it durably and
                // then send under whatever the session now runs.
                if let Some(selection) = message.selection {
                    interaction.set_selection(selection)?;
                }
                // A typed turn is framed here, not by the client, so every
                // caller asks for a shape in the same words. Recorded before
                // sending: an answer that arrives against an unrecorded
                // contract could not be parsed.
                let text = match message.contract {
                    Some(contract) => {
                        journal::store_session_contract(&interaction.session_path, contract)?;
                        crate::contract::frame(&message.text, contract)
                    }
                    None => message.text,
                };
                interaction
                    .interaction
                    .send_with_selection(&text, Some(&interaction.selection()))?;
                Ok(Response::Accepted)
            }
            Request::SetSessionSelection { id, selection } => {
                crate::agent::validate_selection(&selection)?;
                self.interaction(&id)?.set_selection(selection)?;
                Ok(Response::Accepted)
            }
            Request::SetInteractionWorkingDirectory { id, directory } => {
                self.interaction(&id)?.set_working_directory(directory)?;
                Ok(Response::Accepted)
            }
            Request::QueueMessage { id, message } => Ok(Response::Queued(
                self.interaction(&id)?.queue_message(&message)?,
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
            Request::Updates { id, after, raw } => {
                let interaction = self.interaction(&id)?;
                let all = interaction
                    .updates
                    .lock()
                    .expect("interaction update lock poisoned");
                // A client that renders no raw view (the picker preview) asks
                // for none: raw lines are the bulk of an interaction's volume,
                // and cloning then shipping them only to be dropped is what
                // made replaying a long session from zero slow.
                let updates = all
                    .iter()
                    .filter(|update| update.sequence > after)
                    .filter(|update| raw || !matches!(update.update, InteractionUpdate::Raw(_)))
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
            Request::StoredSession { id, raw: want_raw } => {
                let summary = self.stored_summary(&id)?;
                let meta = journal::read_session_meta(&summary.path)?;
                let events = journal::replay(&summary.path, meta.protocol)?;
                // Reconstructing the raw lines re-reads and re-parses the whole
                // journal, so only pay for it when the caller renders them.
                let raw = if want_raw {
                    journal::replay_raw(&summary.path)?
                } else {
                    Vec::new()
                };
                Ok(Response::StoredSession(StoredSession {
                    summary,
                    events,
                    raw,
                }))
            }
            Request::Shell { id } => Ok(Response::Shell(self.shell(&id)?)),
            Request::TurnAnswer { id, contract } => {
                Ok(Response::Answer(self.turn_answer(&id, contract)?))
            }
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
    find_native_session_file(provider, provider_session_id).map(|_| ())
}

/// The provider's own on-disk roots for its native, resumable session
/// transcripts.
fn native_session_roots(provider: crate::agent::Provider) -> Result<Vec<PathBuf>> {
    let home = std::env::var_os("HOME").context("HOME is required to locate provider sessions")?;
    let home = PathBuf::from(home);
    Ok(match provider {
        crate::agent::Provider::Codex => vec![
            home.join(".codex/sessions"),
            home.join(".codex/archived_sessions"),
        ],
        crate::agent::Provider::Claude => vec![home.join(".claude/projects")],
        crate::agent::Provider::CodexExec => {
            anyhow::bail!("provider codex-exec does not support resuming sessions")
        }
    })
}

/// Find a provider's native transcript file by id, searching its known
/// storage roots. Sessions are exempt from styra's picture of where within a
/// root they live (Codex nests by date; Claude nests by project), so this
/// walks the whole tree rather than assuming a layout.
fn find_native_session_file(
    provider: crate::agent::Provider,
    provider_session_id: &str,
) -> Result<PathBuf> {
    native_session_roots(provider)?
        .iter()
        .find_map(|root| find_session_file(root, provider_session_id))
        .with_context(|| {
            format!(
                "{} session {:?} does not exist anymore; the Styra transcript is still available read-only",
                provider.as_str(),
                provider_session_id
            )
        })
}

fn find_session_file(root: &Path, provider_session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            if let Some(found) = find_session_file(&path, provider_session_id) {
                return Some(found);
            }
        } else if entry
            .file_name()
            .to_string_lossy()
            .contains(provider_session_id)
        {
            return Some(path);
        }
    }
    None
}

/// The other of Styra's two interactive providers — conversion always flips
/// between exactly these two, since Genta's batch-only `codex-exec` has no
/// resumable native session to convert to or from.
fn other_interactive_provider(provider: crate::agent::Provider) -> Result<crate::agent::Provider> {
    match provider {
        crate::agent::Provider::Codex => Ok(crate::agent::Provider::Claude),
        crate::agent::Provider::Claude => Ok(crate::agent::Provider::Codex),
        crate::agent::Provider::CodexExec => {
            anyhow::bail!("provider codex-exec does not support session conversion")
        }
    }
}

/// The native transcript format Genta's session conversion reads and writes
/// for a given provider.
fn native_session_format(provider: crate::agent::Provider) -> genta::session::SessionFormat {
    match provider {
        crate::agent::Provider::Codex | crate::agent::Provider::CodexExec => {
            genta::session::SessionFormat::Codex
        }
        crate::agent::Provider::Claude => genta::session::SessionFormat::Claude,
    }
}

/// How many of a native transcript's leading messages have a timestamp at or
/// before `cutoff`. Used to turn a UI-selected moment (a [`RawLine::at_ms`],
/// milliseconds since the epoch) into Genta's `keep_messages` count: the two
/// histories are decoded differently — Styra's journal replays its own
/// records, this parses the provider's own transcript — so time is the only
/// axis they agree on. Messages are recorded in order, so this is the length
/// of their leading run at or before the cutoff, not a filtered count; a
/// message with no timestamp, or one Styra cannot parse, is kept rather than
/// used to end that run, since excluding it could silently drop history the
/// operator meant to keep.
fn messages_up_to(
    source: &str,
    format: genta::session::SessionFormat,
    cutoff: u64,
) -> Result<usize> {
    let session = genta::session::parse(source, format)?;
    Ok(session
        .messages
        .iter()
        .take_while(|message| {
            message
                .timestamp
                .as_deref()
                .and_then(parse_rfc3339_ms)
                .is_none_or(|at_ms| at_ms <= cutoff)
        })
        .count())
}

/// Parse a provider's RFC 3339 message timestamp (e.g.
/// `"2026-08-21T10:00:00.000Z"`) into milliseconds since the epoch, so it can
/// be compared against a [`RawLine::at_ms`] cutoff. `None` on anything that
/// does not parse — callers treat that as "keep it" rather than as an error,
/// since one malformed timestamp should not fail an entire branch.
fn parse_rfc3339_ms(timestamp: &str) -> Option<u64> {
    let parsed =
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
            .ok()?;
    u64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Where a freshly converted transcript must be written for its destination
/// provider's own resume to find it: Claude scopes sessions under a
/// per-project directory keyed by the encoded working directory; Codex scans
/// its whole sessions tree by id, so a dedicated subdirectory keeps
/// Genta-imported rollouts apart from Codex's own date-nested ones.
fn native_session_destination(
    provider: crate::agent::Provider,
    session_id: &str,
    cwd: &Path,
) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .context("HOME is required to place a converted provider session")?;
    let home = PathBuf::from(home);
    match provider {
        crate::agent::Provider::Claude => Ok(home
            .join(".claude/projects")
            .join(claude_project_directory_name(cwd))
            .join(format!("{session_id}.jsonl"))),
        crate::agent::Provider::Codex => Ok(home
            .join(".codex/sessions/genta-imported")
            .join(format!("rollout-{session_id}.jsonl"))),
        crate::agent::Provider::CodexExec => {
            anyhow::bail!("provider codex-exec cannot store a resumable session")
        }
    }
}

/// Claude Code's own encoding of a project's working directory into its
/// session storage directory name: every character that is not plain ASCII
/// alphanumeric becomes `-`.
fn claude_project_directory_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

/// Resolve and merge the named Driva templates against a `driva.toml` in the
/// operator's workspace, if any (falling back to Driva's built-ins), in the
/// order given: later names take precedence on conflicting settings, mirroring
/// `driva run --template` layering.
fn resolve_templates(workspace: &Path, names: &[String]) -> Result<Option<ResolvedTemplate>> {
    if names.is_empty() {
        return Ok(None);
    }
    let driva_config = workspace_driva_config(workspace)?;
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

/// The Driva configuration a launch in this Workspace resolves against: the
/// Workspace's own `driva.toml` when it has one, otherwise Driva's built-ins.
fn workspace_driva_config(workspace: &Path) -> Result<driva::Config> {
    let candidate = workspace.join("driva.toml");
    if candidate.exists() {
        driva::Config::load(&candidate)
    } else {
        Ok(driva::Config::default())
    }
}

/// Turn the operator's mount requests into concrete bind mounts.
///
/// The source is canonicalized here rather than at launch, so a path that does
/// not exist (or a typo) is reported while the operator is still editing the
/// policy, instead of surfacing later as a bwrap failure with no context. A
/// destination is optional and defaults to the canonical source, matching
/// Driva's rule for a bind mount that names no destination.
fn resolve_launch_mounts(mounts: &[LaunchMount]) -> Result<Vec<MountSpec>> {
    mounts
        .iter()
        .map(|mount| {
            let source = driva::canonicalize_mount(&mount.source).with_context(|| {
                format!("invalid extra mount source {}", mount.source.display())
            })?;
            let destination = mount.destination.clone().unwrap_or_else(|| source.clone());
            if !destination.is_absolute() {
                anyhow::bail!(
                    "extra mount destination {} must be an absolute path inside the sandbox",
                    destination.display()
                );
            }
            Ok(MountSpec {
                source,
                destination,
                writable: mount.writable,
            })
        })
        .collect()
}

/// Reject a policy that binds two things at the same place inside the sandbox.
///
/// Driva itself refuses this when it validates the request, but that is at
/// spawn time. Checking the captured policy means an extra mount that lands on
/// top of the workspace (or on a template's grant) is reported while it is
/// still being chosen, rather than as a failed launch afterwards.
fn ensure_distinct_destinations(options: &DrivaOptions) -> Result<()> {
    let mut seen = HashSet::new();
    for attributed in &options.mounts {
        let destination = match &attributed.mount {
            driva::Mount::Bind { destination, .. }
            | driva::Mount::Overlay { destination, .. }
            | driva::Mount::Temporary { destination } => destination,
        };
        if !seen.insert(destination.clone()) {
            anyhow::bail!(
                "conflicting mount destination: {} is already bound",
                destination.display()
            );
        }
    }
    Ok(())
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
    use crate::protocol::{AttributedMount, MountOrigin};

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

    /// An operator's mount request is resolved against the host now, so a path
    /// that is not there is reported while the policy is being chosen.
    #[test]
    fn extra_mounts_resolve_their_source_and_default_the_destination_to_it() {
        let root =
            std::env::temp_dir().join(format!("styra-server-extra-mounts-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("data")).unwrap();

        let resolved = resolve_launch_mounts(&[
            LaunchMount {
                source: root.join("data"),
                destination: None,
                writable: true,
            },
            LaunchMount {
                source: root.join("data"),
                destination: Some(PathBuf::from("/mnt/data")),
                writable: false,
            },
        ])
        .unwrap();

        assert_eq!(
            resolved[0].source,
            root.join("data").canonicalize().unwrap()
        );
        assert_eq!(resolved[0].destination, resolved[0].source);
        assert!(resolved[0].writable);
        assert_eq!(resolved[1].destination, PathBuf::from("/mnt/data"));
        assert!(!resolved[1].writable);

        let missing = resolve_launch_mounts(&[LaunchMount {
            source: root.join("absent"),
            destination: None,
            writable: false,
        }])
        .unwrap_err();
        assert!(
            missing.to_string().contains("invalid extra mount source"),
            "{missing}"
        );

        let relative = resolve_launch_mounts(&[LaunchMount {
            source: root.join("data"),
            destination: Some(PathBuf::from("mnt/data")),
            writable: false,
        }])
        .unwrap_err();
        assert!(relative.to_string().contains("absolute"), "{relative}");

        std::fs::remove_dir_all(root).ok();
    }

    /// Two mounts landing on the same place inside the sandbox is a policy
    /// Driva refuses, so it is caught while planning rather than at spawn.
    #[test]
    fn a_policy_binding_two_things_at_one_destination_is_rejected() {
        let options = DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: vec![
                AttributedMount {
                    origin: MountOrigin::Workspace,
                    mount: driva::Mount::Bind {
                        source: PathBuf::from("/srv/one"),
                        destination: PathBuf::from("/tmp/styra/workspace"),
                        access: driva::MountAccess::ReadWrite,
                    },
                },
                AttributedMount {
                    origin: MountOrigin::Operator,
                    mount: driva::Mount::Bind {
                        source: PathBuf::from("/srv/two"),
                        destination: PathBuf::from("/tmp/styra/workspace"),
                        access: driva::MountAccess::ReadOnly,
                    },
                },
            ],
        };
        let error = ensure_distinct_destinations(&options).unwrap_err();
        assert!(
            error.to_string().contains("conflicting mount destination"),
            "{error}"
        );

        let mut fine = options;
        fine.mounts.pop();
        assert!(ensure_distinct_destinations(&fine).is_ok());
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
    fn describing_a_broker_names_the_same_mount_without_staging_anything() {
        let root =
            std::env::temp_dir().join(format!("styra-server-plan-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let state = ServerState::new(root.join("store"), root.join("styra.sock"));

        let planned = state.describe_broker(PENDING_SESSION_ID, PathBuf::from("/usr/bin/tmux"));
        let real = state.describe_broker("session-1", PathBuf::from("/usr/bin/tmux"));

        assert_eq!(planned.control.destination, real.control.destination);
        assert_eq!(planned.executable, real.executable);
        assert_eq!(
            planned.control.source,
            root.join("sandboxes").join(PENDING_SESSION_ID)
        );
        assert!(!planned.control.source.exists());
        assert!(!root.join("sandboxes").exists());
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
                    id: "0000000000001-1-1".into(),
                    raw: true,
                })
                .unwrap(),
            Response::StoredSession(_)
        ));
        // The same session asked for without raw lines: the decoded events are
        // still there, the verbatim wire log is not shipped at all.
        let Response::StoredSession(events_only) = state
            .handle(Request::StoredSession {
                id: "0000000000001-1-1".into(),
                raw: false,
            })
            .unwrap()
        else {
            panic!("expected a stored session");
        };
        assert_eq!(events_only.events.len(), 2);
        assert!(events_only.raw.is_empty());
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
                launch: LaunchPolicy::default(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("can be viewed but not resumed"));

        // Conversion needs the provider's native transcript just as resume
        // does. A legacy Styra-only journal must fail without making a
        // partially-created sibling session appear in the picker.
        let error = state
            .convert_session_provider("0000000000001-1-1")
            .unwrap_err();
        assert!(
            error.to_string().contains("converting session"),
            "{error:#}"
        );
        assert!(
            format!("{error:#}").contains("no stored provider session id"),
            "{error:#}"
        );
        assert_eq!(
            crate::workspace::get(&store, &workspace.id)
                .unwrap()
                .session_count,
            1
        );

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    #[test]
    fn native_session_lookup_detects_removal() {
        let root = std::env::temp_dir().join(format!("styra-native-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/rollout-provider-7.jsonl"), "").unwrap();
        assert!(find_session_file(&root, "provider-7").is_some());
        assert!(find_session_file(&root, "provider-gone").is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn conversion_always_flips_between_styras_two_interactive_providers() {
        assert_eq!(
            other_interactive_provider(crate::agent::Provider::Codex).unwrap(),
            crate::agent::Provider::Claude
        );
        assert_eq!(
            other_interactive_provider(crate::agent::Provider::Claude).unwrap(),
            crate::agent::Provider::Codex
        );
        assert!(other_interactive_provider(crate::agent::Provider::CodexExec).is_err());
    }

    #[test]
    fn native_session_format_matches_each_providers_own_transcript() {
        assert_eq!(
            native_session_format(crate::agent::Provider::Codex),
            genta::session::SessionFormat::Codex
        );
        assert_eq!(
            native_session_format(crate::agent::Provider::CodexExec),
            genta::session::SessionFormat::Codex
        );
        assert_eq!(
            native_session_format(crate::agent::Provider::Claude),
            genta::session::SessionFormat::Claude
        );
    }

    #[test]
    fn claude_project_directory_names_replace_every_non_alphanumeric_character() {
        assert_eq!(
            claude_project_directory_name(Path::new("/home/op/.dotfiles/project")),
            "-home-op--dotfiles-project"
        );
    }

    #[test]
    fn a_converted_destination_is_scoped_by_provider() {
        let claude = native_session_destination(
            crate::agent::Provider::Claude,
            "new-id",
            Path::new("/home/op/project"),
        )
        .unwrap();
        assert!(claude.ends_with("-home-op-project/new-id.jsonl"));

        let codex =
            native_session_destination(crate::agent::Provider::Codex, "new-id", Path::new("/x"))
                .unwrap();
        assert!(codex.ends_with("genta-imported/rollout-new-id.jsonl"));

        assert!(native_session_destination(
            crate::agent::Provider::CodexExec,
            "new-id",
            Path::new("/x")
        )
        .is_err());
    }

    #[test]
    fn rfc3339_timestamps_parse_to_milliseconds_since_the_epoch() {
        assert_eq!(
            parse_rfc3339_ms("2026-08-21T10:00:00.000Z"),
            Some(1787306400000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-08-21T10:00:00.500Z"),
            Some(1787306400500)
        );
        assert_eq!(parse_rfc3339_ms("not a timestamp"), None);
    }

    #[test]
    fn messages_up_to_keeps_the_leading_run_at_or_before_the_cutoff() {
        let claude = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"first"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:02.000Z","message":{"role":"user","content":"third"}}"#,
            "\n",
        );
        let cutoff = parse_rfc3339_ms("2026-08-21T10:00:01.000Z").unwrap();
        assert_eq!(
            messages_up_to(claude, genta::session::SessionFormat::Claude, cutoff).unwrap(),
            2
        );
        assert_eq!(
            messages_up_to(claude, genta::session::SessionFormat::Claude, 0).unwrap(),
            0
        );
        assert_eq!(
            messages_up_to(claude, genta::session::SessionFormat::Claude, u64::MAX).unwrap(),
            3
        );
    }

    /// The standing policy is stored with the Workspace, not with the client
    /// that set it, so every client launching there reads the same one back.
    #[test]
    fn a_workspace_launch_policy_is_stored_and_reported_back() {
        let store = std::env::temp_dir().join(format!(
            "styra-server-workspace-launch-{}",
            std::process::id()
        ));
        let host = store.with_extension("host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));

        let created = match state
            .handle(Request::CreateWorkspace(CreateWorkspace {
                host_path: host.clone(),
                name: None,
            }))
            .unwrap()
        {
            Response::WorkspaceCreated(workspace) => workspace,
            other => panic!("unexpected response: {other:?}"),
        };
        assert!(created.launch.is_empty());

        let launch = LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![LaunchMount {
                source: PathBuf::from("/srv/corpus"),
                destination: None,
                writable: false,
            }],
            standalone: false,
        };
        let updated = match state
            .handle(Request::SetWorkspaceLaunch {
                workspace_id: created.id.clone(),
                launch: launch.clone(),
            })
            .unwrap()
        {
            Response::WorkspaceLaunchUpdated(workspace) => workspace,
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(updated.launch, launch);

        match state
            .handle(Request::Workspace {
                id: created.id.clone(),
            })
            .unwrap()
        {
            Response::Workspace(workspace) => assert_eq!(workspace.launch, launch),
            other => panic!("unexpected response: {other:?}"),
        }

        // A policy cannot be stored for a Workspace that is not there.
        assert!(state
            .handle(Request::SetWorkspaceLaunch {
                workspace_id: "no-such-workspace".into(),
                launch,
            })
            .is_err());

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
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
