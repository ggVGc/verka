use super::*;
use driva::{BwrapIsolation, ExecutionRequest, Mount, MountAccess};
use std::collections::BTreeMap;
use std::ffi::OsString;

/// A stored agent-output fact is self-describing: each protocol stamps a
/// distinct media type, and that media type maps back to exactly the decoder
/// that produced it. A new decoder adds a variant and a distinct type; this
/// guards the round-trip the durable render path depends on.
#[test]
fn output_media_types_round_trip_to_their_decoder() {
    for protocol in [
        OutputFormat::Plain,
        OutputFormat::Agent(Protocol::CodexJsonl),
        OutputFormat::Agent(Protocol::CodexAppServer),
        OutputFormat::Agent(Protocol::ClaudeJsonl),
    ] {
        assert_eq!(
            OutputFormat::from_output_media_type(protocol.output_media_type()),
            Some(protocol),
            "{protocol:?} media type must select its own decoder"
        );
    }
    assert_ne!(
        OutputFormat::Plain.output_media_type(),
        OutputFormat::Agent(Protocol::CodexJsonl).output_media_type(),
        "each decoder must be distinguishable by its stored media type"
    );
    // An output written by a newer or unknown decoder is refused, never
    // mis-decoded as a format this build happens to recognise.
    assert_eq!(
        OutputFormat::from_output_media_type("application/vnd.orka.future.v9+ndjson"),
        None
    );
}

#[test]
fn output_formats_keep_their_flat_durable_names() {
    let codex = OutputFormat::Agent(Protocol::CodexJsonl);
    assert_eq!(serde_json::to_string(&codex).unwrap(), r#""codex-jsonl""#);
    assert_eq!(
        serde_json::from_str::<OutputFormat>(r#""codex-jsonl""#).unwrap(),
        codex
    );
    assert_eq!(
        serde_json::from_str::<OutputFormat>(r#""plain""#).unwrap(),
        OutputFormat::Plain
    );
}

/// A stub codex install, so profile construction resolves against a known
/// path instead of whatever the machine running the tests has on its `PATH`.
fn stub_codex() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let directory = std::env::temp_dir().join(format!("orka-agent-bin-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("codex");
    std::fs::write(&path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn codex_profile_uses_the_orka_layout_and_trusts_only_its_workspace() {
    let layout = SandboxLayout::default();
    let executable = stub_codex();
    let invocation = codex(&executable, &layout, "gpt-5.6-sol", Effort::High).unwrap();

    assert_eq!(layout.workspace, Path::new("/tmp/orka/workspace"));
    assert_eq!(layout.exchange, Path::new("/tmp/orka/exchange"));
    assert_eq!(Path::new(&invocation.command[0]), executable);
    assert_eq!(invocation.protocol, Protocol::CodexJsonl);
    assert!(invocation
        .command
        .iter()
        .any(|argument| argument == "--json"));
    assert!(invocation.command.iter().any(|argument| {
        argument == "projects.\"/tmp/orka/workspace\".trust_level=\"trusted\""
    }));
    assert_eq!(invocation.command.last().unwrap(), AGENT_PROMPT);
    // The attempt's own argv states the model and effort it ran, so the
    // recorded evidence says what produced the work.
    assert!(invocation
        .command
        .iter()
        .any(|argument| argument == r#"model="gpt-5.6-sol""#));
    assert!(invocation
        .command
        .iter()
        .any(|argument| argument == r#"model_reasoning_effort="high""#));
    assert!(invocation.network);
    assert!(invocation
        .mounts
        .iter()
        .any(|mount| mount.destination == Path::new("/tmp/agent-home/.codex") && mount.writable));
    assert_eq!(invocation.environment["HOME"], "/tmp/agent-home");
}

#[test]
fn codex_layout_needs_no_workspace_directory_in_a_bubblewrap_rootfs() {
    let rootfs = std::env::temp_dir().join(format!("orka-agent-rootfs-{}", ulid::Ulid::new()));
    for directory in ["proc", "dev", "tmp"] {
        std::fs::create_dir_all(rootfs.join(directory)).unwrap();
    }

    let layout = SandboxLayout::default();
    let executable = stub_codex();
    let invocation = codex(&executable, &layout, "gpt-5.6-sol", Effort::High).unwrap();
    let mut mounts = vec![
        Mount::Bind {
            source: "/host/attempt".into(),
            destination: layout.workspace.clone(),
            access: MountAccess::ReadWrite,
        },
        Mount::Bind {
            source: "/host/exchange".into(),
            destination: layout.exchange.clone(),
            access: MountAccess::ReadWrite,
        },
    ];
    mounts.extend(invocation.mounts.into_iter().map(|mount| Mount::Bind {
        source: mount.source,
        destination: mount.destination,
        access: if mount.writable {
            MountAccess::ReadWrite
        } else {
            MountAccess::ReadOnly
        },
    }));
    let request = ExecutionRequest {
        command: invocation.command.into_iter().map(OsString::from).collect(),
        working_directory: layout.workspace,
        mounts,
        writable_mounts: driva::WritableMountMode::Direct,
        environment: BTreeMap::new(),
        network: invocation.network,
        interactive: false,
        new_session: true,
    };
    let backend = BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: Some(rootfs.clone()),
        base: driva::BaseConfig::default(),
    };

    backend.command(&request).unwrap();
    assert!(!rootfs.join("workspace").exists());
    std::fs::remove_dir_all(rootfs).unwrap();
}
