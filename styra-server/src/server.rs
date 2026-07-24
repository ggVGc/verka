//! Styra's Unix-socket server and server-owned job manager.

use crate::agent::{MountSpec, Profile, SandboxLayout};
use crate::api::{
    CreateSession, Health, Request, Response, SequencedUpdate, SessionInfo, StoredSession,
    Transcript, Updates, WireRequest, WireResponse, API_VERSION,
};
use crate::journal::{self, Journal};
use crate::job::{Job, JobSpec};
use crate::types::{DrivaOptions, JobSummary, SessionSummary};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct ServerState {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    store_root: PathBuf,
    layout: SandboxLayout,
    jobs: Mutex<HashMap<String, Arc<ManagedJob>>>,
    /// Jobs whose agent process is still running. Idle shutdown keys off this:
    /// the server stays up while any job is live, independent of whether a
    /// client is attached, so a detached agent turn is never killed.
    live_jobs: AtomicUsize,
    /// When the server last saw a job change or a client request. Combined
    /// with a zero `live_jobs`, staleness past the idle timeout ends the
    /// process.
    last_active: Mutex<Instant>,
}

impl ServerInner {
    /// Record that the server just did something worth staying alive for.
    fn touch(&self) {
        *self.last_active.lock().expect("activity lock poisoned") = Instant::now();
    }
}

struct ManagedJob {
    job: Job,
    updates: Arc<Mutex<Vec<SequencedUpdate>>>,
    accepting_messages: Arc<AtomicBool>,
    single_turn: bool,
    /// Captured at spawn so the job can be listed and reattached to without
    /// re-deriving them: the profile name, host workspace, and launch policy.
    profile: String,
    workspace: PathBuf,
    driva: DrivaOptions,
}

impl ManagedJob {
    fn summary(&self) -> JobSummary {
        JobSummary {
            id: self.job.session_id().to_owned(),
            profile: self.profile.clone(),
            workspace: self.workspace.clone(),
            driva: self.driva.clone(),
            accepting: self.accepting_messages.load(Ordering::Acquire),
        }
    }
}

impl ManagedJob {
    fn send(&self, text: &str) -> Result<()> {
        if !self.accepting_messages.load(Ordering::Acquire) {
            anyhow::bail!(
                "session {} is not accepting messages",
                self.job.session_id()
            );
        }
        self.job.send(text)?;
        if self.single_turn {
            self.accepting_messages.store(false, Ordering::Release);
        }
        Ok(())
    }

    fn stop(&self) {
        self.accepting_messages.store(false, Ordering::Release);
        self.job.stop();
    }
}

impl ServerState {
    pub fn new(store_root: PathBuf) -> Self {
        Self {
            inner: Arc::new(ServerInner {
                store_root,
                layout: SandboxLayout::default(),
                jobs: Mutex::new(HashMap::new()),
                live_jobs: AtomicUsize::new(0),
                last_active: Mutex::new(Instant::now()),
            }),
        }
    }

    pub fn store_root(&self) -> &Path {
        &self.inner.store_root
    }

    /// Record that the server just did something worth staying alive for.
    fn touch(&self) {
        self.inner.touch();
    }

    /// Start a background thread that ends the process once it has had no live
    /// jobs and no client activity for `timeout`, removing `socket` on the way
    /// out. A zero `timeout` disables idle shutdown (the server runs until
    /// killed). Mirrors the idle timeout that lets a bloop/sbt server retire
    /// itself when nothing is using it.
    pub fn spawn_idle_monitor(&self, socket: PathBuf, timeout: Duration) {
        if timeout.is_zero() {
            return;
        }
        let inner = Arc::clone(&self.inner);
        // Check several times per timeout so shutdown lands promptly, but never
        // busy-spin on a long timeout.
        let interval = (timeout / 4).clamp(Duration::from_millis(200), Duration::from_secs(30));
        std::thread::Builder::new()
            .name("styra-idle-monitor".into())
            .spawn(move || loop {
                std::thread::sleep(interval);
                if inner.live_jobs.load(Ordering::Acquire) != 0 {
                    continue;
                }
                let idle = inner
                    .last_active
                    .lock()
                    .expect("activity lock poisoned")
                    .elapsed();
                if idle >= timeout {
                    eprintln!("styra-server: no live jobs and idle for {timeout:?}; shutting down");
                    std::fs::remove_file(&socket).ok();
                    std::process::exit(0);
                }
            })
            .expect("spawning the idle monitor");
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
        let spec = JobSpec {
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
        let (job, receiver) = Job::spawn(spec, backend, journal, id.clone(), diagnostics)?;
        let updates = Arc::new(Mutex::new(Vec::new()));
        let accepting_messages = Arc::new(AtomicBool::new(true));
        let managed = Arc::new(ManagedJob {
            job,
            updates: Arc::clone(&updates),
            accepting_messages: Arc::clone(&accepting_messages),
            single_turn,
            profile: profile_name.clone(),
            workspace: workspace.clone(),
            driva: driva.clone(),
        });
        let collector_inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                while let Ok(update) = receiver.recv() {
                    if matches!(update, crate::types::JobUpdate::Ended(_)) {
                        accepting_messages.store(false, Ordering::Release);
                        // The agent process is gone: drop the live-job count
                        // and mark the moment so the idle clock starts here.
                        collector_inner.live_jobs.fetch_sub(1, Ordering::AcqRel);
                        collector_inner.touch();
                    }
                    let mut history = updates.lock().expect("job update lock poisoned");
                    let sequence = history.len() as u64 + 1;
                    history.push(SequencedUpdate { sequence, update });
                }
            })
            .context("starting the job update collector")?;
        self.inner
            .jobs
            .lock()
            .expect("server job lock poisoned")
            .insert(id.clone(), Arc::clone(&managed));
        self.inner.live_jobs.fetch_add(1, Ordering::AcqRel);
        self.touch();

        if let Some(message) = request
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            if let Err(error) = managed.send(message) {
                managed.stop();
                self.inner
                    .jobs
                    .lock()
                    .expect("server job lock poisoned")
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

    fn job(&self, id: &str) -> Result<Arc<ManagedJob>> {
        self.inner
            .jobs
            .lock()
            .expect("server job lock poisoned")
            .get(id)
            .cloned()
            .with_context(|| format!("no live job for session {id:?}"))
    }

    fn stored_summary(&self, id: &str) -> Result<SessionSummary> {
        journal::list_sessions(&self.inner.store_root)?
            .into_iter()
            .find(|session| session.id == id)
            .with_context(|| format!("stored session {id:?} was not found"))
    }

    fn handle(&self, request: Request) -> Result<Response> {
        // Any request is activity: keep the server alive while a client is
        // using it, even when it is only browsing stored sessions.
        self.touch();
        match request {
            Request::Health => Ok(Response::Health(Health {
                service: "styra".into(),
                api_version: API_VERSION.into(),
            })),
            Request::CreateSession(request) => {
                Ok(Response::SessionCreated(self.create_session(request)?))
            }
            Request::SendMessage { id, message } => {
                self.job(&id)?.send(&message.text)?;
                Ok(Response::Accepted)
            }
            Request::StopSession { id } => {
                self.job(&id)?.stop();
                Ok(Response::Accepted)
            }
            Request::Updates { id, after } => {
                let job = self.job(&id)?;
                let all = job
                    .updates
                    .lock()
                    .expect("job update lock poisoned");
                let updates = all
                    .iter()
                    .filter(|update| update.sequence > after)
                    .cloned()
                    .collect();
                let next = all.last().map(|update| update.sequence).unwrap_or(after);
                Ok(Response::Updates(Updates { updates, next }))
            }
            Request::ListJobs => {
                let jobs = self.inner.jobs.lock().expect("server job lock poisoned");
                let mut summaries: Vec<JobSummary> =
                    jobs.values().map(|managed| managed.summary()).collect();
                // Newest first: the id embeds a millisecond timestamp, so a
                // descending id sort orders jobs by creation time.
                summaries.sort_by(|a, b| b.id.cmp(&a.id));
                Ok(Response::Jobs(summaries))
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
