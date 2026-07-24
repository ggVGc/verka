//! Styra's Unix-socket server and server-owned session manager.

use crate::agent::{MountSpec, Profile, SandboxLayout};
use crate::api::{
    CreateSession, Health, Request, Response, SequencedUpdate, SessionInfo, StoredSession,
    Transcript, Updates, WireRequest, WireResponse, API_VERSION,
};
use crate::journal::{self, Journal};
use crate::session::{Session, SessionSpec};
use crate::types::{DrivaOptions, SessionSummary};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct ServerState {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    store_root: PathBuf,
    layout: SandboxLayout,
    sessions: Mutex<HashMap<String, Arc<ManagedSession>>>,
}

struct ManagedSession {
    session: Session,
    updates: Arc<Mutex<Vec<SequencedUpdate>>>,
    accepting_messages: Arc<AtomicBool>,
    single_turn: bool,
}

impl ManagedSession {
    fn send(&self, text: &str) -> Result<()> {
        if !self.accepting_messages.load(Ordering::Acquire) {
            anyhow::bail!(
                "session {} is not accepting messages",
                self.session.session_id()
            );
        }
        self.session.send(text)?;
        if self.single_turn {
            self.accepting_messages.store(false, Ordering::Release);
        }
        Ok(())
    }

    fn stop(&self) {
        self.accepting_messages.store(false, Ordering::Release);
        self.session.stop();
    }
}

impl ServerState {
    pub fn new(store_root: PathBuf) -> Self {
        Self {
            inner: Arc::new(ServerInner {
                store_root,
                layout: SandboxLayout::default(),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn store_root(&self) -> &Path {
        &self.inner.store_root
    }

    fn create_session(&self, request: CreateSession) -> Result<SessionInfo> {
        let workspace = request.workspace.canonicalize().with_context(|| {
            format!(
                "workspace directory {} must exist",
                request.workspace.display()
            )
        })?;
        let mut profile = Profile::builtin(&request.profile, &self.inner.layout)?;
        profile.network = profile.network || request.network;
        let (journal, id) = Journal::create_in_store(&self.inner.store_root, &profile)?;
        let journal_path = journal.path().to_path_buf();
        let diagnostics = journal_path
            .parent()
            .unwrap_or(&self.inner.store_root)
            .join("diagnostics.log");
        let spec = SessionSpec {
            profile,
            working_directory: self.inner.layout.workspace.clone(),
            workspace: MountSpec {
                source: workspace.clone(),
                destination: self.inner.layout.workspace.clone(),
                writable: true,
            },
            temporary_mounts: Vec::new(),
        };
        let single_turn = spec.profile.single_turn;
        let driva = DrivaOptions::capture(&spec, "bwrap");
        let profile_name = spec.profile.name.clone();
        let backend = Box::new(driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: Some(PathBuf::from("/")),
        });
        let (session, receiver) = Session::spawn(spec, backend, journal, id.clone(), diagnostics)?;
        let updates = Arc::new(Mutex::new(Vec::new()));
        let accepting_messages = Arc::new(AtomicBool::new(true));
        let managed = Arc::new(ManagedSession {
            session,
            updates: Arc::clone(&updates),
            accepting_messages: Arc::clone(&accepting_messages),
            single_turn,
        });
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                while let Ok(update) = receiver.recv() {
                    if matches!(update, crate::types::SessionUpdate::Ended(_)) {
                        accepting_messages.store(false, Ordering::Release);
                    }
                    let mut history = updates.lock().expect("session update lock poisoned");
                    let sequence = history.len() as u64 + 1;
                    history.push(SequencedUpdate { sequence, update });
                }
            })
            .context("starting the session update collector")?;
        self.inner
            .sessions
            .lock()
            .expect("server session lock poisoned")
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
                    .sessions
                    .lock()
                    .expect("server session lock poisoned")
                    .remove(&id);
                return Err(error);
            }
        }

        Ok(SessionInfo {
            id,
            profile: profile_name,
            workspace,
            journal_path,
            driva,
        })
    }

    fn session(&self, id: &str) -> Result<Arc<ManagedSession>> {
        self.inner
            .sessions
            .lock()
            .expect("server session lock poisoned")
            .get(id)
            .cloned()
            .with_context(|| format!("live session {id:?} was not found"))
    }

    fn stored_summary(&self, id: &str) -> Result<SessionSummary> {
        journal::list_sessions(&self.inner.store_root)?
            .into_iter()
            .find(|session| session.id == id)
            .with_context(|| format!("stored session {id:?} was not found"))
    }

    fn handle(&self, request: Request) -> Result<Response> {
        match request {
            Request::Health => Ok(Response::Health(Health {
                service: "styra".into(),
                api_version: API_VERSION.into(),
            })),
            Request::CreateSession(request) => {
                Ok(Response::SessionCreated(self.create_session(request)?))
            }
            Request::SendMessage { id, message } => {
                self.session(&id)?.send(&message.text)?;
                Ok(Response::Accepted)
            }
            Request::StopSession { id } => {
                self.session(&id)?.stop();
                Ok(Response::Accepted)
            }
            Request::Updates { id, after } => {
                let session = self.session(&id)?;
                let all = session
                    .updates
                    .lock()
                    .expect("session update lock poisoned");
                let updates = all
                    .iter()
                    .filter(|update| update.sequence > after)
                    .cloned()
                    .collect();
                let next = all.last().map(|update| update.sequence).unwrap_or(after);
                Ok(Response::Updates(Updates { updates, next }))
            }
            Request::ListStoredSessions => Ok(Response::StoredSessions(journal::list_sessions(
                self.store_root(),
            )?)),
            Request::StoredSession { id } => {
                let summary = self.stored_summary(&id)?;
                let meta = journal::read_session_meta(&summary.path)?
                    .with_context(|| format!("session {id:?} has no session.json"))?;
                let events = journal::replay(&summary.path, meta.protocol)?;
                let raw = journal::replay_raw(&summary.path)?;
                Ok(Response::StoredSession(StoredSession {
                    summary,
                    events,
                    raw,
                }))
            }
            Request::Transcript { id } => {
                let summary = self.stored_summary(&id)?;
                let meta = journal::read_session_meta(&summary.path)?
                    .with_context(|| format!("session {id:?} has no session.json"))?;
                let text = journal::render_transcript(&summary.path, meta.protocol)?;
                Ok(Response::Transcript(Transcript { text }))
            }
        }
    }
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
    stream.flush().context("flushing the Styra response")
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
    let wire: WireRequest = serde_json::from_str(&line).context("decoding the Styra request")?;
    if wire.api_version != API_VERSION {
        anyhow::bail!(
            "unsupported API version {:?}; server supports {:?}",
            wire.api_version,
            API_VERSION
        );
    }
    Ok(wire.request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "styra-server-{tag}-{}-{}.sock",
            std::process::id(),
            crate::journal::sessions_dir(Path::new(""))
                .components()
                .count()
        ))
    }

    #[test]
    fn socket_protocol_reports_version() {
        let socket = temp_path("health");
        std::fs::remove_file(&socket).ok();
        let listener = UnixListener::bind(&socket).unwrap();
        let store = socket.with_extension("store");
        let state = ServerState::new(store.clone());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, &state).unwrap();
        });

        let health = Client::new(&socket).health().unwrap();
        assert_eq!(health.service, "styra");
        assert_eq!(health.api_version, API_VERSION);

        server.join().unwrap();
        std::fs::remove_file(socket).ok();
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn stored_ids_are_resolved_from_the_store_listing() {
        let store =
            std::env::temp_dir().join(format!("styra-server-id-test-{}", std::process::id()));
        std::fs::remove_dir_all(&store).ok();
        let state = ServerState::new(store.clone());
        let error = state.stored_summary("../../etc").unwrap_err();
        assert!(error.to_string().contains("was not found"));
        std::fs::remove_dir_all(store).ok();
    }
}
