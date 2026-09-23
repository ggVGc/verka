//! Converting a Session to the other provider seals the Session it came from.
//!
//! Driven through the real daemon over its socket, because the seal is the
//! server's own doing rather than anything a client asks for: no request says
//! "seal this", and the only way to observe the rule is to convert and then
//! look at what the source became.
//!
//! This is an integration test rather than a unit test because it needs `HOME`
//! pointed at a scratch directory — that is where a provider's native
//! transcript lives, and a conversion is a copy from one provider's home to
//! the other's. Setting it here is safe: this file is its own process, and its
//! one test is the only thing in it.

use std::path::{Path, PathBuf};

use styra_server::agent::{MessageFormat, Profile, Provider, Selection};
use styra_server::event::Protocol;
use styra_server::journal::{self, Journal};
use styra_server::protocol::{CompletionState, LaunchPolicy, ResumeSession};
use styra_server::ensure_server;

/// One Claude conversation, in Claude Code's own on-disk transcript shape.
const CLAUDE_TRANSCRIPT: &str = concat!(
    r#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/project","sessionId":"conversion-source","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"why is the checkout failing?"}}"#,
    "\n",
    r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/project","sessionId":"conversion-source","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"the lockfile is stale"}]}}"#,
    "\n",
);

/// A scratch HOME and state directory, and a spawner pointed at the freshly
/// built daemon. The daemon inherits all three, so it reads the same store
/// this test writes the source Session into.
fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!("styra-conversion-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("STYRA_SERVER_BIN", env!("CARGO_BIN_EXE_styra-server"));
    std::env::set_var("XDG_STATE_HOME", root.join("state"));
    std::env::set_var("HOME", root.join("home"));
    root
}

/// Put a Session on disk with a native transcript behind it, without running
/// an agent: a conversion reads the stored history and the provider's own
/// file, and neither needs a live process to exist.
fn stored_claude_session(store: &Path, workspace_id: &str, home: &Path) -> (String, PathBuf) {
    let selection = Selection::new(Provider::Claude);
    let profile = Profile {
        name: "claude".into(),
        command: vec!["true".into()],
        protocol: Protocol::ClaudeJsonl,
        mounts: Vec::new(),
        environment: Default::default(),
        network: false,
        message_format: MessageFormat::ClaudeStreamJson,
        single_turn: false,
    };
    let (mut journal, id) =
        Journal::create_in_workspace(store, workspace_id, &profile, &selection, Some("review".into()))
            .unwrap();
    journal.record_user_message("why is the checkout failing?").unwrap();
    let session_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);

    let native_id = "conversion-source";
    journal::store_provider_session_id(&session_path, native_id).unwrap();
    let native = home.join(".claude/projects/-project");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(native.join(format!("{native_id}.jsonl")), CLAUDE_TRANSCRIPT).unwrap();

    (id, session_path)
}

#[test]
fn converting_a_session_seals_the_one_it_was_converted_from() {
    let root = scratch();
    let home = root.join("home");
    let host = root.join("project");
    std::fs::create_dir_all(&host).unwrap();
    let store = styra_server::paths::default_store().unwrap();

    let workspace = styra_server::workspace::create(&store, &host, Some("work".into())).unwrap();
    let (source_id, source_path) = stored_claude_session(&store, &workspace.id, &home);

    // Nothing is finished with yet: the seal below is the conversion's doing,
    // not a state the Session was already in.
    assert_eq!(
        journal::read_session_completed(&source_path).unwrap(),
        CompletionState::Active
    );

    let client = ensure_server(&root.join("styra.sock")).expect("the daemon should start");
    let converted = client
        .convert_session_provider(&source_id)
        .expect("converting a stored Claude Session to Codex");

    // The conversation carried on somewhere else, under the other provider.
    assert_eq!(converted.selection.provider, Provider::Codex);
    assert_ne!(converted.id, source_id);

    // And the Session it came from is sealed, not merely completed: its
    // transcript stayed with Claude, so there is nothing to go back to it for.
    assert_eq!(
        journal::read_session_completed(&source_path).unwrap(),
        CompletionState::Sealed
    );

    // Which is to say it can never run again.
    let error = client
        .resume_session(&ResumeSession {
            id: source_id.clone(),
            launch: LaunchPolicy::default(),
            selection: None,
        })
        .expect_err("a sealed Session cannot be resumed");
    assert!(error.to_string().contains("sealed"), "{error:#}");

    // The seal is not something a client can talk the server out of either.
    let error = client
        .set_session_completed(&source_id, CompletionState::Active)
        .expect_err("a sealed Session cannot be un-sealed");
    assert!(error.to_string().contains("sealed"), "{error:#}");

    // The daemon this test spawned is detached, so it would outlive the run
    // and keep its scratch store open unless it is told to stop.
    client.shutdown().ok();
    std::fs::remove_dir_all(&root).ok();
}
