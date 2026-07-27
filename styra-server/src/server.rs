//! Styra's Unix-socket server and server-owned interaction manager.

use crate::agent::{MountSpec, SandboxLayout, Selection};
use crate::api::{
    CreateSession, CreateWorkspace, Health, Request, Response, ResumeSession, SequencedUpdate,
    SessionInfo, ShellInfo, StoredSession, Updates, WireResponse,
};
use crate::interaction::{Interaction, InteractionSpec, ResolvedTemplate, SandboxBroker};
use crate::journal::{self, Journal};
use crate::types::{DrivaOptions, InteractionSummary, SessionSummary};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

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
    /// Captured at spawn so the interaction can be listed and reattached to without
    /// re-deriving them: the agent selection, host workspace, and launch policy.
    workspace_id: String,
    selection: Selection,
    workspace: PathBuf,
    driva: DrivaOptions,
    shell: ShellInfo,
}

impl ManagedInteraction {
    fn summary(&self) -> InteractionSummary {
        InteractionSummary {
            id: self.interaction.session_id().to_owned(),
            workspace_id: self.workspace_id.clone(),
            selection: self.selection.clone(),
            workspace: self.workspace.clone(),
            driva: self.driva.clone(),
            accepting: self.accepting_messages.load(Ordering::Acquire),
        }
    }
}

impl ManagedInteraction {
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
        let mut profile = crate::agent::resolve_profile(&selection, &self.inner.layout)?;
        profile.network = profile.network || request.network;
        let template = resolve_templates(&workspace, &request.templates)?;
        // Resolve host tooling before creating durable session state, so a
        // missing tmux or broker executable cannot leave an empty journal.
        let tmux = genta::agent::resolve_executable(Path::new("tmux"))
            .context("tmux is required for Styra session shells")?;
        let broker_executable =
            std::env::current_exe().context("locating the Styra sandbox broker")?;
        let (journal, id) = Journal::create_in_workspace(
            &self.inner.store_root,
            &request.workspace_id,
            &profile,
            &selection,
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
            broker: Some(self.prepare_broker(&id, broker_executable, tmux)?),
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
        let managed = Arc::new(ManagedInteraction {
            interaction,
            updates: Arc::clone(&updates),
            accepting_messages: Arc::clone(&accepting_messages),
            workspace_id: request.workspace_id.clone(),
            selection: selection.clone(),
            workspace: workspace.clone(),
            driva: driva.clone(),
            shell,
        });
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                while let Ok(update) = receiver.recv() {
                    if matches!(update, crate::types::InteractionUpdate::Ended(_)) {
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
            workspace_id: request.workspace_id,
            selection,
            workspace,
            journal_path,
            driva,
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
        let broker_executable =
            std::env::current_exe().context("locating the Styra sandbox broker")?;
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
            broker: Some(self.prepare_broker(&request.id, broker_executable, tmux)?),
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
        let updates = Arc::new(Mutex::new(Vec::new()));
        let accepting_messages = Arc::new(AtomicBool::new(true));
        let managed = Arc::new(ManagedInteraction {
            interaction,
            updates: Arc::clone(&updates),
            accepting_messages: Arc::clone(&accepting_messages),
            workspace_id: summary.workspace_id.clone(),
            selection: selection.clone(),
            workspace: workspace.clone(),
            driva: driva.clone(),
            shell,
        });
        let id = request.id.clone();
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                while let Ok(update) = receiver.recv() {
                    if matches!(update, crate::types::InteractionUpdate::Ended(_)) {
                        accepting_messages.store(false, Ordering::Release);
                    }
                    let mut history = updates.lock().expect("interaction update lock poisoned");
                    let sequence = history.len() as u64 + 1;
                    history.push(SequencedUpdate { sequence, update });
                }
            })
            .context("starting the resumed interaction update collector")?;
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .insert(request.id.clone(), managed);

        Ok(SessionInfo {
            id: request.id,
            workspace_id: summary.workspace_id,
            selection,
            workspace,
            journal_path,
            driva,
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

    fn prepare_broker(
        &self,
        id: &str,
        executable: PathBuf,
        tmux: PathBuf,
    ) -> Result<SandboxBroker> {
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
        Ok(SandboxBroker {
            executable,
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
            Request::Workspace { id } => Ok(Response::Workspace(crate::workspace::get(
                &self.inner.store_root,
                &id,
            )?)),
            Request::CreateSession(request) => {
                Ok(Response::SessionCreated(self.create_session(request)?))
            }
            Request::ResumeSession(request) => {
                Ok(Response::SessionResumed(self.resume_session(request)?))
            }
            Request::SendMessage { id, message } => {
                self.interaction(&id)?.send(&message.text)?;
                Ok(Response::Accepted)
            }
            Request::StopInteraction { id } => {
                self.interaction(&id)?.stop();
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
    let wire = match read_request(&stream).and_then(|request| state.handle(request)) {
        Ok(response) => WireResponse::Ok { response },
        Err(error) => WireResponse::Error {
            error: format!("{error:#}"),
        },
    };
    serde_json::to_writer(&mut stream, &wire).context("encoding the Styra response")?;
    stream
        .write_all(b"\n")
        .context("writing the Styra response")?;
    stream.flush().context("flushing the Styra response")?;
    // The ack is on its way to the client; only now is it safe to exit.
    state.shutdown_if_requested();
    Ok(())
}

fn read_request(stream: &UnixStream) -> Result<Request> {
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("reading the Styra request")?;
    if line.is_empty() {
        anyhow::bail!("client closed the socket without a request");
    }
    if line.len() > MAX_REQUEST_BYTES {
        anyhow::bail!("request exceeds the {MAX_REQUEST_BYTES}-byte limit");
    }
    serde_json::from_str(&line).context("decoding the Styra request")
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
    fn a_legacy_session_without_a_provider_id_is_viewable_but_not_resumable() {
        let store =
            std::env::temp_dir().join(format!("styra-server-legacy-test-{}", std::process::id()));
        let host = store.with_extension("host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));
        let workspace =
            crate::workspace::create(&store, &host, Some("legacy".into())).unwrap();
        let session_dir =
            crate::workspace::sessions_dir(&store, &workspace.id).join("0000000000001-1-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("journal.jsonl"), "").unwrap();
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
        let root =
            std::env::temp_dir().join(format!("styra-native-test-{}", std::process::id()));
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
