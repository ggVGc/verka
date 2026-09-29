//! Branching a Session that works in a linked checkout gives the branch a
//! checkout of its own, forked from the source's branch.
//!
//! Driven through the real daemon over its socket, on a real repository: what
//! is being asserted is where Git put the new working tree and what its branch
//! started from, which only Git can answer. An integration test also gets its
//! own `HOME` — a branch copies the provider's native transcript, and that
//! lives under the operator's home directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use styra_server::agent::{MessageFormat, Profile, Provider, Selection};
use styra_server::ensure_server;
use styra_server::event::Protocol;
use styra_server::journal::{self, Journal};
use styra_server::protocol::BranchHistory;
use styra_server::worktree::Checkout;

/// One Codex conversation, in Codex's own on-disk rollout shape.
const CODEX_ROLLOUT: &str = concat!(
    r#"{"timestamp":"2026-08-21T10:00:00.000Z","type":"session_meta","payload":{"id":"branch-source","session_id":"branch-source","timestamp":"2026-08-21T10:00:00.000Z","cwd":"/tmp/styra/workspace"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-21T10:00:00.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"teach the picker to filter"}]}}"#,
    "\n",
    r#"{"timestamp":"2026-08-21T10:00:01.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
    "\n",
);

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("running git {arguments:?}: {error}"));
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A scratch HOME and state directory, and a spawner pointed at the freshly
/// built daemon. The daemon inherits all three, so it reads the same store
/// this test writes the source Session into.
fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!("styra-branch-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("STYRA_SERVER_BIN", env!("CARGO_BIN_EXE_styra-server"));
    std::env::set_var("XDG_STATE_HOME", root.join("state"));
    std::env::set_var("HOME", root.join("home"));
    root
}

/// A repository with one commit on `main`, which is what a worktree can be
/// added from.
fn repository(host: &Path) {
    std::fs::create_dir_all(host).unwrap();
    git(host, &["init", "--initial-branch=main"]);
    git(host, &["config", "user.email", "test@example.invalid"]);
    git(host, &["config", "user.name", "Test"]);
    std::fs::write(host.join("README.md"), "project\n").unwrap();
    git(host, &["add", "."]);
    git(host, &["commit", "-m", "start"]);
}

#[test]
fn branching_a_session_forks_the_checkout_it_works_in() {
    let root = scratch();
    let home = root.join("home");
    let host = root.join("project");
    repository(&host);
    let store = styra_server::paths::default_store().unwrap();

    let workspace = styra_server::workspace::create(&store, &host, Some("work".into())).unwrap();
    let selection = Selection::new(Provider::Codex);
    let profile = Profile {
        name: "codex".into(),
        command: vec!["true".into()],
        protocol: Protocol::CodexJsonl,
        mounts: Vec::new(),
        environment: Default::default(),
        network: false,
        message_format: MessageFormat::CodexSubmission,
        single_turn: false,
    };
    let (mut journal, source_id) = Journal::create_in_workspace(
        &store,
        &workspace.id,
        &profile,
        &selection,
        Some("picker".into()),
    )
    .unwrap();
    journal
        .record_user_message("teach the picker to filter")
        .unwrap();
    let source_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);

    let native_id = "branch-source";
    journal::store_provider_session_id(&source_path, native_id).unwrap();
    let native = home.join(".codex/sessions/2026/08/21");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(
        native.join(format!("rollout-{native_id}.jsonl")),
        CODEX_ROLLOUT,
    )
    .unwrap();

    // The checkout the source works in, written the way a `W` press writes it:
    // a topic in front of the Session id, on a branch of the same name. Its own
    // commit is what tells a fork of this branch apart from a fork of `main`.
    let worktrees = styra_server::workspace::worktrees_dir(&store, &workspace.id);
    std::fs::create_dir_all(&worktrees).unwrap();
    let source_checkout = worktrees.join(format!("teach-the-picker-to-filter-{source_id}"));
    git(
        &host,
        &[
            "worktree",
            "add",
            "-b",
            &format!("styra/teach-the-picker-to-filter-{source_id}"),
            "--",
            source_checkout.to_str().unwrap(),
        ],
    );
    std::fs::write(source_checkout.join("picker.rs"), "filtered\n").unwrap();
    git(&source_checkout, &["add", "."]);
    git(&source_checkout, &["commit", "-m", "filter the picker"]);
    let source_head = git(&source_checkout, &["rev-parse", "HEAD"]);
    assert_ne!(source_head, git(&host, &["rev-parse", "HEAD"]));
    journal::store_session_checkout(&source_path, &Checkout::at(source_checkout.clone())).unwrap();

    let client = ensure_server(root.join("styra.sock")).expect("the daemon should start");
    let branched = client
        .branch_session(&source_id, None, BranchHistory::ThroughSelected, None)
        .expect("branching a stored Session");

    // The branch works somewhere of its own, under the topic it inherited.
    let checkout = journal::read_session_checkout(&branched.path)
        .unwrap()
        .expect("a branch of a Session with a checkout has one");
    let path = checkout.path.clone().expect("the checkout was just made");
    assert_eq!(
        path,
        worktrees.join(format!("teach-the-picker-to-filter-{}", branched.id))
    );
    assert_ne!(path, source_checkout, "the two work apart");
    assert_eq!(
        checkout.branch,
        format!("styra/teach-the-picker-to-filter-{}", branched.id)
    );

    // And it starts where the source's work had got to, not at the repository's
    // own head — which is the whole point of continuing that conversation.
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), source_head);
    assert_eq!(
        std::fs::read_to_string(path.join("picker.rs")).unwrap(),
        "filtered\n"
    );

    // The source keeps everything it had: a branch takes a copy.
    assert_eq!(
        journal::read_session_checkout(&source_path).unwrap(),
        Some(Checkout::at(source_checkout))
    );

    // The daemon this test spawned is detached, so it would outlive the run
    // and keep its scratch store open unless it is told to stop.
    client.shutdown().ok();
    std::fs::remove_dir_all(&root).ok();
}
