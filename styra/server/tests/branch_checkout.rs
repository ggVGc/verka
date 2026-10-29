//! Branching can fork a linked checkout or share its files and Git branch.
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
use styra_server::protocol::{
    BranchCheckout, BranchHistory, CompletionState, InteractionActivity, InteractionActivityReason,
    WorktreeCleanup,
};
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
fn branching_sessions_can_fork_or_share_the_source_checkout() {
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

    assert_eq!(branched.name.as_deref(), Some("Branch: picker"));

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
    // The branch has a first snapshot before it is ever resumed, but diff
    // mode measures from where it will be integrated: the source's branch.
    let snapshots = journal::read_snapshots(&branched.path).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].turn, Some(2));
    assert_eq!(
        client.diff_base(&branched.id).unwrap(),
        Some(styra_server::protocol::DiffBase {
            base: source_head.clone(),
            branch: Some(format!("styra/teach-the-picker-to-filter-{source_id}")),
        })
    );
    assert_eq!(
        std::fs::read_to_string(path.join("picker.rs")).unwrap(),
        "filtered\n"
    );

    // Its log names where it came from twice: at the top, and again at the
    // branch point, after the copied history and before anything of its own.
    let marker = styra_server::event::AgentEvent::Branched {
        direction: styra_server::event::BranchDirection::From,
        session: source_id.clone(),
        name: None,
    };
    assert_eq!(
        journal::replay(&branched.path, Protocol::CodexJsonl).unwrap(),
        vec![
            marker.clone(),
            styra_server::event::AgentEvent::UserMessage {
                text: "teach the picker to filter".into(),
            },
            marker,
        ]
    );

    // The source keeps everything it had: a branch takes a copy.
    assert_eq!(
        journal::read_session_checkout(&source_path).unwrap(),
        Some(Checkout::at(source_checkout.clone()))
    );

    // Nothing runs the branch yet, but it is listed among the interactions,
    // stopped, working in its own checkout and naming the Session it came
    // from so a listing can nest it there.
    let row = client
        .list_interactions(&styra_server::protocol::InteractionQuery::default())
        .unwrap()
        .into_iter()
        .find(|interaction| interaction.id == branched.id)
        .expect("the branch is listed among the interactions");
    assert_eq!(row.activity, InteractionActivity::Stopped);
    assert_eq!(
        row.activity_reason,
        Some(InteractionActivityReason::Branched)
    );
    assert_eq!(
        row.origin.map(|origin| origin.session_id),
        Some(source_id.clone())
    );
    assert_eq!(row.workspace, path);
    let loaded = client
        .load_interaction(&branched.id)
        .expect("the listed branch opens");
    assert_eq!(
        loaded.summary.activity_reason,
        Some(InteractionActivityReason::Branched)
    );

    // A conversation branch can share the exact same checkout, including
    // uncommitted files, while retaining its own transcript and history.
    std::fs::write(source_checkout.join("picker.rs"), "unfinished edits\n").unwrap();
    let refs_before = git(&host, &["for-each-ref", "--format=%(refname)"]);
    let worktrees_before = git(&host, &["worktree", "list", "--porcelain"]);
    let shared = client
        .branch_session_with_checkout(
            &source_id,
            None,
            BranchHistory::ThroughSelected,
            None,
            None,
            Some(BranchCheckout::ShareSource),
        )
        .unwrap();
    assert_ne!(shared.id, source_id);
    assert_ne!(
        journal::read_provider_session_id(&shared.path).unwrap(),
        Some(native_id.into())
    );
    assert_eq!(
        journal::read_session_checkout(&shared.path).unwrap(),
        journal::read_session_checkout(&source_path).unwrap()
    );
    assert_eq!(
        git(&host, &["for-each-ref", "--format=%(refname)"]),
        refs_before
    );
    assert_eq!(
        git(&host, &["worktree", "list", "--porcelain"]),
        worktrees_before
    );
    assert_eq!(
        std::fs::read_to_string(source_checkout.join("picker.rs")).unwrap(),
        "unfinished edits\n"
    );
    assert_eq!(git(&source_checkout, &["rev-parse", "HEAD"]), source_head);
    let shared_row = client.load_interaction(&shared.id).unwrap().summary;
    assert_eq!(shared_row.workspace, source_checkout);
    assert_eq!(shared_row.origin.unwrap().session_id, source_id);
    assert!(journal::replay(&shared.path, Protocol::CodexJsonl).unwrap().iter().any(|event| {
        matches!(event, styra_server::event::AgentEvent::UserMessage { text } if text == "teach the picker to filter")
    }));

    // Selecting one message still shares today's files; it only narrows history.
    let at_ms = journal::replay_raw(&source_path)
        .unwrap()
        .into_iter()
        .find(|line| line.text == "teach the picker to filter")
        .unwrap()
        .at_ms;
    let selected = client
        .branch_session_with_checkout(
            &source_id,
            Some(at_ms),
            BranchHistory::SelectedOnly,
            None,
            None,
            Some(BranchCheckout::ShareSource),
        )
        .unwrap();
    assert_eq!(
        client
            .load_interaction(&selected.id)
            .unwrap()
            .summary
            .workspace,
        source_checkout
    );
    assert_eq!(
        std::fs::read_to_string(source_checkout.join("picker.rs")).unwrap(),
        "unfinished edits\n"
    );
    assert_eq!(
        journal::replay(&selected.path, Protocol::CodexJsonl)
            .unwrap()
            .iter()
            .filter(|event| matches!(event, styra_server::event::AgentEvent::UserMessage { .. }))
            .count(),
        1
    );
    client
        .set_session_completed(&selected.id, CompletionState::Completed)
        .unwrap();

    // Finishing the source must not remove the checkout of its active sibling.
    std::fs::write(source_checkout.join("picker.rs"), "filtered\n").unwrap();
    client
        .set_session_completed(&source_id, CompletionState::Completed)
        .unwrap();
    let cleaned = client.clean_worktrees(Some(&workspace.id)).unwrap();
    assert!(cleaned.iter().any(|entry| entry.session_id == source_id &&
        matches!(&entry.outcome, WorktreeCleanup::Shared { session_id } if session_id == &shared.id)));
    assert!(source_checkout.exists());

    // Once both are finished it may be cleaned. Sharing a historical source
    // restores that same branch instead of forking a replacement branch.
    client
        .set_session_completed(&shared.id, CompletionState::Abandoned)
        .unwrap();
    client.clean_worktrees(Some(&workspace.id)).unwrap();
    assert!(!source_checkout.exists());
    let restored = client
        .branch_session_with_checkout(
            &source_id,
            None,
            BranchHistory::ThroughSelected,
            None,
            None,
            Some(BranchCheckout::ShareSource),
        )
        .unwrap();
    let restored_checkout = journal::read_session_checkout(&restored.path)
        .unwrap()
        .unwrap();
    assert_eq!(
        restored_checkout.branch,
        format!("styra/teach-the-picker-to-filter-{source_id}")
    );
    let restored_path = restored_checkout.path.unwrap();
    assert!(restored_path.exists());
    assert_eq!(git(&restored_path, &["rev-parse", "HEAD"]), source_head);
    assert_eq!(
        client
            .load_interaction(&restored.id)
            .unwrap()
            .summary
            .workspace,
        restored_path
    );
    assert_eq!(
        git(&host, &["for-each-ref", "--format=%(refname)"]),
        refs_before
    );

    // Sources in the ordinary Workspace preserve the old default and can
    // explicitly opt into a new worktree as well.
    let (mut ordinary_journal, ordinary_id) = Journal::create_in_workspace(
        &store,
        &workspace.id,
        &profile,
        &selection,
        Some("ordinary".into()),
    )
    .unwrap();
    ordinary_journal
        .record_user_message("teach the picker to filter")
        .unwrap();
    let ordinary_path = ordinary_journal.path().parent().unwrap().to_path_buf();
    drop(ordinary_journal);
    journal::store_provider_session_id(&ordinary_path, native_id).unwrap();
    let ordinary = client
        .branch_session(&ordinary_id, None, BranchHistory::ThroughSelected, None)
        .unwrap();
    assert_eq!(
        journal::read_session_checkout(&ordinary.path).unwrap(),
        None
    );
    assert_eq!(
        client
            .load_interaction(&ordinary.id)
            .unwrap()
            .summary
            .workspace,
        host
    );
    let isolated = client
        .branch_session_with_checkout(
            &ordinary_id,
            None,
            BranchHistory::ThroughSelected,
            None,
            None,
            Some(BranchCheckout::NewWorktree),
        )
        .unwrap();
    let isolated_checkout = journal::read_session_checkout(&isolated.path)
        .unwrap()
        .unwrap();
    let isolated_path = isolated_checkout.path.unwrap();
    assert_ne!(isolated_path, host);
    assert_eq!(
        git(&isolated_path, &["rev-parse", "HEAD"]),
        git(&host, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        client
            .load_interaction(&isolated.id)
            .unwrap()
            .summary
            .workspace,
        isolated_path
    );

    // The daemon this test spawned is detached, so it would outlive the run
    // and keep its scratch store open unless it is told to stop.
    client.shutdown().ok();
    std::fs::remove_dir_all(&root).ok();
}
