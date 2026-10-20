use std::path::{Path, PathBuf};

use styra_server::agent::{MessageFormat, Profile, Provider, Selection};
use styra_server::event::Protocol;
use styra_server::journal::{self, Journal};

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
pub fn scratch() -> PathBuf {
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
pub fn stored_claude_session(store: &Path, workspace_id: &str, home: &Path) -> (String, PathBuf) {
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
    let (mut journal, id) = Journal::create_in_workspace(
        store,
        workspace_id,
        &profile,
        &selection,
        Some("review".into()),
    )
    .unwrap();
    journal
        .record_user_message("why is the checkout failing?")
        .unwrap();
    journal.record_agent_line(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"the lockfile is stale"}]}}"#).unwrap();
    let session_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);

    let native_id = "conversion-source";
    journal::store_provider_session_id(&session_path, native_id).unwrap();
    let native = home.join(".claude/projects/-project");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(native.join(format!("{native_id}.jsonl")), CLAUDE_TRANSCRIPT).unwrap();

    (id, session_path)
}
