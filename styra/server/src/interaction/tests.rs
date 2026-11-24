use super::*;
use crate::agent::{MessageFormat, SandboxLayout};
use crate::event::Protocol;
use driva::{ExecutionOutcome, ProcessExit};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Barrier;
use std::time::{Duration, SystemTime};

/// The policy a spec would launch under, through the same Bubblewrap
/// backend the server builds for it — which is what knows the sandbox's
/// floor and its environment.
fn capture(spec: &InteractionSpec) -> Result<DrivaOptions> {
    capture_driva_options(
        spec,
        &driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: None,
            base: spec.base.clone(),
        },
    )
}

#[test]
fn reported_cwd_is_translated_only_when_it_stays_in_the_workspace() {
    let sandbox = Path::new("/sandbox/project");
    let host = Path::new("/home/operator/project");

    assert_eq!(
        host_working_directory("/sandbox/project/crates/ui", sandbox, host),
        Some(PathBuf::from("/home/operator/project/crates/ui"))
    );
    assert_eq!(
        host_working_directory("/sandbox/project", sandbox, host),
        Some(PathBuf::from("/home/operator/project"))
    );
    assert_eq!(host_working_directory("relative", sandbox, host), None);
    assert_eq!(
        host_working_directory("/sandbox/project/../secrets", sandbox, host),
        None
    );
    assert_eq!(host_working_directory("/etc", sandbox, host), None);
}

/// A backend that speaks a tiny protocol: for each submission line it reads
/// on stdin, it writes back one Claude assistant message echoing the text, then
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
            let text = submission["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let event = serde_json::json!({
                "type": "assistant",
                "message": { "role": "assistant", "content": format!("echo: {text}") },
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

/// Emits one complete agent message and exits as soon as the test releases
/// it. The test can then hold the journal lock to keep the stdout reader
/// from publishing that already-buffered message while the execution
/// thread publishes the process ending.
struct BufferedExitBackend {
    release: Arc<Barrier>,
}

impl Isolation for BufferedExitBackend {
    fn run(&self, request: &ExecutionRequest, mut io: ExecutionIo) -> Result<ExecutionOutcome> {
        self.release.wait();
        let event = serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": "buffered reply" },
        });
        writeln!(io.stdout, "{event}")?;
        io.stdout.flush()?;
        let now = SystemTime::now();
        Ok(ExecutionOutcome {
            exit: ProcessExit::Code(0),
            evidence: driva::ExecutionEvidence {
                isolation_backend: "buffered-exit-test".into(),
                effective_policy: driva::effective_policy(request),
                started_at: now,
                finished_at: now,
            },
        })
    }
}

/// Ignores stdin EOF and exits only when the execution owner asks it to be
/// terminated, modelling a provider that does not shut down gracefully.
struct StubbornBackend {
    terminated: Arc<AtomicBool>,
}

impl Isolation for StubbornBackend {
    fn run(&self, _request: &ExecutionRequest, _io: ExecutionIo) -> Result<ExecutionOutcome> {
        unreachable!("Styra runs interactions with execution control")
    }

    fn run_controlled(
        &self,
        request: &ExecutionRequest,
        _io: ExecutionIo,
        control: &ExecutionControl,
    ) -> Result<ExecutionOutcome> {
        while !control.termination_requested() {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.terminated.store(true, Ordering::Release);
        let now = SystemTime::now();
        Ok(ExecutionOutcome {
            exit: ProcessExit::Signaled,
            evidence: driva::ExecutionEvidence {
                isolation_backend: "stubborn-test".into(),
                effective_policy: driva::effective_policy(request),
                started_at: now,
                finished_at: now,
            },
        })
    }
}

fn workspace_spec(dir: &std::path::Path) -> InteractionSpec {
    // A profile with no credential mounts so request validation only needs
    // the workspace directory to exist.
    let mut profile = crate::agent::claude(
        &SandboxLayout::default(),
        std::path::Path::new("claude"),
        crate::agent::Provider::Claude.default_model(),
        crate::agent::Provider::Claude.default_effort(),
    );
    profile.mounts.clear();
    profile.network = false;
    profile.message_format = MessageFormat::ClaudeStreamJson;
    InteractionSpec {
        profile,
        resume_provider_session_id: None,
        working_directory: dir.to_path_buf(),
        workspace: MountSpec {
            source: dir.to_path_buf(),
            destination: dir.to_path_buf(),
            writable: true,
        },
        repository_mounts: Vec::new(),
        automatic_mounts: Vec::new(),
        tooling_mounts: Vec::new(),
        base: driva::BaseConfig::default(),
        temporary_mounts: Vec::new(),
        extra_mounts: Vec::new(),
        user_environment: BTreeMap::new(),
        template: None,
        broker: None,
    }
}

struct InteractionFixture(tempfile::TempDir);

impl InteractionFixture {
    fn new(name: &str) -> Self {
        Self(tempfile::Builder::new().prefix(name).tempdir().unwrap())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn spawn(
        &self,
        backend: impl Isolation + Send + 'static,
        session_id: &str,
    ) -> (Interaction, Receiver<InteractionUpdate>) {
        Interaction::spawn(
            workspace_spec(self.path()),
            Box::new(backend),
            Journal::create(self.path()).unwrap(),
            session_id.into(),
            self.path().join("diagnostics.log"),
        )
        .unwrap()
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

/// A client selection must reach the internal profile's command unchanged;
/// otherwise the picked model is not the one the sandbox runs.
#[test]
fn a_selection_carrying_a_model_and_effort_reaches_the_launched_command() {
    let root = std::env::temp_dir().join(format!("styra-interaction-model-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let stub = root.join("codex");
    std::fs::write(&stub, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut spec = workspace_spec(&root);
    spec.profile = crate::agent::Selection {
        provider: crate::agent::Provider::Codex,
        model: "gpt-5.6-terra".into(),
        effort: crate::agent::Effort::XHigh,
    }
    .resolve_on_path(&SandboxLayout::default(), root.as_os_str())
    .unwrap();
    spec.profile.mounts.clear();

    let command = capture(&spec).unwrap().command;
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
    let root =
        std::env::temp_dir().join(format!("styra-interaction-broker-{}", std::process::id()));
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

    let displayed = capture(&spec).unwrap();
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
    let fixture = InteractionFixture::new("styra-session");
    let (interaction, updates) = fixture.spawn(EchoBackend, "test-session");

    interaction.send("hello agent").unwrap();
    // Closing stdin lets the echo backend finish after replying.
    interaction.stop();

    let mut user = None;
    let mut agent = None;
    let mut raw_directions = Vec::new();
    let mut logs: Vec<String> = Vec::new();
    let mut ended = false;
    // Collect every part of the exchange, including its terminal update.
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
            Ok(InteractionUpdate::Event(AgentEvent::UserMessage { text })) => user = Some(text),
            Ok(InteractionUpdate::Event(AgentEvent::AgentMessage { text })) => agent = Some(text),
            Ok(InteractionUpdate::Event(_)) => {}
            Ok(InteractionUpdate::Raw(line)) => raw_directions.push(line.direction),
            Ok(InteractionUpdate::Log(entry)) => logs.push(entry.message),
            Ok(InteractionUpdate::WorkingDirectoryChanged(_)) => {}
            // Never sent on this channel: quota readings are derived from
            // the raw lines by the server's update collector, not by the
            // interaction's own threads.
            Ok(InteractionUpdate::Quota(_)) => {}
            Ok(InteractionUpdate::Ended(_)) => ended = true,
            Err(_) => {}
        }
    }

    assert_eq!(user.as_deref(), Some("hello agent"));
    assert_eq!(agent.as_deref(), Some("echo: hello agent"));
    assert!(ended, "the interaction should report that it ended");
    // The raw view sees both the outgoing submission and the agent reply.
    assert!(raw_directions.contains(&Direction::ToAgent));
    assert!(raw_directions.contains(&Direction::FromAgent));
    // Agent stderr is streamed to the log view.
    assert!(stderr_seen(&logs), "agent stderr should reach the log");

    drop(interaction);

    // The journal captured both the operator turn and the agent reply.
    let replayed = crate::journal::replay(fixture.path(), Protocol::ClaudeJsonl).unwrap();
    assert_eq!(
        replayed,
        vec![
            AgentEvent::UserMessage {
                text: "hello agent".into()
            },
            AgentEvent::AgentMessage {
                text: "echo: hello agent".into()
            },
        ]
    );
}

/// Regression test for an interaction being marked ended while its stdout
/// reader still has agent data to publish.
#[test]
fn ended_is_the_final_update_after_buffered_stdout_is_drained() {
    let fixture = InteractionFixture::new("styra-buffered-exit-session");
    let release = Arc::new(Barrier::new(2));

    let (interaction, updates) = fixture.spawn(
        BufferedExitBackend {
            release: Arc::clone(&release),
        },
        "buffered-exit-session",
    );

    // The reader records a line before publishing its Raw and Event
    // updates. Holding this lock makes the process-exit thread win the
    // race deterministically even though the reply is already in stdout.
    let journal_guard = interaction.journal.lock().unwrap();
    release.wait();

    let blocked_deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut order = Vec::new();
    while std::time::Instant::now() < blocked_deadline && !order.contains(&"ended") {
        match updates.recv_timeout(Duration::from_millis(200)) {
            Ok(InteractionUpdate::Ended(_)) => order.push("ended"),
            Ok(InteractionUpdate::Event(AgentEvent::AgentMessage { .. })) => {
                order.push("agent message")
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    drop(journal_guard);
    let drain_deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < drain_deadline
        && !(order.contains(&"agent message") && order.contains(&"ended"))
    {
        match updates.recv_timeout(Duration::from_millis(200)) {
            Ok(InteractionUpdate::Ended(_)) => order.push("ended"),
            Ok(InteractionUpdate::Event(AgentEvent::AgentMessage { .. })) => {
                order.push("agent message")
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }

    drop(interaction);

    assert_eq!(
        order,
        vec!["agent message", "ended"],
        "Ended must be terminal; observed updates in this order: {order:?}"
    );
}

#[test]
fn dropping_an_interaction_force_terminates_a_backend_that_ignores_eof() {
    let fixture = InteractionFixture::new("styra-stubborn-session");
    let terminated = Arc::new(AtomicBool::new(false));

    let (interaction, _updates) = fixture.spawn(
        StubbornBackend {
            terminated: Arc::clone(&terminated),
        },
        "stubborn-session",
    );

    let started = Instant::now();
    drop(interaction);
    assert!(terminated.load(Ordering::Acquire));
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// A resume launched straight after a stop reuses the stopped sandbox's
/// broker path, so it needs the sandbox actually gone — not just asked to
/// go, which is all `stop` does. `shut_down` waits for that while the
/// interaction is still alive to be replaced, terminating if it must.
#[test]
fn shutting_down_waits_for_the_sandbox_to_be_gone() {
    let fixture = InteractionFixture::new("styra-shut-down-session");

    let (polite, _updates) = fixture.spawn(EchoBackend, "polite-session");
    assert!(polite.shut_down());
    assert!(polite.exec.as_ref().unwrap().is_finished());

    let terminated = Arc::new(AtomicBool::new(false));
    let (stubborn, _updates) = fixture.spawn(
        StubbornBackend {
            terminated: Arc::clone(&terminated),
        },
        "stubborn-session",
    );
    assert!(stubborn.shut_down());
    assert!(terminated.load(Ordering::Acquire));
    assert!(stubborn.exec.as_ref().unwrap().is_finished());
}

/// An agent with background work ignores stdin EOF and works on, so
/// stopping one has to end in termination, and completing a Session kills
/// it outright rather than asking first.
#[test]
fn stopping_and_killing_end_an_agent_that_ignores_eof() {
    let fixture = InteractionFixture::new("styra-kill-session");
    let spawn_stubborn = |terminated: &Arc<AtomicBool>| {
        fixture.spawn(
            StubbornBackend {
                terminated: Arc::clone(terminated),
            },
            "stubborn-session",
        )
    };

    let terminated = Arc::new(AtomicBool::new(false));
    let (stopped, _updates) = spawn_stubborn(&terminated);
    stopped.stop_or_terminate();
    assert!(!terminated.load(Ordering::Acquire));
    assert!(wait_finished(
        stopped.exec.as_ref().unwrap(),
        SHUTDOWN_GRACE * 4
    ));
    assert!(terminated.load(Ordering::Acquire));

    let terminated = Arc::new(AtomicBool::new(false));
    let (killed, _updates) = spawn_stubborn(&terminated);
    assert!(killed.kill());
    assert!(terminated.load(Ordering::Acquire));
    assert!(killed.exec.as_ref().unwrap().is_finished());
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
        Mount::Bind {
            access: MountAccess::ReadWrite,
            ..
        }
    )));
    assert_eq!(
        request.environment.get(&OsString::from("HOME")),
        Some(&OsString::from("/root"))
    );
}

/// An operator-added mount is enforced, not merely displayed: it has to
/// reach the `ExecutionRequest` Driva runs, carrying the access it was
/// asked for.
#[test]
fn build_request_binds_the_operators_extra_mounts_with_their_access() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.extra_mounts = vec![
        MountSpec {
            source: PathBuf::from("/srv/data"),
            destination: PathBuf::from("/srv/data"),
            writable: false,
        },
        MountSpec {
            source: PathBuf::from("/srv/out"),
            destination: PathBuf::from("/mnt/out"),
            writable: true,
        },
    ];
    let request = build_request(&spec);

    assert!(request.mounts.iter().any(|mount| matches!(
        mount,
        Mount::Bind { source, destination, access: MountAccess::ReadOnly }
            if source == std::path::Path::new("/srv/data")
                && destination == std::path::Path::new("/srv/data")
    )));
    assert!(request.mounts.iter().any(|mount| matches!(
        mount,
        Mount::Bind { source, destination, access: MountAccess::ReadWrite }
            if source == std::path::Path::new("/srv/out")
                && destination == std::path::Path::new("/mnt/out")
    )));
}

#[test]
fn repository_mounts_are_enforced_and_attributed_separately() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.repository_mounts = vec![
        MountSpec {
            source: PathBuf::from("/host/repository"),
            destination: PathBuf::from("/host/repository"),
            writable: false,
        },
        MountSpec {
            source: PathBuf::from("/host/repository/.git"),
            destination: PathBuf::from("/host/repository/.git"),
            writable: true,
        },
    ];

    let options = capture(&spec).unwrap();
    assert!(options.mounts.iter().any(|mount| matches!(
        mount,
        AttributedMount {
            origin: MountOrigin::GitRepository,
            mount: Mount::Bind { destination, access: MountAccess::ReadOnly, .. },
        } if destination == std::path::Path::new("/host/repository")
    )));
    assert!(options.mounts.iter().any(|mount| matches!(
        mount,
        AttributedMount {
            origin: MountOrigin::GitRepository,
            mount: Mount::Bind { destination, access: MountAccess::ReadWrite, .. },
        } if destination == std::path::Path::new("/host/repository/.git")
    )));
}

/// A Workspace that is itself a checkout root — or a linked worktree — is
/// named twice: once as the Workspace and once as the repository. The two
/// requests are the same mount, so the effective list holds it once, with
/// the Workspace's write access.
#[test]
fn a_repository_mount_repeating_the_workspace_is_bound_once() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.repository_mounts = vec![MountSpec {
        source: dir.clone(),
        destination: dir.clone(),
        writable: false,
    }];

    let options = capture(&spec).unwrap();
    let at_workspace: Vec<_> = options
        .mounts
        .iter()
        .filter(|attributed| attributed.mount.destination() == dir)
        .collect();
    assert!(matches!(
        at_workspace.as_slice(),
        [AttributedMount {
            origin: MountOrigin::Workspace,
            mount: Mount::Bind {
                access: MountAccess::ReadWrite,
                ..
            },
        }]
    ));
}

/// The Git common directory reaches an interaction from both the
/// repository mounts and, with worktrees enabled, the automatic ones.
#[test]
fn an_automatic_mount_repeating_a_repository_mount_is_bound_once() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let common = PathBuf::from("/tmp/styra/workspace/.git");
    let mut spec = workspace_spec(&dir);
    spec.repository_mounts = vec![MountSpec {
        source: common.clone(),
        destination: common.clone(),
        writable: true,
    }];
    spec.automatic_mounts = vec![MountSpec {
        source: common.clone(),
        destination: common.clone(),
        writable: true,
    }];

    let options = capture(&spec).unwrap();
    assert_eq!(
        options
            .mounts
            .iter()
            .filter(|attributed| attributed.mount.destination() == common)
            .count(),
        1
    );
}

/// Coalescing is only for a repeat of the same mount. A different source
/// at the same destination stays in the list, so the policy check still
/// reports it.
#[test]
fn a_different_source_at_the_same_destination_is_kept_as_a_conflict() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.extra_mounts = vec![MountSpec {
        source: PathBuf::from("/srv/elsewhere"),
        destination: dir.clone(),
        writable: false,
    }];

    let options = capture(&spec).unwrap();
    assert_eq!(
        options
            .mounts
            .iter()
            .filter(|attributed| attributed.mount.destination() == dir)
            .count(),
        2
    );
}

#[test]
fn driva_options_capture_the_backend_command_and_effective_mounts() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let spec = workspace_spec(&dir);
    let command = spec.profile.command.clone();
    let options = capture(&spec).unwrap();

    assert_eq!(options.isolation_backend, "bwrap");
    assert_eq!(options.command, command);
    assert_eq!(options.working_directory, dir);
    assert!(!options.network);
    assert!(options.mounts.iter().any(|attributed| matches!(
        attributed,
        AttributedMount {
            origin: MountOrigin::Workspace,
            mount: Mount::Bind { destination, .. },
        } if destination == &dir
    )));
}

/// The agent binary is in the sandbox because the launch mounted it, and
/// it says so: with no host root behind the policy, a tool the private
/// root does not carry is a grant like any other and is attributed to its
/// own layer rather than arriving unannounced.
#[test]
fn a_host_tool_is_a_read_only_mount_of_its_own() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.tooling_mounts = vec![MountSpec {
        source: PathBuf::from("/home/operator/.local/bin/claude"),
        destination: PathBuf::from("/home/operator/.local/bin/claude"),
        writable: false,
    }];

    let options = capture(&spec).unwrap();

    assert!(options.mounts.iter().any(|attributed| matches!(
        attributed,
        AttributedMount {
            origin: MountOrigin::Tooling,
            mount: Mount::Bind { destination, access: MountAccess::ReadOnly, .. },
        } if destination == std::path::Path::new("/home/operator/.local/bin/claude")
    )));
    assert!(build_request(&spec).mounts.iter().any(|mount| matches!(
        mount,
        Mount::Bind { destination, access: MountAccess::ReadOnly, .. }
            if destination == std::path::Path::new("/home/operator/.local/bin/claude")
    )));
}

/// What the private root carries is reported alongside the mounts, grouped
/// by the capability that asked for it. It is the rest of the answer to
/// "what can this agent reach", and none of it is the operator's home.
#[test]
fn the_captured_policy_states_the_base_it_runs_on() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let options = capture(&workspace_spec(&dir)).unwrap();

    let names: Vec<&str> = options
        .base
        .iter()
        .map(|capability| capability.name.as_str())
        .collect();
    assert!(names.contains(&"core"), "{names:?}");
    assert!(names.contains(&"dns"), "{names:?}");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    assert!(options
        .base
        .iter()
        .flat_map(|capability| capability.entries.iter())
        .all(|entry| !home
            .as_ref()
            .is_some_and(|home| entry.path.starts_with(home))));
}

/// The mounts are not the whole of what the sandbox holds: the backend
/// lays down a floor of its own under them, and part of it is writable.
/// A profile that pins `HOME` under `/tmp` is the case that makes this
/// matter — the agent's entire home is then scratch space no mount row
/// anywhere accounts for.
#[test]
fn the_captured_policy_states_the_floor_the_backend_lays_down() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let options = capture(&workspace_spec(&dir)).unwrap();

    let floor: Vec<(driva::FloorKind, String)> = options
        .floor
        .iter()
        .map(|entry| (entry.kind, entry.path.display().to_string()))
        .collect();
    assert!(
        floor.contains(&(driva::FloorKind::PrivateRoot, "/".to_owned())),
        "the private root has to be said: {floor:?}"
    );
    assert!(
        floor.contains(&(driva::FloorKind::Tmpfs, "/tmp".to_owned())),
        "every sandbox gets /tmp whether or not anything asked: {floor:?}"
    );
    // The agent's home lives on that floor rather than in any mount.
    let home = options
        .environment
        .iter()
        .find(|variable| variable.name == "HOME")
        .expect("the profile pins HOME");
    assert!(
        home.value.starts_with("/tmp/"),
        "unexpected sandbox home {}",
        home.value
    );
    assert!(!options.mounts.iter().any(|attributed| attributed
        .mount
        .destination()
        .to_string_lossy()
        == home.value));
}

/// The environment is reported whole and attributed layer by layer, the way
/// the mounts are: the sandbox's own defaults, the profile's, and — where
/// one is selected — a template's over it.
#[test]
fn the_captured_policy_attributes_every_environment_variable() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.template = Some(ResolvedTemplate {
        mounts: Vec::new(),
        environment: BTreeMap::from([(OsString::from("TERM"), OsString::from("dumb"))]),
        network: false,
        capabilities: Vec::new(),
        run: None,
        test: None,
    });

    let options = capture(&spec).unwrap();
    let variable = |name: &str| {
        options
            .environment
            .iter()
            .find(|variable| variable.name == name)
            .unwrap_or_else(|| panic!("{name} is not reported"))
            .clone()
    };

    assert_eq!(variable("PATH").origin, VariableOrigin::Sandbox);
    assert_eq!(variable("HOME").origin, VariableOrigin::Profile);
    // Set by the profile and again by the template: attributed to the layer
    // whose value the agent will actually see.
    assert_eq!(variable("TERM").origin, VariableOrigin::Template);
    assert_eq!(variable("TERM").value, "dumb");
    // And what is reported is what is enforced.
    assert_eq!(
        build_request(&spec)
            .environment
            .get(&OsString::from("TERM")),
        Some(&OsString::from("dumb"))
    );
}

/// The operator's user-config variables sit between the profile and a
/// template: they override what the profile sets and yield to what a
/// template selected for this launch sets.
#[test]
fn user_config_variables_layer_between_profile_and_template() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.user_environment = BTreeMap::from([
        (OsString::from("TERM"), OsString::from("xterm")),
        (OsString::from("EDITOR"), OsString::from("vi")),
        (OsString::from("PAGER"), OsString::from("less")),
    ]);
    spec.template = Some(ResolvedTemplate {
        mounts: Vec::new(),
        environment: BTreeMap::from([(OsString::from("PAGER"), OsString::from("cat"))]),
        network: false,
        capabilities: Vec::new(),
        run: None,
        test: None,
    });

    let options = capture(&spec).unwrap();
    let variable = |name: &str| {
        options
            .environment
            .iter()
            .find(|variable| variable.name == name)
            .unwrap_or_else(|| panic!("{name} is not reported"))
            .clone()
    };

    assert_eq!(variable("EDITOR").origin, VariableOrigin::User);
    assert_eq!(variable("TERM").origin, VariableOrigin::User);
    assert_eq!(variable("TERM").value, "xterm");
    assert_eq!(variable("PAGER").origin, VariableOrigin::Template);
    assert_eq!(variable("PAGER").value, "cat");
    let request = build_request(&spec);
    assert_eq!(
        request.environment.get(&OsString::from("EDITOR")),
        Some(&OsString::from("vi"))
    );
}

/// A capability a template requires reaches the base the launch runs on,
/// so a template can state what its command needs without the Workspace
/// having to know.
#[test]
fn a_template_requirement_reaches_the_captured_base() {
    let dir = PathBuf::from("/tmp/styra/workspace");
    let mut spec = workspace_spec(&dir);
    spec.base
        .include
        .retain(|name| *name == driva::Capability::Core);
    let before = capture(&spec).unwrap();
    assert_eq!(before.base.len(), 1);

    spec.base.include(driva::Capability::Timezone);
    let after = capture(&spec).unwrap();
    assert_eq!(
        after
            .base
            .iter()
            .map(|capability| capability.name.as_str())
            .collect::<Vec<_>>(),
        vec!["core", "timezone"]
    );
}
