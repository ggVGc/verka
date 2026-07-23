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
use crate::event::{decode_line, StyraEvent};
use crate::journal::Journal;
use anyhow::{Context, Result};
use driva::{ExecutionIo, ExecutionRequest, Isolation, Mount, MountAccess};
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

/// An update delivered from the session threads to the UI.
pub enum SessionUpdate {
    /// A decoded agent event or an operator message, in occurrence order.
    Event(StyraEvent),
    /// The agent process ended; no further events will arrive.
    Ended(SessionEnd),
}

/// How a session finished.
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
    exec: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
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

        // stdin: we write, the child reads. stdout: the child writes, we read.
        let (stdin_read, stdin_write) = std::io::pipe().context("creating agent stdin pipe")?;
        let (stdout_read, stdout_write) = std::io::pipe().context("creating agent stdout pipe")?;
        let diagnostics = File::create(&diagnostics_path).with_context(|| {
            format!("creating diagnostics file {}", diagnostics_path.display())
        })?;

        let io = ExecutionIo {
            stdin: File::from(OwnedFd::from(stdin_read)),
            stdout: File::from(OwnedFd::from(stdout_write)),
            stderr: diagnostics,
        };

        let (updates, receiver) = channel();
        let journal = Arc::new(Mutex::new(journal));

        // Reader thread: decode events from the child's stdout, journal each
        // verbatim line, and forward the decoded event to the UI.
        let reader_updates = updates.clone();
        let reader_journal = Arc::clone(&journal);
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
                            let event = decode_line(protocol, raw);
                            if reader_updates.send(SessionUpdate::Event(event)).is_err() {
                                break;
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
                    Ok(outcome) => SessionEnd {
                        exit_code: Some(outcome.exit.code()),
                        error: None,
                    },
                    Err(error) => SessionEnd {
                        exit_code: None,
                        error: Some(format!("{error:#}")),
                    },
                };
                let _ = exec_updates.send(SessionUpdate::Ended(end));
            })
            .context("starting the execution thread")?;

        let session = Session {
            profile: spec.profile,
            session_id,
            stdin: Arc::new(Mutex::new(Some(stdin_write))),
            journal,
            updates,
            exec: Some(exec),
            reader: Some(reader),
        };
        Ok((session, receiver))
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Send an operator message to the agent. It is journaled, echoed to the UI
    /// as a [`StyraEvent::UserMessage`], then written as one protocol input line.
    pub fn send(&self, text: &str) -> Result<()> {
        if let Ok(mut journal) = self.journal.lock() {
            let _ = journal.record_user_message(text);
        }
        let _ = self.updates.send(SessionUpdate::Event(StyraEvent::UserMessage {
            text: text.to_owned(),
        }));

        let bytes = self.profile.encode_message(text);
        let mut guard = self.stdin.lock().expect("session stdin lock poisoned");
        let writer = guard
            .as_mut()
            .context("the session input is closed; the agent has stopped")?;
        writer.write_all(&bytes).context("writing to agent stdin")?;
        writer.flush().context("flushing agent stdin")?;
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
        let mut ended = false;
        // `Ended` (worker thread) and the echo event (reader thread) race, so
        // drain until all are seen rather than stopping on `Ended`.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline && !(user.is_some() && agent.is_some() && ended) {
            match updates.recv_timeout(Duration::from_millis(200)) {
                Ok(SessionUpdate::Event(StyraEvent::UserMessage { text })) => user = Some(text),
                Ok(SessionUpdate::Event(StyraEvent::AgentMessage { text })) => agent = Some(text),
                Ok(SessionUpdate::Event(_)) => {}
                Ok(SessionUpdate::Ended(_)) => ended = true,
                Err(_) => {}
            }
        }

        assert_eq!(user.as_deref(), Some("hello agent"));
        assert_eq!(agent.as_deref(), Some("echo: hello agent"));
        assert!(ended, "the session should report that it ended");

        drop(session);

        // The journal captured both the operator turn and the agent reply.
        let replayed = crate::journal::replay(&dir, Protocol::CodexJsonl).unwrap();
        assert_eq!(
            replayed,
            vec![
                StyraEvent::UserMessage { text: "hello agent".into() },
                StyraEvent::AgentMessage { text: "echo: hello agent".into() },
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
}
