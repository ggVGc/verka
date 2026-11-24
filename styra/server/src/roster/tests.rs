use super::*;
use crate::agent::{Effort, Provider, Selection};

fn store(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "styra-roster-{}-{name}-{}",
        std::process::id(),
        crate::journal::now_ms()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn summary(id: &str) -> InteractionSummary {
    InteractionSummary {
        id: id.to_owned(),
        name: None,
        tags: Vec::new(),
        workspace_id: "workspace".into(),
        selection: Selection {
            provider: Provider::Claude,
            model: "sonnet".into(),
            effort: Effort::Medium,
        },
        workspace: PathBuf::from("/tmp/project"),
        activity: InteractionActivity::Running,
        activity_reason: None,
        activity_since_ms: 7,
        idle_unseen: true,
        interacted_at_ms: 0,
        last_user_message_at_ms: 0,
        uncommitted_changes: false,
        expected_branch: None,
        checkout: Some(crate::protocol::CheckoutState {
            worktree: PathBuf::from("/tmp/worktrees/project-session"),
            repository: PathBuf::from("/tmp/project"),
            branch: Some("styra/project-session".into()),
        }),
        branched_from: Some(crate::protocol::BranchPoint {
            branch: Some("main".into()),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
        }),
        last_message: Some("still going".into()),
        recent_messages: Vec::new(),
        auto_retry: false,
        auto_commit: false,
        events: 12,
        completed: CompletionState::Active,
        focus_requested: false,
        origin: None,
        integration: None,
        restored: false,
    }
}

/// The whole point: a row mirrored by one run is a row the next run lists.
/// It comes back stopped, because the agent behind it did not come back,
/// and it keeps what it was talking about so the list still reads.
#[test]
fn a_mirrored_row_is_listed_again_by_the_next_run_as_stopped() {
    let root = store("restart");
    let session = root.join("session-a");
    std::fs::create_dir_all(&session).unwrap();

    let mut journal = crate::journal::Journal::create(&session).unwrap();
    journal.record_user_message("latest request").unwrap();
    drop(journal);
    let sent_at = crate::journal::last_user_message_at_ms(&session).unwrap();
    assert!(sent_at > 0);
    // A roster from before message times were reported has the default 0.
    Roster::open(&root).publish(vec![(session, summary("session-a"), Default::default())]);

    let restored = Roster::open(&root).restored(&InteractionQuery::default());
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].id, "session-a");
    assert_eq!(restored[0].activity, InteractionActivity::Stopped);
    assert_eq!(
        restored[0].activity_reason,
        Some(InteractionActivityReason::ServerRestarted)
    );
    assert_eq!(restored[0].last_message.as_deref(), Some("still going"));
    assert_eq!(restored[0].last_user_message_at_ms, sent_at);
    // Where the agent was working outlives the agent. Nothing this run can
    // do would re-derive it — the branch was read from a checkout the
    // previous run's agent may since have been the last to touch — so a
    // row that lost it would have to show the operator nothing.
    let checkout = restored[0].checkout.as_ref().expect("the checkout is kept");
    assert_eq!(checkout.branch.as_deref(), Some("styra/project-session"));
    assert_eq!(checkout.repository, PathBuf::from("/tmp/project"));
    assert!(checkout.linked());
    // The previous run's notification is not this run's to raise.
    assert!(!restored[0].idle_unseen);
    std::fs::remove_dir_all(root).ok();
}

/// Closing an Interaction is the one thing that takes it off the list, and
/// it has to take it off the mirrored one too — otherwise a restart would
/// bring back precisely the conversations the operator had finished with.
#[test]
fn a_forgotten_row_does_not_survive_the_next_publish() {
    let root = store("forget");
    let session = root.join("session-b");
    std::fs::create_dir_all(&session).unwrap();
    Roster::open(&root).publish(vec![(session, summary("session-b"), Default::default())]);

    let roster = Roster::open(&root);
    assert!(roster.holds("session-b"));
    roster.forget("session-b");
    roster.publish(Vec::new());

    assert!(Roster::open(&root)
        .restored(&InteractionQuery::default())
        .is_empty());
    std::fs::remove_dir_all(root).ok();
}

/// Completion is why a row is stopped, so a restart — which stops every
/// row it brings back — must not overwrite it with its own reason.
#[test]
fn a_restored_row_can_be_marked_completed() {
    let root = store("complete");
    let host = store("complete-host");
    let workspace = crate::workspace::create(&root, &host, Some("work".into())).unwrap();
    let profile = crate::agent::Profile {
        name: "codex".into(),
        command: vec!["true".into()],
        protocol: crate::event::Protocol::CodexJsonl,
        mounts: Vec::new(),
        environment: Default::default(),
        network: false,
        message_format: crate::agent::MessageFormat::PlainLine,
        single_turn: false,
    };
    let (_, id) = crate::journal::Journal::create_in_workspace(
        &root,
        &workspace.id,
        &profile,
        &Selection {
            provider: Provider::Codex,
            model: "codex".into(),
            effort: Effort::Medium,
        },
        None,
    )
    .unwrap();
    let session = crate::workspace::sessions_dir(&root, &workspace.id).join(&id);
    let mut row = summary(&id);
    row.workspace_id = workspace.id.clone();
    Roster::open(&root).publish(vec![(session.clone(), row, Default::default())]);

    // The roster's copy is an interaction snapshot, not another owner of
    // mutable Session metadata. A later rename is resolved directly from
    // the Session even though the mirrored row predates it.
    crate::journal::store_session_name(&session, Some("Renamed after restart")).unwrap();
    assert_eq!(
        Roster::open(&root).restored(&InteractionQuery::default())[0]
            .name
            .as_deref(),
        Some("Renamed after restart")
    );

    // Likewise tags: retagging a stopped row must show on the roster that
    // is already open, not only after the next restart.
    let roster = Roster::open(&root);
    crate::journal::store_session_tags(&session, &["bug".into()]).unwrap();
    assert_eq!(
        roster.restored(&InteractionQuery::default())[0].tags,
        vec!["bug".to_owned()]
    );

    // Set from a live interaction, as `set_completed` does: written to the
    // Session's own metadata first, then mirrored onto the roster row.
    crate::journal::store_session_completed(&session, CompletionState::Completed).unwrap();
    let roster = Roster::open(&root);
    assert!(roster.set_completed(&id, CompletionState::Completed));
    roster.publish(Vec::new());

    // Read back fresh, the way the next run does — the mirrored row is
    // never itself the record of it.
    let restored = Roster::open(&root).restored(&InteractionQuery::default());
    assert_eq!(restored[0].completed, CompletionState::Completed);
    assert_eq!(
        restored[0].activity_reason,
        Some(InteractionActivityReason::ServerRestarted)
    );
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(host).ok();
}

#[test]
fn an_operator_can_pause_a_restored_row() {
    let root = store("pause");
    let session = root.join("session-pause");
    std::fs::create_dir_all(&session).unwrap();
    Roster::open(&root).publish(vec![(
        session,
        summary("session-pause"),
        Default::default(),
    )]);

    let roster = Roster::open(&root);
    assert!(roster.pause("session-pause"));
    let restored = roster.restored(&InteractionQuery::default());
    assert_eq!(restored[0].activity, InteractionActivity::Stopped);
    assert_eq!(
        restored[0].activity_reason,
        Some(InteractionActivityReason::Paused)
    );
    std::fs::remove_dir_all(root).ok();
}

/// Finished rows are most of a long-lived roster, and a client polling
/// for the ones it shows asks to be spared them.
#[test]
fn a_query_leaves_out_the_rows_it_does_not_admit() {
    let root = store("query");
    let done = root.join("session-done");
    let open = root.join("session-open");
    std::fs::create_dir_all(&done).unwrap();
    std::fs::create_dir_all(&open).unwrap();
    Roster::open(&root).publish(vec![
        (done, summary("session-done"), Default::default()),
        (open, summary("session-open"), Default::default()),
    ]);
    let roster = Roster::open(&root);
    assert!(roster.set_completed("session-done", CompletionState::Sealed));

    let ids = |query: InteractionQuery| {
        let mut ids: Vec<String> = roster
            .restored(&query)
            .into_iter()
            .map(|summary| summary.id)
            .collect();
        ids.sort();
        ids
    };
    let unfinished = InteractionQuery {
        skip_done: true,
        ..InteractionQuery::default()
    };
    assert_eq!(ids(unfinished.clone()), vec!["session-open"]);
    assert_eq!(
        ids(InteractionQuery {
            keep: vec!["session-done".into()],
            ..unfinished
        }),
        vec!["session-done", "session-open"]
    );
    // Every restored row is stopped.
    assert!(ids(InteractionQuery {
        skip_stopped: true,
        ..InteractionQuery::default()
    })
    .is_empty());
    std::fs::remove_dir_all(root).ok();
}

/// A run that inherits rows and opens none of its own must not drop the
/// inherited ones when it mirrors the list: they are still open.
#[test]
fn publishing_keeps_the_rows_this_run_has_not_superseded() {
    let root = store("carry");
    let first = root.join("session-c");
    let second = root.join("session-d");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    Roster::open(&root).publish(vec![
        (first.clone(), summary("session-c"), Default::default()),
        (second, summary("session-d"), Default::default()),
    ]);

    // This run revived only one of the two.
    let roster = Roster::open(&root);
    roster.forget("session-c");
    roster.publish(vec![(first, summary("session-c"), Default::default())]);

    let mut ids: Vec<String> = Roster::open(&root)
        .restored(&InteractionQuery::default())
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["session-c", "session-d"]);
    std::fs::remove_dir_all(root).ok();
}

/// A restart that lands partway through a turn says so, while one that
/// finds the agent between turns does not — whatever the mirrored row
/// last recorded, since it is not rewritten as turns start and end.
#[test]
fn a_restart_mid_turn_is_told_apart_from_one_between_turns() {
    let root = store("mid-turn");
    let row = |name: &str, lines: &[&str]| {
        let session = root.join(name);
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("session.json"),
            r#"{"workspace_id":"w-1","protocol":"claude-jsonl","command":"claude",
                    "selection":{"provider":"claude","model":"sonnet","effort":"medium"}}"#,
        )
        .unwrap();
        let mut journal = crate::journal::Journal::create(&session).unwrap();
        journal.record_user_message("do the thing").unwrap();
        for line in lines {
            journal.record_agent_line(line).unwrap();
        }
        let mut summary = summary(name);
        summary.activity = InteractionActivity::Pending;
        (session, summary, Default::default())
    };
    Roster::open(&root).publish(vec![
        row("session-cut", &[]),
        row(
            "session-idle",
            &[r#"{"type":"result","subtype":"success","usage":{}}"#],
        ),
    ]);

    let mut restored = Roster::open(&root).restored(&InteractionQuery::default());
    restored.sort_by(|a, b| a.id.cmp(&b.id));
    let reasons: Vec<_> = restored
        .iter()
        .map(|row| (row.id.as_str(), row.activity_reason.clone()))
        .collect();
    assert_eq!(
        reasons,
        vec![
            (
                "session-cut",
                Some(InteractionActivityReason::ServerRestartedMidTurn)
            ),
            (
                "session-idle",
                Some(InteractionActivityReason::ServerRestarted)
            ),
        ]
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_row_already_stopped_keeps_its_reason_across_a_restart() {
    let root = store("already-stopped");
    let session = root.join("session-stopped");
    std::fs::create_dir_all(&session).unwrap();
    let mut row = summary("session-stopped");
    row.activity = InteractionActivity::Stopped;
    row.activity_reason = Some(InteractionActivityReason::Interrupted);
    Roster::open(&root).publish(vec![(session, row, Default::default())]);

    let restored = Roster::open(&root).restored(&InteractionQuery::default());
    assert_eq!(restored[0].activity, InteractionActivity::Stopped);
    assert_eq!(
        restored[0].activity_reason,
        Some(InteractionActivityReason::Interrupted)
    );
    assert!(restored[0].restored);
    std::fs::remove_dir_all(root).ok();
}

/// A Session deleted out from under the roster leaves a row that could
/// only be opened onto nothing.
#[test]
fn a_row_whose_session_is_gone_is_dropped_on_the_way_in() {
    let root = store("stale");
    let session = root.join("session-e");
    std::fs::create_dir_all(&session).unwrap();
    Roster::open(&root).publish(vec![(
        session.clone(),
        summary("session-e"),
        Default::default(),
    )]);
    std::fs::remove_dir_all(&session).unwrap();

    assert!(Roster::open(&root)
        .restored(&InteractionQuery::default())
        .is_empty());
    std::fs::remove_dir_all(root).ok();
}

/// A roster mirrored while the summary still carried the policy is the
/// only record of what those runs launched under, so the policy is read
/// out of the summary rather than lost with the field.
#[test]
fn a_row_mirrored_with_the_policy_in_its_summary_keeps_it() {
    let root = store("policy-in-summary");
    let session = root.join("session-f");
    std::fs::create_dir_all(&session).unwrap();
    let mut summary = serde_json::to_value(summary("session-f")).unwrap();
    summary["driva"] = serde_json::json!({
        "isolation_backend": "bwrap",
        "command": ["claude"],
        "working_directory": "/tmp/styra/workspace",
        "network": true,
        "mounts": []
    });
    let row = serde_json::json!({ "session_path": session, "summary": summary });
    std::fs::write(root.join(ROSTER_FILE), format!("{row}\n")).unwrap();

    let roster = Roster::open(&root);
    let driva = roster.driva("session-f").expect("the row is restored");
    assert_eq!(driva.isolation_backend, "bwrap");
    assert_eq!(driva.command, vec!["claude".to_owned()]);
    assert!(driva.network);

    // And it is written back beside the summary from then on.
    roster.publish(Vec::new());
    let text = std::fs::read_to_string(root.join(ROSTER_FILE)).unwrap();
    let row: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(row["driva"]["isolation_backend"], "bwrap");
    assert!(row["summary"].get("driva").is_none());
    std::fs::remove_dir_all(root).ok();
}
