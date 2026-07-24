//! One live agent session: Driva launch, pipe plumbing, and the threads that
//! carry events to the UI.
//!
//! Driva's interface fits an interactive session without change. Its
//! [`ExecutionIo`] takes plain `File` handles wired to the child's stdio; where
//! Orka passes `/dev/null` for a one-shot run, Styra passes the ends of OS
//! pipes and drives a bidirectional protocol:
//!
//! - the child's stdin-read and stdout-write ends become the `ExecutionIo`;
//! - `driva::execute` runs on a worker thread and blocks for the session's life;
//! - a reader thread decodes newline-delimited events from the stdout-read end;
//! - the UI thread writes operator messages to the stdin-write end.

use crate::agent::{MountSpec, Profile};
use crate::event::{decode_line, AgentEvent};
use crate::journal::Journal;
use anyhow::{Context, Result};
use driva::{ExecutionIo, ExecutionRequest, Isolation, Mount, MountAccess};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, PipeWriter, Write};
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// What Styra needs to launch one session: an agent profile plus the concrete
/// workspace mount and working directory the operator selected.
pub struct SessionSpec {
    pub profile: Profile,
    pub working_directory: PathBuf,
    /// The operator's project, mounted writable as the agent workspace.
    pub workspace: MountSpec,
    /// Empty writable filesystems discarded after the run (e.g. `/root`).
    pub temporary_mounts: Vec<PathBuf>,
}

/// A human-facing summary of the Driva policy a session was launched with:
/// the isolation backend, the command it runs, and the mount/network policy
/// enforced around it. Captured once at spawn time from the same
/// [`ExecutionRequest`] Driva itself executes, so it can never drift from
/// what is actually running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrivaOptions {
    pub isolation_backend: String,
    pub command: Vec<String>,
    pub working_directory: PathBuf,
    pub network: bool,
    pub mounts: Vec<Mount>,
}

impl DrivaOptions {
    /// Capture the policy a `spec` would launch under, without running it.
    pub fn capture(spec: &SessionSpec, isolation_backend: impl Into<String>) -> Self {
        let request = build_request(spec);
        Self {
            isolation_backend: isolation_backend.into(),
            command: spec.profile.command.clone(),
            working_directory: request.working_directory,
            network: request.network,
            mounts: request.mounts,
        }
    }
}

/// An update delivered from the session threads to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SessionUpdate {
    /// A decoded agent event or an operator message, in occurrence order.
    Event(AgentEvent),
    /// One verbatim wire line, for the raw-interaction view.
    Raw(RawLine),
    /// A diagnostic message for the log view.
    Log(LogEntry),
    /// The agent process ended; no further events will arrive.
    Ended(SessionEnd),
}

/// Severity of a [`LogEntry`], used to colour the log view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line in the log view: a Styra-internal note or a line of agent stderr.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

impl LogEntry {
    pub fn info(message: impl Into<String>) -> Self {
        Self { level: LogLevel::Info, message: message.into() }
    }
    pub fn warn(message: impl Into<String>) -> Self {
        Self { level: LogLevel::Warn, message: message.into() }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self { level: LogLevel::Error, message: message.into() }
    }
}

/// Which way a wire line travelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// A line Styra wrote to the agent's stdin.
    ToAgent,
    /// A line received on the agent's stdout.
    FromAgent,
}

/// One verbatim line of the agent interaction, undecoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLine {
    pub direction: Direction,
    pub text: String,
}

/// How a session finished.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEnd {
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

/// A running session. Dropping it closes the agent's stdin, which ends most
/// protocol agents; the worker thread then observes the child exit.
pub struct Session {
    profile: Profile,
    session_id: String,
    stdin: Arc<Mutex<Option<PipeWriter>>>,
    journal: Arc<Mutex<Journal>>,
    updates: Sender<SessionUpdate>,
    /// Present when the profile speaks the stateful app-server protocol; owns
    /// the JSON-RPC handshake and turn dispatch.
    appserver: Option<Arc<crate::appserver::AppServer>>,
    exec: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<()>>,
}

impl Session {
    /// Launch the agent and start the worker and reader threads. Returns the
    /// session and the receiver the UI polls for updates.
    pub fn spawn(
        spec: SessionSpec,
        backend: Box<dyn Isolation + Send>,
        journal: Journal,
        session_id: String,
        diagnostics_path: PathBuf,
    ) -> Result<(Session, Receiver<SessionUpdate>)> {
        let request = build_request(&spec);
        let protocol = spec.profile.protocol;

        // stdin: we write, the child reads. stdout/stderr: the child writes,
        // we read. Streaming stderr through a pipe lets the log view show
        // agent diagnostics live instead of only persisting them to a file.
        let (stdin_read, stdin_write) = std::io::pipe().context("creating agent stdin pipe")?;
        let (stdout_read, stdout_write) = std::io::pipe().context("creating agent stdout pipe")?;
        let (stderr_read, stderr_write) = std::io::pipe().context("creating agent stderr pipe")?;

        let io = ExecutionIo {
            stdin: File::from(OwnedFd::from(stdin_read)),
            stdout: File::from(OwnedFd::from(stdout_write)),
            stderr: File::from(OwnedFd::from(stderr_write)),
        };

        let (updates, receiver) = channel();
        let journal = Arc::new(Mutex::new(journal));
        let stdin = Arc::new(Mutex::new(Some(stdin_write)));

        // A stateful protocol gets a client that owns its handshake; the
        // reader thread routes lines through it instead of plain decoding.
        let appserver = match protocol {
            crate::event::Protocol::CodexAppServer => Some(Arc::new(
                crate::appserver::AppServer::new(
                    spec.working_directory.to_string_lossy().into_owned(),
                ),
            )),
            crate::event::Protocol::CodexJsonl | crate::event::Protocol::ClaudeJsonl => None,
        };

        let _ = updates.send(SessionUpdate::Log(LogEntry::info(format!(
            "launching {} (network {})",
            spec.profile.command.join(" "),
            if spec.profile.network { "on" } else { "off" }
        ))));

        // Stderr thread: append agent diagnostics to the log file and stream
        // each line to the log view.
        let stderr_updates = updates.clone();
        let stderr = std::thread::Builder::new()
            .name("styra-stderr".into())
            .spawn(move || {
                let mut diagnostics = File::create(&diagnostics_path).ok();
                let mut lines = BufReader::new(stderr_read);
                let mut line = String::new();
                loop {
                    line.clear();
                    match lines.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            if let Some(file) = diagnostics.as_mut() {
                                let _ = file.write_all(line.as_bytes());
                                let _ = file.flush();
                            }
                            let text = line.trim_end_matches(['\r', '\n']);
                            if text.is_empty() {
                                continue;
                            }
                            let entry = LogEntry::warn(format!("agent: {text}"));
                            if stderr_updates.send(SessionUpdate::Log(entry)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("starting the stderr thread")?;

        // Reader thread: journal each verbatim line, then either decode it
        // directly (streaming protocols) or hand it to the app-server client,
        // which drives the handshake and forwards the decoded events itself.
        let reader_updates = updates.clone();
        let reader_journal = Arc::clone(&journal);
        let reader_stdin = Arc::clone(&stdin);
        let reader_client = appserver.clone();
        let reader = std::thread::Builder::new()
            .name("styra-reader".into())
            .spawn(move || {
                let mut lines = BufReader::new(stdout_read);
                let mut line = String::new();
                loop {
                    line.clear();
                    match lines.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            let raw = line.trim_end_matches(['\r', '\n']);
                            if raw.is_empty() {
                                continue;
                            }
                            if let Ok(mut journal) = reader_journal.lock() {
                                let _ = journal.record_agent_line(raw);
                            }
                            let raw_line = RawLine {
                                direction: Direction::FromAgent,
                                text: raw.to_owned(),
                            };
                            if reader_updates.send(SessionUpdate::Raw(raw_line)).is_err() {
                                break;
                            }
                            match &reader_client {
                                Some(client) => apply_appserver_actions(
                                    client.handle_line(raw),
                                    &reader_stdin,
                                    &reader_updates,
                                ),
                                None => {
                                    let event = decode_line(protocol, raw);
                                    if reader_updates.send(SessionUpdate::Event(event)).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("starting the reader thread")?;

        // Worker thread: run Driva, blocking until the child exits, then report.
        let exec_updates = updates.clone();
        let exec = std::thread::Builder::new()
            .name("styra-exec".into())
            .spawn(move || {
                let end = match driva::execute(backend.as_ref(), &request, io) {
                    Ok(outcome) => {
                        let code = outcome.exit.code();
                        let _ = exec_updates.send(SessionUpdate::Log(LogEntry::info(format!(
                            "agent process exited with code {code}"
                        ))));
                        SessionEnd { exit_code: Some(code), error: None }
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        let _ = exec_updates.send(SessionUpdate::Log(LogEntry::error(format!(
                            "could not run the agent: {message}"
                        ))));
                        SessionEnd { exit_code: None, error: Some(message) }
                    }
                };
                let _ = exec_updates.send(SessionUpdate::Ended(end));
            })
            .context("starting the execution thread")?;

        // A stateful protocol opens its handshake as soon as the process runs.
        if let Some(client) = &appserver {
            apply_appserver_actions(client.start(), &stdin, &updates);
        }

        let session = Session {
            profile: spec.profile,
            session_id,
            stdin,
            journal,
            updates,
            appserver,
            exec: Some(exec),
            reader: Some(reader),
            stderr: Some(stderr),
        };
        Ok((session, receiver))
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Send an operator message to the agent. It is journaled, echoed to the UI
    /// as a [`AgentEvent::UserMessage`], then written as one protocol input line
    /// (or dispatched as an app-server turn).
    pub fn send(&self, text: &str) -> Result<()> {
        if let Ok(mut journal) = self.journal.lock() {
            let _ = journal.record_user_message(text);
        }
        let _ = self.updates.send(SessionUpdate::Event(AgentEvent::UserMessage {
            text: text.to_owned(),
        }));

        // The app-server client owns turn dispatch (and emits its own raw
        // update for the exact wire line).
        if let Some(client) = &self.appserver {
            apply_appserver_actions(client.send(text), &self.stdin, &self.updates);
            return Ok(());
        }

        let bytes = self.profile.encode_message(text);
        // Surface the exact bytes going onto the wire in the raw view.
        let _ = self.updates.send(SessionUpdate::Raw(RawLine {
            direction: Direction::ToAgent,
            text: String::from_utf8_lossy(&bytes)
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        }));
        let mut guard = self.stdin.lock().expect("session stdin lock poisoned");
        {
            let writer = guard
                .as_mut()
                .context("the session input is closed; the agent has stopped")?;
            writer.write_all(&bytes).context("writing to agent stdin")?;
            writer.flush().context("flushing agent stdin")?;
        }
        let _ = self.updates.send(SessionUpdate::Log(LogEntry::info(format!(
            "sent {} bytes to the agent",
            bytes.len()
        ))));
        if self.profile.single_turn {
            // A one-shot exec agent reads the prompt to end-of-input; close
            // stdin so the turn starts.
            guard.take();
            let _ = self.updates.send(SessionUpdate::Log(LogEntry::info(
                "closed input (single-turn profile); the agent is running the turn",
            )));
        }
        Ok(())
    }

    /// Close the agent's stdin, signalling end-of-input. Most protocol agents
    /// exit on stdin EOF; the worker thread then delivers [`SessionUpdate::Ended`].
    pub fn stop(&self) {
        if let Ok(mut guard) = self.stdin.lock() {
            guard.take();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.exec.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr.take() {
            let _ = handle.join();
        }
    }
}

/// Carry out the actions the pure app-server client asked for: write outgoing
/// lines to the agent's stdin (surfacing them in the raw view), and forward
/// events and diagnostics as session updates.
fn apply_appserver_actions(
    actions: Vec<crate::appserver::Action>,
    stdin: &Mutex<Option<PipeWriter>>,
    updates: &Sender<SessionUpdate>,
) {
    use crate::appserver::Action;
    for action in actions {
        match action {
            Action::Send(line) => {
                let _ = updates.send(SessionUpdate::Raw(RawLine {
                    direction: Direction::ToAgent,
                    text: line.clone(),
                }));
                if let Ok(mut guard) = stdin.lock() {
                    if let Some(writer) = guard.as_mut() {
                        let _ = writer.write_all(line.as_bytes());
                        let _ = writer.write_all(b"\n");
                        let _ = writer.flush();
                    }
                }
            }
            Action::Event(event) => {
                let _ = updates.send(SessionUpdate::Event(event));
            }
            Action::Info(message) => {
                let _ = updates.send(SessionUpdate::Log(LogEntry::info(message)));
            }
            Action::Warn(message) => {
                let _ = updates.send(SessionUpdate::Log(LogEntry::warn(message)));
            }
        }
    }
}

/// Translate a [`SessionSpec`] into a validated-shape Driva request. Mount and
/// policy translation mirrors Orka's Driva adapter.
fn build_request(spec: &SessionSpec) -> ExecutionRequest {
    let mut mounts: Vec<Mount> = spec
        .temporary_mounts
        .iter()
        .cloned()
        .map(|destination| Mount::Temporary { destination })
        .collect();
    for mount in std::iter::once(&spec.workspace).chain(spec.profile.mounts.iter()) {
        mounts.push(Mount::Bind {
            source: mount.source.clone(),
            destination: mount.destination.clone(),
            access: if mount.writable {
                MountAccess::ReadWrite
            } else {
                MountAccess::ReadOnly
            },
        });
    }
    ExecutionRequest {
        command: spec.profile.command.iter().map(OsString::from).collect(),
        working_directory: spec.working_directory.clone(),
        mounts,
        environment: spec
            .profile
            .environment
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect(),
        network: spec.profile.network,
        interactive: false,
        new_session: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{MessageFormat, SandboxLayout};
    use crate::event::Protocol;
    use driva::{ExecutionOutcome, ProcessExit};
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime};

    /// A backend that speaks a tiny protocol: for each submission line it reads
    /// on stdin, it writes back one codex agent_message echoing the text, then
    /// exits on stdin EOF. This exercises the full bidirectional path without a
    /// real agent, the way Orka tests its executor with a stub.
    struct EchoBackend;

    impl Isolation for EchoBackend {
        fn run(&self, request: &ExecutionRequest, mut io: ExecutionIo) -> Result<ExecutionOutcome> {
            writeln!(io.stderr, "echo backend online").ok();
            io.stderr.flush().ok();
            let reader = BufReader::new(io.stdin);
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let submission: serde_json::Value = serde_json::from_str(&line)?;
                let text = submission["op"]["items"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                let event = serde_json::json!({
                    "type": "item.completed",
                    "item": { "type": "agent_message", "text": format!("echo: {text}") },
                });
                writeln!(io.stdout, "{event}")?;
                io.stdout.flush()?;
            }
            let now = SystemTime::now();
            Ok(ExecutionOutcome {
                exit: ProcessExit::Code(0),
                evidence: driva::ExecutionEvidence {
                    isolation_backend: "echo".into(),
                    effective_policy: driva::effective_policy(request),
                    started_at: now,
                    finished_at: now,
                },
            })
        }
    }

    fn workspace_spec(dir: &std::path::Path) -> SessionSpec {
        // A profile with no credential mounts so request validation only needs
        // the workspace directory to exist.
        let mut profile = crate::agent::codex(&SandboxLayout::default());
        profile.mounts.clear();
        profile.network = false;
        profile.message_format = MessageFormat::CodexSubmission;
        // Keep input open across the turn so the test's explicit stop() is what
        // signals end-of-input (exercises the multi-turn-capable path).
        profile.single_turn = false;
        SessionSpec {
            profile,
            working_directory: dir.to_path_buf(),
            workspace: MountSpec {
                source: dir.to_path_buf(),
                destination: dir.to_path_buf(),
                writable: true,
            },
            temporary_mounts: Vec::new(),
        }
    }

    #[test]
    fn a_sent_message_round_trips_through_the_agent_and_into_the_journal() {
        let dir = std::env::temp_dir().join(format!("styra-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Journal::create(&dir).unwrap();

        let (session, updates) = Session::spawn(
            workspace_spec(&dir),
            Box::new(EchoBackend),
            journal,
            "test-session".into(),
            dir.join("diagnostics.log"),
        )
        .unwrap();

        session.send("hello agent").unwrap();
        // Closing stdin lets the echo backend finish after replying.
        session.stop();

        let mut user = None;
        let mut agent = None;
        let mut raw_directions = Vec::new();
        let mut logs: Vec<String> = Vec::new();
        let mut ended = false;
        // `Ended` (worker thread) and the echo event (reader thread) race, so
        // drain until all are seen rather than stopping on `Ended`.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let stderr_seen = |logs: &[String]| logs.iter().any(|l| l.contains("echo backend online"));
        while std::time::Instant::now() < deadline
            && !(user.is_some()
                && agent.is_some()
                && ended
                && raw_directions.len() >= 2
                && stderr_seen(&logs))
        {
            match updates.recv_timeout(Duration::from_millis(200)) {
                Ok(SessionUpdate::Event(AgentEvent::UserMessage { text })) => user = Some(text),
                Ok(SessionUpdate::Event(AgentEvent::AgentMessage { text })) => agent = Some(text),
                Ok(SessionUpdate::Event(_)) => {}
                Ok(SessionUpdate::Raw(line)) => raw_directions.push(line.direction),
                Ok(SessionUpdate::Log(entry)) => logs.push(entry.message),
                Ok(SessionUpdate::Ended(_)) => ended = true,
                Err(_) => {}
            }
        }

        assert_eq!(user.as_deref(), Some("hello agent"));
        assert_eq!(agent.as_deref(), Some("echo: hello agent"));
        assert!(ended, "the session should report that it ended");
        // The raw view sees both the outgoing submission and the agent reply.
        assert!(raw_directions.contains(&Direction::ToAgent));
        assert!(raw_directions.contains(&Direction::FromAgent));
        // Agent stderr is streamed to the log view.
        assert!(stderr_seen(&logs), "agent stderr should reach the log");

        drop(session);

        // The journal captured both the operator turn and the agent reply.
        let replayed = crate::journal::replay(&dir, Protocol::CodexJsonl).unwrap();
        assert_eq!(
            replayed,
            vec![
                AgentEvent::UserMessage { text: "hello agent".into() },
                AgentEvent::AgentMessage { text: "echo: hello agent".into() },
            ]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_request_translates_mounts_and_policy() {
        let dir = PathBuf::from("/tmp/styra/workspace");
        let mut spec = workspace_spec(&dir);
        spec.temporary_mounts = vec![PathBuf::from("/root")];
        spec.profile.environment = BTreeMap::from([("HOME".into(), "/root".into())]);
        let request = build_request(&spec);

        assert!(!request.network);
        assert!(request.new_session);
        assert!(matches!(request.mounts[0], Mount::Temporary { .. }));
        assert!(request.mounts.iter().any(|mount| matches!(
            mount,
            Mount::Bind { access: MountAccess::ReadWrite, .. }
        )));
        assert_eq!(
            request.environment.get(&OsString::from("HOME")),
            Some(&OsString::from("/root"))
        );
    }

    #[test]
    fn driva_options_capture_the_backend_command_and_effective_mounts() {
        let dir = PathBuf::from("/tmp/styra/workspace");
        let spec = workspace_spec(&dir);
        let command = spec.profile.command.clone();
        let options = DrivaOptions::capture(&spec, "bwrap");

        assert_eq!(options.isolation_backend, "bwrap");
        assert_eq!(options.command, command);
        assert_eq!(options.working_directory, dir);
        assert!(!options.network);
        assert!(options.mounts.iter().any(|mount| matches!(
            mount,
            Mount::Bind { destination, .. } if destination == &dir
        )));
    }
}
