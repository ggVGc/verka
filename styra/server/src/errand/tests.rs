use super::*;
use crate::agent::Effort;
use std::os::unix::fs::PermissionsExt;

/// An errand profile built against a stub agent, so the test states what
/// the sandbox holds on any host — installed agent or not.
fn stub_profile(provider: Provider) -> Profile {
    let directory = std::env::temp_dir().join(format!(
        "styra-errand-stub-{}-{}",
        std::process::id(),
        crate::journal::now_ms()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = directory.join(provider.executable());
    std::fs::write(&executable, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    Errand::new(provider, "name this")
        .selection()
        .resolve_for_on_path(
            genta::agent::ExecutionMode::Batch,
            &SandboxLayout::same_path(SCRATCH),
            &OsString::from(&directory),
        )
        .unwrap()
}

/// An errand is the operator's own agent on its cheap tier, in the
/// one-shot form — never the model their session runs on. Cheap, but still
/// launchable: the rung is the lowest that model actually accepts, and the
/// model is the cheapest one that takes a rung at all.
#[test]
fn an_errand_runs_on_the_providers_cheapest_launchable_model() {
    let codex = Errand::new(Provider::Codex, "name this").selection();
    assert_eq!(codex.provider, Provider::Codex);
    assert_eq!(codex.model, "gpt-5.6-luna");
    assert_eq!(codex.effort, Effort::Low);

    let claude = Errand::new(Provider::Claude, "name this").selection();
    assert_eq!(claude.provider, Provider::Claude);
    // Not Haiku 4.5, which is cheaper but takes no effort setting at all.
    assert_eq!(claude.model, "claude-sonnet-5-5");
    assert_eq!(claude.effort, Effort::Low);

    for provider in crate::agent::PROVIDERS {
        let errand = Errand::new(provider, "name this").selection();
        assert_ne!(errand.model, provider.default_model());
        crate::agent::validate_selection(&Selection {
            provider,
            ..errand.clone()
        })
        .unwrap();
    }
}

/// The sandbox holds the scratch directory, the agent binary, and the
/// provider's own credentials — and nothing of the operator's. This is the
/// property the whole type exists for, so it is asserted exhaustively:
/// every mount is one of the three, not merely "none of the bad ones".
#[test]
fn the_sandbox_holds_nothing_but_scratch_the_agent_and_its_credentials() {
    let profile = stub_profile(Provider::Claude);
    let base = driva::BaseConfig::default();
    let request = request(&profile, &base, None).unwrap();

    assert_eq!(request.working_directory, PathBuf::from(SCRATCH));
    assert!(matches!(
        request.mounts[0],
        Mount::Temporary { ref destination } if destination == Path::new(SCRATCH)
    ));
    let profile_destinations: Vec<&Path> = profile
        .mounts
        .iter()
        .map(|mount| mount.destination.as_path())
        .collect();
    let executable = PathBuf::from(&profile.command[0]);
    for mount in &request.mounts[1..] {
        let destination = mount.destination();
        assert!(
            profile_destinations.contains(&destination)
                || executable.starts_with(destination)
                || destination.starts_with(&executable),
            "{destination:?} is neither the agent nor a profile mount"
        );
    }
    assert!(request.new_session);
    assert!(!request.interactive);
}

/// Resuming a Session, nothing is mounted writable: its checkout and
/// repository are read-only whatever they asked for, the provider's state
/// is an overlay whose writes are discarded, and the agent stands where
/// the Session did rather than in scratch.
#[test]
fn a_resumed_errand_writes_nothing_back() {
    let profile = stub_profile(Provider::Claude);
    let resume = Resume {
        selection: Selection::new(Provider::Claude),
        provider_session_id: "claude-session-1".into(),
        working_directory: "/tmp/styra/workspace".into(),
        mounts: vec![
            MountSpec {
                source: "/host/checkout".into(),
                destination: "/tmp/styra/workspace".into(),
                writable: false,
            },
            MountSpec {
                source: "/host/repository/.git".into(),
                destination: "/host/repository/.git".into(),
                writable: true,
            },
            MountSpec {
                source: "/host/checkout".into(),
                destination: "/tmp/styra/workspace".into(),
                writable: false,
            },
        ],
    };
    let request = request(&profile, &driva::BaseConfig::default(), Some(&resume)).unwrap();

    assert_eq!(request.working_directory, Path::new("/tmp/styra/workspace"));
    let checkouts = request
        .mounts
        .iter()
        .filter(|mount| mount.destination() == Path::new("/tmp/styra/workspace"))
        .count();
    assert_eq!(
        checkouts, 1,
        "the same directory asked for twice is bound once"
    );
    assert!(request
        .mounts
        .iter()
        .all(|mount| !matches!(mount, Mount::Temporary { .. })));
    assert!(request.mounts.iter().all(|mount| !matches!(
        mount,
        Mount::Bind {
            access: MountAccess::ReadWrite,
            ..
        }
    )));
    for state in profile.mounts.iter().filter(|mount| mount.writable) {
        assert!(request.mounts.iter().any(|mount| matches!(
            mount,
            Mount::Overlay { destination, .. } if *destination == state.destination
        )));
    }
    assert!(request.mounts.contains(&Mount::Bind {
        source: "/host/repository/.git".into(),
        destination: "/host/repository/.git".into(),
        access: MountAccess::ReadOnly,
    }));
}

/// The prompt reaches the agent as a message in its own input format, not
/// as a command-line argument — a first prompt can be anything at all.
#[test]
fn the_prompt_is_written_to_stdin_in_the_agents_own_format() {
    let profile = stub_profile(Provider::Codex);
    let request = request(&profile, &driva::BaseConfig::default(), None).unwrap();

    assert_eq!(request.command.last().unwrap(), "-");
    let encoded = String::from_utf8(profile.encode_message("Fix the flaky test")).unwrap();
    assert_eq!(encoded, "Fix the flaky test\n");
    assert!(!request
        .command
        .iter()
        .any(|argument| argument.to_string_lossy().contains("Fix the flaky test")));
}

/// The answer is what the agent said last, not its reasoning, its tool
/// calls, or an earlier draft.
#[test]
fn the_answer_is_the_agents_last_message() {
    let output = [
            r#"{"type":"item.completed","item":{"id":"r1","type":"reasoning","text":"thinking"}}"#,
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"first-guess"}}"#,
            r#"{"type":"item.completed","item":{"id":"m2","type":"agent_message","text":" fix-flaky-checkout-test\n"}}"#,
        ]
        .join("\n");

    assert_eq!(
        answer_from(Protocol::CodexJsonl, &output).as_deref(),
        Some("fix-flaky-checkout-test")
    );
    assert_eq!(answer_from(Protocol::CodexJsonl, "not json\n"), None);
    assert_eq!(answer_from(Protocol::CodexJsonl, ""), None);
}

/// The prompt is handed over as an unlinked file: the agent reads it, and
/// nothing on the host is left holding what the operator typed.
#[test]
fn the_prompt_file_does_not_outlive_the_call_that_staged_it() {
    let streams = Streams::open(b"name this\n").unwrap();
    let staged: Vec<PathBuf> = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "in")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("styra-errand-"))
        })
        .collect();

    assert!(staged.is_empty(), "{staged:?} outlived the errand's setup");
    assert!(streams.output.exists());
    std::fs::remove_file(streams.output).ok();
}
