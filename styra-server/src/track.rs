//! One live agent track: the Driva launch, pipe plumbing, and threads behind a
//! single running agent process. A `Track` belongs to a persistent Styra
//! session (identified by `session_id`) and carries that session's events to
//! the UI while it runs.
//!
//! Driva's interface fits a live track without change. Its
//! [`ExecutionIo`] takes plain `File` handles wired to the child's stdio; where
//! Orka passes `/dev/null` for a one-shot run, Styra passes the ends of OS
//! pipes and drives a bidirectional protocol:
//!
//! - the child's stdin-read and stdout-write ends become the `ExecutionIo`;
//! - `driva::execute` runs on a worker thread and blocks for the track's life;
//! - a reader thread decodes newline-delimited events from the stdout-read end;
//! - the UI thread writes operator messages to the stdin-write end.

use crate::agent::{MountSpec, Profile};
use crate::event::{decode_line, AgentEvent};
use crate::journal::Journal;
use crate::types::{Direction, DrivaOptions, LogEntry, RawLine, TrackEnd, TrackUpdate};
use anyhow::{Context, Result};
use driva::{ExecutionIo, ExecutionRequest, Isolation, Mount, MountAccess};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, PipeWriter, Write};
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// What Styra needs to launch one track: an agent profile plus the concrete
/// workspace mount and working directory the operator selected.
pub struct TrackSpec {
    pub profile: Profile,
    pub working_directory: PathBuf,
    /// The operator's project, mounted writable as the agent workspace.
    pub workspace: MountSpec,
    /// Empty writable filesystems discarded after the run (e.g. `/root`).
    pub temporary_mounts: Vec<PathBuf>,
    /// Named Driva template(s) the operator selected, merged and resolved
    /// against the host filesystem, layered additively on top of the
    /// profile's own mounts, environment, and network policy.
    pub template: Option<ResolvedTemplate>,
    /// Hidden launcher and control mount used to keep an interactive tmux
    /// shell in the exact sandbox that runs the agent.
    pub broker: Option<SandboxBroker>,
}

pub struct SandboxBroker {
    pub executable: PathBuf,
    pub tmux: PathBuf,
    pub control: MountSpec,
    pub socket: PathBuf,
}

/// A Driva [`driva::TemplateConfig`] resolved against the host filesystem:
/// its mounts (including PATH additions) and environment translated to the
/// same vocabulary [`build_request`] uses for a profile, so the two overlay
/// with a plain extend/OR rather than a second round of policy resolution.
pub struct ResolvedTemplate {
    pub mounts: Vec<Mount>,
    pub environment: BTreeMap<OsString, OsString>,
    pub network: bool,
}

impl ResolvedTemplate {
    pub fn resolve(template: driva::TemplateConfig) -> Result<Self> {
        let mut mounts: Vec<Mount> = template
            .mounts
            .into_iter()
            .map(driva::MountConfig::resolve)
            .collect::<Result<_>>()?;
        let mut environment: BTreeMap<OsString, OsString> = template
            .environment
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();
        // Template mounts land at their host paths, so a `~` in a template's
        // environment names the *host* home, not the profile's sandbox `HOME`
        // (which Styra pins to a disposable /tmp directory).
        driva::expand_environment_home(&mut environment)?;
        driva::path_mounts(&template.paths, &mut mounts, &mut environment)?;
        Ok(Self {
            mounts,
            environment,
            network: template.network.unwrap_or(false),
        })
    }
}

/// Capture the Driva policy a [`TrackSpec`] would launch under, without
/// running it. This fills the [`DrivaOptions`] the server reports for a live
/// track, taken from the same [`ExecutionRequest`] Driva executes, so it can
/// never drift from what is actually running.
impl DrivaOptions {
    pub fn capture(spec: &TrackSpec, isolation_backend: impl Into<String>) -> Self {
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

/// A running agent track. Dropping it closes the agent's stdin, which ends most
/// protocol agents; the worker thread then observes the child exit.
pub struct Track {
    profile: Profile,
    session_id: String,
    stdin: Arc<Mutex<Option<PipeWriter>>>,
    journal: Arc<Mutex<Journal>>,
    updates: Sender<TrackUpdate>,
    /// Present when the profile speaks the stateful app-server protocol; owns
    /// the JSON-RPC handshake and turn dispatch.
    appserver: Option<Arc<crate::appserver::AppServer>>,
    exec: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<()>>,
}

impl Track {
    /// Launch the agent and start the worker and reader threads. Returns the
    /// track and the receiver the UI polls for updates.
    pub fn spawn(
        spec: TrackSpec,
        backend: Box<dyn Isolation + Send>,
        journal: Journal,
        session_id: String,
        diagnostics_path: PathBuf,
    ) -> Result<(Track, Receiver<TrackUpdate>)> {
        let broker_control = spec
            .broker
            .as_ref()
            .map(|broker| broker.control.source.clone());
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

        let _ = updates.send(TrackUpdate::Log(LogEntry::info(format!(
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
                            if stderr_updates.send(TrackUpdate::Log(entry)).is_err() {
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
                            if reader_updates.send(TrackUpdate::Raw(raw_line)).is_err() {
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
                                    if reader_updates.send(TrackUpdate::Event(event)).is_err() {
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
                        let _ = exec_updates.send(TrackUpdate::Log(LogEntry::info(format!(
                            "agent process exited with code {code}"
                        ))));
                        TrackEnd { exit_code: Some(code), error: None }
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        let _ = exec_updates.send(TrackUpdate::Log(LogEntry::error(format!(
                            "could not run the agent: {message}"
                        ))));
                        TrackEnd { exit_code: None, error: Some(message) }
                    }
                };
                if let Some(control) = broker_control {
                    std::fs::remove_dir_all(control).ok();
                }
                let _ = exec_updates.send(TrackUpdate::Ended(end));
            })
            .context("starting the execution thread")?;

        // A stateful protocol opens its handshake as soon as the process runs.
        if let Some(client) = &appserver {
            apply_appserver_actions(client.start(), &stdin, &updates);
        }

        let track = Track {
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
        Ok((track, receiver))
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
        let _ = self.updates.send(TrackUpdate::Event(AgentEvent::UserMessage {
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
        let _ = self.updates.send(TrackUpdate::Raw(RawLine {
            direction: Direction::ToAgent,
            text: String::from_utf8_lossy(&bytes)
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        }));
        let mut guard = self.stdin.lock().expect("track stdin lock poisoned");
        {
            let writer = guard
                .as_mut()
                .context("the track input is closed; the agent has stopped")?;
            writer.write_all(&bytes).context("writing to agent stdin")?;
            writer.flush().context("flushing agent stdin")?;
        }
        let _ = self.updates.send(TrackUpdate::Log(LogEntry::info(format!(
            "sent {} bytes to the agent",
            bytes.len()
        ))));
        if self.profile.single_turn {
            // A one-shot exec agent reads the prompt to end-of-input; close
            // stdin so the turn starts.
            guard.take();
            let _ = self.updates.send(TrackUpdate::Log(LogEntry::info(
                "closed input (single-turn profile); the agent is running the turn",
            )));
        }
        Ok(())
    }

    /// Close the agent's stdin, signalling end-of-input. Most protocol agents
    /// exit on stdin EOF; the worker thread then delivers [`TrackUpdate::Ended`].
    pub fn stop(&self) {
        if let Ok(mut guard) = self.stdin.lock() {
            guard.take();
        }
    }
}

impl Drop for Track {
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
/// events and diagnostics as track updates.
fn apply_appserver_actions(
    actions: Vec<crate::appserver::Action>,
    stdin: &Mutex<Option<PipeWriter>>,
    updates: &Sender<TrackUpdate>,
) {
    use crate::appserver::Action;
    for action in actions {
        match action {
            Action::Send(line) => {
                let _ = updates.send(TrackUpdate::Raw(RawLine {
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
                let _ = updates.send(TrackUpdate::Event(event));
            }
            Action::Info(message) => {
                let _ = updates.send(TrackUpdate::Log(LogEntry::info(message)));
            }
            Action::Warn(message) => {
                let _ = updates.send(TrackUpdate::Log(LogEntry::warn(message)));
            }
        }
    }
}

/// Translate a [`TrackSpec`] into a validated-shape Driva request. Mount and
/// policy translation mirrors Orka's Driva adapter.
fn build_request(spec: &TrackSpec) -> ExecutionRequest {
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
    if let Some(broker) = &spec.broker {
        mounts.push(Mount::Bind {
            source: broker.control.source.clone(),
            destination: broker.control.destination.clone(),
            access: MountAccess::ReadWrite,
        });
    }
    let mut environment: BTreeMap<OsString, OsString> = spec
        .profile
        .environment
        .iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect();
    let mut network = spec.profile.network;
    if let Some(template) = &spec.template {
        mounts.extend(template.mounts.iter().cloned());
        environment.extend(
            template
                .environment
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        network = network || template.network;
    }
    let command = if let Some(broker) = &spec.broker {
        environment.insert(
            OsString::from(crate::broker::BROKER_ENV),
            OsString::from("1"),
        );
        environment.insert(
            OsString::from(crate::broker::AGENT_COMMAND_ENV),
            OsString::from(
                serde_json::to_string(&spec.profile.command)
                    .expect("serializing a string command cannot fail"),
            ),
        );
        environment.insert(
            OsString::from(crate::broker::TMUX_ENV),
            broker.tmux.clone().into_os_string(),
        );
        environment.insert(
            OsString::from(crate::broker::TMUX_SOCKET_ENV),
            broker.socket.clone().into_os_string(),
        );
        environment.insert(
            OsString::from(crate::broker::WORKDIR_ENV),
            spec.working_directory.clone().into_os_string(),
        );
        vec![broker.executable.clone().into_os_string()]
    } else {
        spec.profile.command.iter().map(OsString::from).collect()
    };
    ExecutionRequest {
        command,
        working_directory: spec.working_directory.clone(),
        mounts,
        environment,
        network,
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
    use std::os::unix::fs::PermissionsExt;
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

    fn workspace_spec(dir: &std::path::Path) -> TrackSpec {
        // A profile with no credential mounts so request validation only needs
        // the workspace directory to exist.
        let mut profile =
            crate::agent::codex(&SandboxLayout::default(), std::path::Path::new("codex"), None, None);
        profile.mounts.clear();
        profile.network = false;
        profile.message_format = MessageFormat::CodexSubmission;
        // Keep input open across the turn so the test's explicit stop() is what
        // signals end-of-input (exercises the multi-turn-capable path).
        profile.single_turn = false;
        TrackSpec {
            profile,
            working_directory: dir.to_path_buf(),
            workspace: MountSpec {
                source: dir.to_path_buf(),
                destination: dir.to_path_buf(),
                writable: true,
            },
            temporary_mounts: Vec::new(),
            template: None,
            broker: None,
        }
    }

    /// A template names host state with `~` (the rust template's `RUSTUP_HOME`,
    /// say). Profiles pin `HOME` to a disposable sandbox directory, so the
    /// marker must resolve against the host home the template's mounts use, and
    /// the overlay must not lose the profile's own environment.
    #[test]
    fn a_template_overlays_the_profile_with_host_expanded_environment() {
        let dir = PathBuf::from("/tmp/styra/workspace");
        let mut spec = workspace_spec(&dir);
        let template = driva::TemplateConfig {
            environment: BTreeMap::from([
                ("TOOL_HOME".to_string(), "~/.tool".to_string()),
                ("LITERAL".to_string(), "verbatim".to_string()),
            ]),
            network: Some(true),
            ..Default::default()
        };
        spec.template = Some(ResolvedTemplate::resolve(template).unwrap());
        let request = build_request(&spec);
        let home = std::env::var("HOME").unwrap();

        assert_eq!(
            request.environment.get(&OsString::from("TOOL_HOME")),
            Some(&OsString::from(format!("{home}/.tool")))
        );
        assert_eq!(
            request.environment.get(&OsString::from("LITERAL")),
            Some(&OsString::from("verbatim"))
        );
        assert_eq!(
            request.environment.get(&OsString::from("HOME")),
            Some(&OsString::from("/tmp/agent-home"))
        );
        assert!(request.network);
    }

    /// A client picks an agent, model, and effort by sending one profile name;
    /// what the sandbox actually runs — and what the Driva view reports — must
    /// carry all three, or the picked model silently is not the one that runs.
    #[test]
    fn a_profile_name_carrying_a_model_and_effort_reaches_the_launched_command() {
        let root = std::env::temp_dir().join(format!("styra-track-model-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let stub = root.join("codex");
        std::fs::write(&stub, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut spec = workspace_spec(&root);
        spec.profile = Profile::builtin_on_path(
            "codex:gpt-5.6-terra/xhigh",
            &SandboxLayout::default(),
            root.as_os_str(),
        )
        .unwrap();
        spec.profile.mounts.clear();

        let command = DrivaOptions::capture(&spec, "bwrap").command;
        assert_eq!(spec.profile.name, "codex:gpt-5.6-terra/xhigh");
        assert!(
            command.contains(&r#"model="gpt-5.6-terra""#.to_string()),
            "{command:?}"
        );
        assert!(
            command.contains(&r#"model_reasoning_effort="xhigh""#.to_string()),
            "{command:?}"
        );
        assert_eq!(command.last().unwrap(), "app-server");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn broker_wraps_the_command_without_changing_the_reported_agent_policy() {
        let root = std::env::temp_dir().join(format!("styra-track-broker-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut spec = workspace_spec(&root);
        let agent_command = spec.profile.command.clone();
        spec.broker = Some(SandboxBroker {
            executable: PathBuf::from("/usr/bin/styra-server"),
            tmux: PathBuf::from("/usr/bin/tmux"),
            control: MountSpec {
                source: root.clone(),
                destination: PathBuf::from("/tmp/styra/control"),
                writable: true,
            },
            socket: PathBuf::from("/tmp/styra/control/tmux.sock"),
        });

        let displayed = DrivaOptions::capture(&spec, "bwrap");
        let request = build_request(&spec);
        assert_eq!(displayed.command, agent_command);
        assert_eq!(
            request.command,
            vec![OsString::from("/usr/bin/styra-server")]
        );
        assert_eq!(
            request
                .environment
                .get(&OsString::from(crate::broker::AGENT_COMMAND_ENV)),
            Some(&OsString::from(
                serde_json::to_string(&agent_command).unwrap()
            ))
        );
        assert!(request.mounts.iter().any(|mount| matches!(
            mount,
            Mount::Bind {
                destination,
                access: MountAccess::ReadWrite,
                ..
            } if destination == &PathBuf::from("/tmp/styra/control")
        )));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_sent_message_round_trips_through_the_agent_and_into_the_journal() {
        let dir = std::env::temp_dir().join(format!("styra-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Journal::create(&dir).unwrap();

        let (track, updates) = Track::spawn(
            workspace_spec(&dir),
            Box::new(EchoBackend),
            journal,
            "test-session".into(),
            dir.join("diagnostics.log"),
        )
        .unwrap();

        track.send("hello agent").unwrap();
        // Closing stdin lets the echo backend finish after replying.
        track.stop();

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
                Ok(TrackUpdate::Event(AgentEvent::UserMessage { text })) => user = Some(text),
                Ok(TrackUpdate::Event(AgentEvent::AgentMessage { text })) => agent = Some(text),
                Ok(TrackUpdate::Event(_)) => {}
                Ok(TrackUpdate::Raw(line)) => raw_directions.push(line.direction),
                Ok(TrackUpdate::Log(entry)) => logs.push(entry.message),
                Ok(TrackUpdate::Ended(_)) => ended = true,
                Err(_) => {}
            }
        }

        assert_eq!(user.as_deref(), Some("hello agent"));
        assert_eq!(agent.as_deref(), Some("echo: hello agent"));
        assert!(ended, "the track should report that it ended");
        // The raw view sees both the outgoing submission and the agent reply.
        assert!(raw_directions.contains(&Direction::ToAgent));
        assert!(raw_directions.contains(&Direction::FromAgent));
        // Agent stderr is streamed to the log view.
        assert!(stderr_seen(&logs), "agent stderr should reach the log");

        drop(track);

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
