use super::*;

/// The interaction's own override wins, then the merged launch policy, then
/// the templates' default; a blank command at any layer is no answer.
#[test]
fn a_project_command_resolves_from_the_most_specific_layer_that_states_one() {
    let driva = DrivaOptions {
        template_run: Some("cargo run".into()),
        template_test: Some("cargo test".into()),
        ..DrivaOptions::default()
    };
    let workspace = LaunchPolicy {
        run: Some("cargo run --release".into()),
        ..LaunchPolicy::default()
    };
    let resolve = |requested: Option<&str>, launch: &LaunchPolicy, action| {
        project_command(action, requested, launch, &driva)
    };

    assert_eq!(
        resolve(None, &workspace, ProjectAction::Run).as_deref(),
        Some("cargo run --release")
    );
    assert_eq!(
        resolve(None, &workspace, ProjectAction::Test).as_deref(),
        Some("cargo test")
    );
    assert_eq!(
        resolve(
            Some("  cargo run -- --help "),
            &workspace,
            ProjectAction::Run
        )
        .as_deref(),
        Some("cargo run -- --help")
    );
    assert_eq!(
        resolve(Some(" "), &LaunchPolicy::default(), ProjectAction::Run).as_deref(),
        Some("cargo run")
    );
    assert_eq!(
        project_command(
            ProjectAction::Test,
            None,
            &LaunchPolicy::default(),
            &DrivaOptions::default()
        ),
        None
    );
}
use crate::client::Client;
use crate::git::Git as _;
use crate::protocol::{AttributedMount, MountOrigin};
use driva::{Mount, MountAccess};

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("styra-server-{tag}-{}.sock", std::process::id(),))
}

#[test]
fn provider_switch_requires_a_stopped_interaction() {
    assert!(ensure_provider_switchable(InteractionActivity::Stopped).is_ok());
    for activity in [
        InteractionActivity::Running,
        InteractionActivity::Background,
        InteractionActivity::Pending,
    ] {
        assert!(ensure_provider_switchable(activity)
            .unwrap_err()
            .to_string()
            .contains("stop the interaction"));
    }
}

/// A turn is dated once, when it starts. Restating what the interaction is
/// already doing — a second operator message inside one running turn —
/// must not restart the clock, or the figure a client shows would fall
/// back to zero without the work having begun again.
#[test]
fn the_activity_clock_restarts_only_when_the_activity_actually_changes() {
    let activity = CurrentActivity::new();
    activity.set(InteractionActivity::Running, None);
    let started = activity.get().since_ms;

    std::thread::sleep(Duration::from_millis(2));
    activity.set(InteractionActivity::Running, None);
    let restated = activity.get();
    assert_eq!(restated.activity, InteractionActivity::Running);
    assert_eq!(restated.since_ms, started);

    std::thread::sleep(Duration::from_millis(2));
    activity.set(
        InteractionActivity::Pending,
        Some(InteractionActivityReason::TurnCompleted),
    );
    let idle = activity.get();
    assert_eq!(idle.activity, InteractionActivity::Pending);
    assert!(idle.since_ms > started, "a real change dates itself");
}

/// An interaction still working — a turn, or background work after one —
/// cannot be branched; one waiting for input or stopped can.
#[test]
fn only_an_interaction_at_rest_can_be_branched() {
    for working in [
        InteractionActivity::Running,
        InteractionActivity::Background,
    ] {
        let error = ensure_branchable("source", working).unwrap_err();
        assert!(error.to_string().contains("still working"), "{error:#}");
    }
    assert!(ensure_branchable("source", InteractionActivity::Pending).is_ok());
    assert!(ensure_branchable("source", InteractionActivity::Stopped).is_ok());
}

/// A new checkout is refused to an interaction still working, whichever
/// client asks: moving it there restarts it and loses the work under way.
#[test]
fn only_an_interaction_at_rest_can_be_moved_to_a_new_worktree() {
    for working in [
        InteractionActivity::Running,
        InteractionActivity::Background,
    ] {
        let error = ensure_movable("source", working).unwrap_err();
        assert!(error.to_string().contains("still working"), "{error:#}");
    }
    assert!(ensure_movable("source", InteractionActivity::Pending).is_ok());
    assert!(ensure_movable("source", InteractionActivity::Stopped).is_ok());
}

/// A background set reported empty answers for the state it was asked
/// about. A turn that has started since owns the interaction, and the late
/// answer must not put it back to waiting for input.
#[test]
fn an_empty_background_set_does_not_displace_a_turn_that_has_since_started() {
    let activity = CurrentActivity::new();
    activity.set(InteractionActivity::Running, None);

    assert!(!activity.replace_if(
        InteractionActivity::Background,
        InteractionActivity::Pending,
        Some(InteractionActivityReason::BackgroundFinished),
    ));
    assert_eq!(activity.activity(), InteractionActivity::Running);
    assert_eq!(
        activity.get().reason,
        None,
        "a refused transition states nothing about why"
    );

    activity.set(InteractionActivity::Background, None);
    assert!(activity.replace_if(
        InteractionActivity::Background,
        InteractionActivity::Pending,
        Some(InteractionActivityReason::BackgroundFinished),
    ));
    assert_eq!(activity.activity(), InteractionActivity::Pending);
    assert_eq!(
        activity.get().reason,
        Some(InteractionActivityReason::BackgroundFinished)
    );
}

/// The agent reports one ending for all of these, so the answer comes from
/// what the server noticed on the way to it — and the more specific
/// noticing wins: a turn a window refused never ran to be interrupted.
#[test]
fn a_turns_ending_is_read_from_what_the_server_noticed_during_it() {
    let refusal = InteractionActivityReason::RateLimited {
        window: "five_hour".into(),
        resets_at_ms: None,
    };

    assert_eq!(
        turn_end_reason(None, false, None),
        InteractionActivityReason::TurnCompleted
    );
    assert_eq!(
        turn_end_reason(None, true, None),
        InteractionActivityReason::Interrupted
    );
    assert_eq!(
        turn_end_reason(None, false, Some("context window exceeded".into())),
        InteractionActivityReason::Failed {
            message: "context window exceeded".into()
        }
    );
    assert_eq!(
        turn_end_reason(Some(refusal.clone()), true, Some("refused".into())),
        refusal
    );
}

/// Claude's refusal does not take its agent away: the CLI reports the
/// rate limit, the turn ends, and the session sits there idle and still
/// taking messages — "idle · after an error" once the operator's next ask
/// fails on the same spent window.
///
/// The quota log hands the window's reset over on time, which is why the
/// view at the bottom of the screen turns over to `-`. The interaction it
/// refused must come back with it; being alive is not being unrefused,
/// and an operator who asked to wait the window out gets nothing at all
/// if a live-but-refused session is passed over.
#[test]
fn a_refused_interaction_still_taking_messages_comes_back_with_its_window() {
    const CLAUDE_REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":
            {"status":"rejected","resetsAt":1788290400,"rateLimitType":"five_hour",
             "overageStatus":"rejected","overageDisabledReason":"out_of_credits",
             "isUsingOverage":false}}"#;

    // The refusal, as the wire stated it and the interaction recorded it.
    let log = crate::quota::QuotaLog::new();
    let refusal = log
        .observe("s-1", crate::agent::Provider::Claude, 1, CLAUDE_REJECTED)
        .rejected
        .expect("the line refused the work");

    // The agent survived it, so the turn merely ended. The operator asked
    // again, the same window failed that turn with an error, and the
    // interaction is left idle — accepting messages nothing will run.
    let activity = CurrentActivity::new();
    activity.set(
        InteractionActivity::Pending,
        Some(turn_end_reason(
            None,
            false,
            Some("Claude usage limit reached".into()),
        )),
    );
    assert_eq!(
        activity.get().reason,
        Some(InteractionActivityReason::Failed {
            message: "Claude usage limit reached".into()
        }),
        "the state the operator is looking at: idle, after an error"
    );

    // The window comes back, and says so: this is the reset the quota
    // view draws as turned over.
    // Well past the grace the log holds a reported reset for.
    let due = log.resets(refusal.resets_at_ms.expect("a reported reset") + 10 * 60 * 1_000);
    assert_eq!(due.len(), 1, "the refused window turned over");
    assert!(reset_releases(&refusal, &due[0]));

    assert_eq!(
        awaiting_window(true, activity.activity(), Some(&refusal)),
        Some(refusal.clone()),
        "a session told to wait the window out is asked again when it resets, \
             whether or not its agent outlived the refusal"
    );
}

/// The other side of asking a live session again: an interaction with a
/// turn under way is being served, whatever refused it earlier, and a
/// reset arriving mid-turn must not ask the same question beside it.
#[test]
fn a_turn_under_way_is_not_waiting_for_a_window() {
    let refusal = crate::protocol::QuotaEvent {
        at_ms: 1,
        session_id: "s-1".into(),
        provider: crate::agent::Provider::Claude,
        window: "five_hour".into(),
        status: crate::protocol::QuotaStatus::Exhausted,
        utilization: None,
        resets_at_ms: Some(1_788_290_400_000),
        detail: None,
    };

    for busy in [
        InteractionActivity::Running,
        InteractionActivity::Background,
    ] {
        assert_eq!(awaiting_window(true, busy, Some(&refusal)), None);
    }
    for waiting in [InteractionActivity::Pending, InteractionActivity::Stopped] {
        assert_eq!(
            awaiting_window(true, waiting, Some(&refusal)),
            Some(refusal.clone())
        );
        // Without the operator's standing answer there is nobody to ask
        // again for, whatever the window does.
        assert_eq!(awaiting_window(false, waiting, Some(&refusal)), None);
    }
}

/// A refusal is only interesting until the provider contradicts it, and it
/// is contradicted silently — by serving. Left standing, it would have the
/// next reset send a turn this session has long since had an answer to.
#[test]
fn a_provider_serving_again_retires_the_refusal_on_record() {
    const CLAUDE_ALLOWED: &str = r#"{"type":"rate_limit_event","rate_limit_info":
            {"status":"allowed","resetsAt":1788290400,"rateLimitType":"five_hour"}}"#;

    let log = crate::quota::QuotaLog::new();
    let observed = log.observe("s-1", crate::agent::Provider::Claude, 2, CLAUDE_ALLOWED);
    assert!(observed.rejected.is_none());
    assert!(
        observed.serving,
        "a reading exists because the provider answered a turn"
    );
}

/// Stopping an interaction is not the agent doing something else, so it
/// says why without claiming the turn moved — and without restarting the
/// clock that says how long it has been where it is.
#[test]
fn a_stop_states_its_reason_without_moving_the_activity() {
    let activity = CurrentActivity::new();
    activity.set(InteractionActivity::Running, None);
    let started = activity.get().since_ms;

    std::thread::sleep(Duration::from_millis(2));
    activity.note_reason(InteractionActivityReason::Paused);

    let state = activity.get();
    assert_eq!(state.activity, InteractionActivity::Running);
    assert_eq!(state.reason, Some(InteractionActivityReason::Paused));
    assert_eq!(state.since_ms, started);
}

#[test]
fn an_ending_preserves_its_cause_or_explains_itself() {
    let refusal = InteractionActivityReason::RateLimited {
        window: "five_hour".into(),
        resets_at_ms: None,
    };
    for (name, initial, prior, exit, expected) in [
        (
            "refusal caused exit",
            InteractionActivity::Running,
            refusal.clone(),
            Some(1),
            refusal,
        ),
        (
            "completed turn does not explain exit",
            InteractionActivity::Pending,
            InteractionActivityReason::TurnCompleted,
            Some(0),
            InteractionActivityReason::Exited { exit_code: Some(0) },
        ),
    ] {
        let activity = CurrentActivity::new();
        activity.set(initial, None);
        activity.note_reason(prior);
        activity.stopped(InteractionActivityReason::Exited { exit_code: exit });
        let state = activity.get();
        assert_eq!(state.activity, InteractionActivity::Stopped, "{name}");
        assert_eq!(state.reason, Some(expected), "{name}");
    }
}

/// Stopping is a transition like any other, so it is dated — how long an
/// interaction has been stopped is as much a question as how long a turn
/// has been running — and it is the one state that takes no messages.
#[test]
fn stopping_dates_itself_and_closes_the_interaction_to_messages() {
    let activity = CurrentActivity::new();
    activity.set(InteractionActivity::Running, None);
    let running_since = activity.get().since_ms;
    assert!(activity.activity().accepting());

    std::thread::sleep(Duration::from_millis(2));
    activity.stopped(InteractionActivityReason::Paused);

    let state = activity.get();
    assert!(state.since_ms > running_since);
    assert!(!state.activity.accepting());
}

/// The notification exists to send an operator somewhere they are not. An
/// interaction that finishes its turn on a client's screen was watched
/// finishing it, so there is nothing to send anyone to.
#[test]
fn going_idle_on_a_clients_screen_is_not_a_notification() {
    let idle = IdleNotice::new(false);

    idle.note_watched();
    idle.became_idle();
    assert!(!idle.unseen());

    // The same interaction, once no client is drawing it any more: this is
    // the case the notification is for.
    *idle.watched.lock().unwrap() = Some(Instant::now() - WATCHED_FOR);
    idle.became_idle();
    assert!(idle.unseen());
}

/// A client arriving at an interaction that went idle unwatched is the
/// operator answering the notification, whether it arrived through a load
/// or by the interaction becoming the client's current screen.
#[test]
fn a_watching_client_acknowledges_a_notification_raised_before_it() {
    let idle = IdleNotice::new(false);
    idle.became_idle();
    assert!(idle.unseen());

    idle.note_watched();

    assert!(!idle.unseen());
}

/// Read when the agent stops, and not before: a checkout the operator has
/// since committed still reads as it did at that moment, and a workspace
/// the agent dirtied mid-turn does not report until the turn is over.
#[test]
fn a_working_tree_is_read_when_the_interaction_stops() {
    let host = temp_path("working-tree-host");
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let tree = WorkingTree::new(git.clone(), host.clone());

    assert!(!tree.uncommitted(), "nothing has been read yet");

    git.set_uncommitted_changes(&host, true);
    assert!(!tree.uncommitted(), "the agent is still working");

    tree.reread();
    assert!(tree.uncommitted());

    git.set_uncommitted_changes(&host, false);
    assert!(tree.uncommitted(), "still what the turn left behind");
    tree.reread();
    assert!(!tree.uncommitted());
}

/// A turn's work is committed only where there is work and a repository
/// to commit it to, and committing it leaves the tree clean.
#[test]
fn a_working_tree_commits_only_work_that_is_there() {
    let host = temp_path("working-tree-commit-host");
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let tree = WorkingTree::new(git.clone(), host.clone());

    assert_eq!(tree.commit("Nothing").unwrap(), None, "a clean tree");

    git.set_uncommitted_changes(&host, true);
    assert!(tree.commit("Add the file").unwrap().is_some());
    assert_eq!(git.commits(&host), vec!["Add the file".to_owned()]);
    tree.reread();
    assert!(!tree.uncommitted(), "the commit took everything");

    let bare = temp_path("working-tree-commit-bare");
    std::fs::remove_dir_all(&bare).ok();
    std::fs::create_dir_all(&bare).unwrap();
    let outside = WorkingTree::new(crate::git::FakeGit::shared(), bare);
    assert_eq!(outside.commit("Anything").unwrap(), None, "no repository");
}

#[test]
fn session_creation_snapshots_the_prepared_checkout_after_inherited_history() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    let host = root.path().join("workspace");
    let checkout = root.path().join("checkout");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&checkout);
    git.set_uncommitted_changes(&checkout, true);
    let state = ServerState::with_git(git.clone(), store.clone());
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();

    let (journal, id, prepared_checkout) = state
        .create_session_record(&workspace.id, &profile, &selection, None, |journal, _| {
            journal.record_user_message("inherited turn")?;
            Ok(checkout.clone())
        })
        .unwrap();

    assert_eq!(prepared_checkout, checkout);
    assert_eq!(
        state.stored_summary(&id).unwrap().workspace_id,
        workspace.id
    );
    let snapshots = journal::read_snapshots(journal.path()).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].checkout, checkout);
    assert_eq!(snapshots[0].turn, Some(2));
    assert!(snapshots[0].diff.is_some());
    assert!(snapshot_base(git.as_ref(), journal.path())
        .unwrap()
        .is_some());
}

#[test]
fn initial_snapshot_is_recorded_once_and_keeps_its_dirty_baseline() {
    let root = tempfile::tempdir().unwrap();
    let host = root.path().join("checkout");
    let session = root.path().join("session");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::create_dir_all(&session).unwrap();
    Journal::create(&session).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    git.set_uncommitted_changes(&host, true);
    assert_eq!(snapshot_base(git.as_ref(), &session).unwrap(), None);
    ensure_initial_snapshot(
        git.as_ref(),
        &session,
        &host,
        crate::event::Protocol::CodexJsonl,
    )
    .unwrap();
    let snapshots = journal::read_snapshots(&session).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].turn, Some(1));
    assert!(snapshots[0].diff.is_some());
    let tree = WorkingTree::new(git.clone(), host.clone());
    assert_eq!(
        tree.snapshot(&session, journal::SnapshotMoment::BeforeMessage, 1)
            .unwrap(),
        None
    );
    let base = snapshot_base(git.as_ref(), &session).unwrap().unwrap();
    assert!(base.contains("fake diff of"), "{base}");

    git.commit_all(&host, "later work").unwrap();
    ensure_initial_snapshot(
        git.as_ref(),
        &session,
        &host,
        crate::event::Protocol::CodexJsonl,
    )
    .unwrap();
    assert_eq!(journal::read_snapshots(&session).unwrap(), snapshots);
    tree.snapshot(&session, journal::SnapshotMoment::TurnEnded, 1)
        .unwrap();
    assert!(
        snapshot_turn_diff(git.as_ref(), &session, 1)
            .unwrap()
            .unwrap()
            .ended
    );
    assert_eq!(snapshot_base(git.as_ref(), &session).unwrap(), Some(base));

    let outside = root.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let other_session = root.path().join("other-session");
    std::fs::create_dir_all(&other_session).unwrap();
    ensure_initial_snapshot(
        git.as_ref(),
        &other_session,
        &outside,
        crate::event::Protocol::CodexJsonl,
    )
    .unwrap();
    assert_eq!(snapshot_base(git.as_ref(), &other_session).unwrap(), None);
}

/// A snapshot names the commit and carries a diff only when there is
/// uncommitted work. One is always taken as a turn ends; one before a
/// message only when the checkout changed since the last, and none
/// outside a repository at all.
#[test]
fn a_working_tree_snapshot_records_the_commit_and_any_uncommitted_diff() {
    use journal::SnapshotMoment::{BeforeMessage, TurnEnded};
    let host = temp_path("working-tree-snapshot-host");
    let session = temp_path("working-tree-snapshot-session");
    for directory in [&host, &session] {
        std::fs::remove_dir_all(directory).ok();
        std::fs::create_dir_all(directory).unwrap();
    }
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let tree = WorkingTree::new(git.clone(), host.clone());

    let first = tree.snapshot(&session, BeforeMessage, 1).unwrap().unwrap();
    assert_eq!((first.sequence, first.moment), (1, BeforeMessage));
    assert!(first.commit.is_some());
    assert_eq!(first.diff, None, "a clean checkout has no diff");

    // The agent changes nothing: the turn's end is recorded all the same.
    let ended = tree.snapshot(&session, TurnEnded, 1).unwrap().unwrap();
    assert_eq!((ended.sequence, ended.moment), (2, TurnEnded));
    // And nothing happens before the next message, so it is not recorded.
    assert_eq!(tree.snapshot(&session, BeforeMessage, 2).unwrap(), None);

    // The operator edits the checkout between turns.
    git.set_uncommitted_changes(&host, true);
    let edited = tree.snapshot(&session, BeforeMessage, 2).unwrap().unwrap();
    assert_eq!(edited.sequence, 3);
    assert_eq!(edited.commit, first.commit);
    let diff = std::fs::read_to_string(session.join(edited.diff.unwrap())).unwrap();
    assert!(diff.starts_with("fake diff of"), "{diff}");
    assert_eq!(tree.snapshot(&session, BeforeMessage, 2).unwrap(), None);

    // A turn whose work was committed ends on a clean checkout.
    git.commit_all(&host, "Turn").unwrap();
    let committed = tree.snapshot(&session, TurnEnded, 2).unwrap().unwrap();
    assert_eq!(committed.diff, None);
    assert_eq!(journal::read_snapshots(&session).unwrap().len(), 4);

    // Turn 1 changed nothing; turn 2 started from the operator's edit.
    let diff = |turn| snapshot_turn_diff(git.as_ref(), &session, turn).unwrap();
    let first = diff(1).unwrap();
    assert_eq!((first.diff.as_str(), first.ended), ("", true));
    let second = diff(2).unwrap();
    assert!(second.ended);
    assert!(
        second.diff.starts_with("fake diff from tree("),
        "{second:?}"
    );
    assert!(
        second.diff.contains("fake diff of"),
        "from the edited tree: {second:?}"
    );

    // Turn 3 is running: read from turn 2's end to the checkout now,
    // without recording anything.
    let running = diff(3).unwrap();
    assert_eq!((running.diff.as_str(), running.ended), ("", false));
    git.set_uncommitted_changes(&host, true);
    let running = diff(3).unwrap();
    assert!(!running.ended);
    assert!(running.diff.contains("fake diff of"), "{running:?}");
    assert_eq!(journal::read_snapshots(&session).unwrap().len(), 4);
    git.set_uncommitted_changes(&host, false);

    let bare = temp_path("working-tree-snapshot-bare");
    std::fs::remove_dir_all(&bare).ok();
    std::fs::create_dir_all(&bare).unwrap();
    let outside = WorkingTree::new(crate::git::FakeGit::shared(), bare);
    assert_eq!(
        outside.snapshot(&session, TurnEnded, 1).unwrap(),
        None,
        "no repository"
    );
    assert_eq!(journal::read_snapshots(&session).unwrap().len(), 4);
}

/// The subject is the start of what the agent said last; the body is the
/// whole turn, the operator's message first.
#[test]
fn a_turn_is_committed_under_its_last_message_with_the_whole_turn_below() {
    let replies = vec![
        "Looking at the tests.".to_owned(),
        "## Fixed the flaky test\n\nIt raced the clock.".to_owned(),
    ];
    let message = turn_commit_message(Some("Fix the flaky test"), &replies);
    assert_eq!(
        message,
        "Fixed the flaky test\n\n\
             User:\nFix the flaky test\n\n\
             Agent:\nLooking at the tests.\n\n## Fixed the flaky test\n\nIt raced the clock.\n"
    );
}

/// A branch's last message is its own: what it inherited before its
/// `branched from` marker is the source's, so a branch that has not
/// answered yet has none, and one that has says only its own.
#[test]
fn a_branchs_last_message_is_never_its_sources() {
    let mut updates = Vec::new();
    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::AgentMessage {
            text: "The source's answer".into(),
        }),
    );
    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::Branched {
            direction: crate::event::BranchDirection::From,
            session: "source".into(),
            name: Some("Source".into()),
        }),
    );
    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::UserMessage {
            text: "Try it another way".into(),
        }),
    );
    assert_eq!(last_message(&updates), None);

    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::AgentMessage {
            text: "The branch's answer".into(),
        }),
    );
    assert_eq!(
        last_message(&updates).as_deref(),
        Some("The branch's answer")
    );
}

/// Both sides of the conversation, oldest first and as written, with the
/// tool traffic between them left out and the operator's message unframed.
/// A branch marker is kept, by the other Session's id alone.
#[test]
fn recent_messages_are_the_conversations_tail() {
    let mut updates = Vec::new();
    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::UserMessage {
            text: crate::contract::frame("Fix the\nflaky test", Contract::Text),
        }),
    );
    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::CommandStarted {
            command: "cargo test".into(),
        }),
    );
    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::AgentMessage {
            text: "It raced **the clock**.\n".into(),
        }),
    );
    push_sequenced(
        &mut updates,
        InteractionUpdate::Event(crate::event::AgentEvent::Branched {
            direction: crate::event::BranchDirection::To,
            session: "retry-backoff".into(),
            name: Some("Retry backoff".into()),
        }),
    );
    assert_eq!(
        recent_messages(&updates),
        [
            RecentMessage {
                from_operator: true,
                text: "Fix the\nflaky test".into(),
                contract: Some(Contract::Text),
                branch: None,
            },
            RecentMessage {
                from_operator: false,
                text: "It raced **the clock**.".into(),
                contract: None,
                branch: None,
            },
            RecentMessage {
                from_operator: false,
                text: String::new(),
                contract: None,
                branch: Some(RecentBranch {
                    direction: crate::event::BranchDirection::To,
                    session: "retry-backoff".into(),
                }),
            },
        ]
    );
}

#[test]
fn a_recent_message_is_clipped_at_its_end() {
    let long = "word\n".repeat(400);
    let text = clipped(&long);
    assert_eq!(text.chars().count(), 1501);
    assert!(text.starts_with("word\nword"));
    assert!(text.ends_with('…'));
}

#[test]
fn a_commit_subject_is_one_clipped_line() {
    let long = "word ".repeat(40);
    let subject = commit_subject(&long).unwrap();
    assert_eq!(subject.chars().count(), 72);
    assert!(subject.ends_with('…'));
    assert_eq!(commit_subject("\n  \n").as_deref(), None);
    // Nothing said in the turn: the prompt names the work instead.
    assert_eq!(
        turn_commit_message(Some("Rename it"), &[]),
        "Rename it\n\nUser:\nRename it\n"
    );
    assert_eq!(
        turn_commit_message(None, &[]),
        "Work left by an agent turn\n"
    );
}

/// An operator arriving at an interaction that left work uncommitted is
/// usually arriving to commit it. The claim is checked again just after
/// they get there, so it stops being made once it stops being true — and
/// no check is scheduled where there is no claim to correct.
#[test]
fn focusing_an_interaction_rereads_work_it_left_behind() {
    let host = temp_path("working-tree-focus-host");
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let tree = Arc::new(WorkingTree::new(git.clone(), host.clone()));

    // Nothing left behind, nothing to go and look at again.
    tree.recheck_after(Duration::from_millis(0));
    git.set_uncommitted_changes(&host, true);
    std::thread::sleep(Duration::from_millis(50));
    assert!(!tree.uncommitted(), "no reading was scheduled");

    tree.reread();
    assert!(tree.uncommitted(), "the turn left work behind");

    // The operator arrives and commits it.
    git.set_uncommitted_changes(&host, false);
    tree.recheck_after(Duration::from_millis(0));
    for _ in 0..200 {
        if !tree.uncommitted() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!tree.uncommitted(), "the checkout was read again");
}

/// A workspace outside a repository has no answer to give, and the
/// question failing is not something to report at an operator.
#[test]
fn a_workspace_outside_a_repository_reports_nothing() {
    let host = temp_path("working-tree-bare-host");
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let tree = WorkingTree::new(crate::git::FakeGit::shared(), host);

    tree.reread();

    assert!(!tree.uncommitted());
    assert_eq!(tree.checkout_state(), None);
}

#[test]
fn polling_detects_branch_switches_without_an_idle_transition() {
    let directory = tempfile::tempdir().unwrap();
    let run = |path: &Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(directory.path(), &["init", "-b", "main"]);
    run(
        directory.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    );
    let linked = directory.path().join("linked");
    run(
        directory.path(),
        &[
            "worktree",
            "add",
            "-b",
            "assigned",
            linked.to_str().unwrap(),
        ],
    );

    for (path, original) in [(directory.path(), "main"), (linked.as_path(), "assigned")] {
        let tree = WorkingTree::new(Arc::new(crate::git::SystemGit), path.to_owned());
        run(path, &["checkout", "-B", "other"]);
        assert_eq!(
            tree.checkout_state().unwrap().branch.as_deref(),
            Some(original),
            "polls within two seconds use the cache"
        );
        for target in ["other", "--detach", original] {
            if target != "other" {
                run(path, &["checkout", target]);
            }
            *tree.branch_checked.lock().unwrap() = Instant::now() - Duration::from_secs(3);
            let expected = (target != "--detach").then_some(target);
            assert_eq!(tree.checkout_state().unwrap().branch.as_deref(), expected);
        }
    }
}

/// Where the work is happening, unlike the work left behind beside it, is
/// known before the agent has done anything: the directory is the one the
/// launch resolved, and Git can be asked about it straight away. So it is
/// answered from the moment the interaction exists, and again whenever the
/// interaction stops.
#[test]
fn a_working_tree_records_the_branch_it_stopped_on() {
    let host = temp_path("working-tree-branch-host");
    std::fs::remove_dir_all(&host).ok();
    let git = Arc::new(crate::git::FakeGit::new());
    let repository = git.init(&host);
    let tree = WorkingTree::new(git.clone(), host.clone());

    assert!(
        tree.checkout_state().is_some(),
        "where the agent is working needs no turn to have ended"
    );

    tree.reread();

    let state = tree.checkout_state().expect("the workspace is a checkout");
    assert_eq!(state.branch.as_deref(), Some("main"));
    assert_eq!(state.worktree, repository.root);
    // A main checkout is its own repository, which is what makes the
    // linked-worktree question answerable without a second field.
    assert_eq!(state.repository, repository.root);
    assert!(!state.linked());
}

/// An interaction given its own linked worktree reports that worktree and
/// its branch, and names the repository the branch will turn up in — which
/// the worktree path, parked wherever Styra keeps them, does not say.
#[test]
fn a_linked_worktree_names_the_repository_it_branches_from() {
    let host = temp_path("working-tree-linked-host");
    std::fs::remove_dir_all(&host).ok();
    let git = Arc::new(crate::git::FakeGit::new());
    let repository = git.init(&host.join("repository"));
    let worktree = host.join("worktrees").join("styra-session");
    git.create_worktree(&repository.root, "styra/session", &worktree)
        .unwrap();
    let tree = WorkingTree::new(git.clone(), worktree.clone());

    tree.reread();

    let state = tree.checkout_state().expect("the worktree is a checkout");
    assert_eq!(state.branch.as_deref(), Some("styra/session"));
    assert_eq!(state.worktree, worktree.canonicalize().unwrap());
    assert_eq!(state.repository, repository.root);
    assert!(state.linked());
}

/// The reading survives the repository becoming unreadable. Where the work
/// was is still the best answer available, and an operator shown nothing
/// would read it as "not a checkout" rather than "could not look".
#[test]
fn a_checkout_that_goes_away_leaves_the_last_reading_standing() {
    let host = temp_path("working-tree-vanished-host");
    std::fs::remove_dir_all(&host).ok();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let tree = WorkingTree::new(git.clone(), host.clone());
    tree.reread();
    assert!(tree.checkout_state().is_some());

    std::fs::remove_dir_all(&host).unwrap();
    tree.reread();

    assert_eq!(
        tree.checkout_state()
            .and_then(|state| state.branch)
            .as_deref(),
        Some("main")
    );
}

#[test]
fn workspace_worktrees_are_not_prepared_until_enabled() {
    let store = temp_path("optional-worktrees-store");
    let host = temp_path("optional-worktrees-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let state = ServerState::with_git(git, store.clone());
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let worktrees_path = crate::workspace::worktrees_dir(&store, &workspace.id);

    assert!(state
        .workspace_worktrees(&workspace, false)
        .unwrap()
        .is_none());
    assert!(!worktrees_path.exists());

    assert!(state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .is_some());
    assert!(worktrees_path.is_dir());

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

fn listed_interaction(
    id: &str,
    workspace_id: &str,
    directory: &Path,
    activity: InteractionActivity,
) -> InteractionSummary {
    InteractionSummary {
        id: id.into(),
        name: None,
        tags: Vec::new(),
        workspace_id: workspace_id.into(),
        selection: Selection::new(crate::agent::Provider::Codex),
        workspace: directory.to_path_buf(),
        activity,
        activity_reason: None,
        activity_since_ms: 1,
        idle_unseen: false,
        interacted_at_ms: 0,
        last_user_message_at_ms: 0,
        uncommitted_changes: false,
        expected_branch: None,
        checkout: None,
        branched_from: None,
        last_message: None,
        recent_messages: Vec::new(),
        auto_retry: false,
        auto_commit: false,
        events: 0,
        completed: CompletionState::Active,
        focus_requested: false,
        origin: None,
        integration: None,
        restored: false,
    }
}

/// A Session on a repository, with no interaction: enough durable state
/// for the checkout questions, which are decided before an agent starts.
struct StoredSession {
    state: ServerState,
    git: Arc<crate::git::FakeGit>,
    store: PathBuf,
    host: PathBuf,
    workspace: crate::protocol::WorkspaceSummary,
    id: String,
    session_path: PathBuf,
    _directory: tempfile::TempDir,
}

fn stored_session(tag: &str) -> StoredSession {
    let directory = tempfile::Builder::new().prefix(tag).tempdir().unwrap();
    let store = directory.path().join("store");
    let host = directory.path().join("host");
    std::fs::create_dir(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let state = ServerState::with_git(git.clone(), store.clone());
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    let (journal, id) =
        Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None).unwrap();
    let session_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);
    StoredSession {
        state,
        git,
        store,
        host,
        workspace,
        id,
        session_path,
        _directory: directory,
    }
}

#[test]
fn a_request_cannot_mutate_when_its_intent_cannot_be_stored() {
    let StoredSession {
        store,
        state,
        id,
        _directory,
        ..
    } = stored_session("action-intent-failure");
    std::fs::create_dir(store.join("actions.jsonl")).unwrap();
    let request = Request::RenameSession(crate::protocol::RenameSession {
        id: id.clone(),
        name: Some("must not be applied".into()),
    });
    assert!(state.handle(request).is_err());
    assert_eq!(state.stored_summary(&id).unwrap().name, None);
    drop(state);
}

#[test]
fn action_history_routes_workspace_edits_and_records_failed_automatic_actions() {
    use crate::protocol::{RenameSession, WorkspaceLaunchChange};
    let StoredSession {
        store,
        host,
        state,
        workspace,
        id,
        _directory,
        ..
    } = stored_session("action-history");
    let other_host = host.join("other");
    std::fs::create_dir_all(&other_host).unwrap();
    let other_workspace = crate::workspace::create(&store, &other_host, None).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    let (_, sibling) =
        Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None).unwrap();
    let (_, unrelated) =
        Journal::create_in_workspace(&store, &other_workspace.id, &profile, &selection, None)
            .unwrap();
    let requests = vec![
        Request::ChangeWorkspaceLaunch {
            workspace_id: workspace.id.clone(),
            change: WorkspaceLaunchChange::SetTemplates(vec!["git".into()]),
        },
        Request::ChangeWorkspaceLaunch {
            workspace_id: workspace.id.clone(),
            change: WorkspaceLaunchChange::AddMounts(vec![LaunchMount {
                source: host.clone(),
                destination: None,
                writable: false,
            }]),
        },
        Request::RenameSession(RenameSession {
            id: id.clone(),
            name: Some("renamed".into()),
        }),
        Request::CreateSessionWorktree { id: id.clone() },
    ];
    for request in &requests {
        state.handle(request.clone()).unwrap();
    }
    let failed = Request::SendMessage {
        id: id.clone(),
        message: SendMessage::new("continue"),
    };
    assert!(state
        .handle_action(failed.clone(), ActionOrigin::Automatic)
        .is_err());
    let history = state.inner.actions.for_session(&id).unwrap();
    assert_eq!(history.len(), 10);
    for (records, request) in history[..8].chunks_exact(2).zip(&requests) {
        assert_eq!(records[0].action, Action::Request(request.clone()));
        assert_eq!(records[0].id, records[1].id);
        assert_eq!(records[0].status, ActionStatus::Started);
        assert_eq!(records[1].status, ActionStatus::Succeeded);
    }
    assert_eq!(history[9].action, Action::Request(failed));
    assert_eq!(history[9].origin, ActionOrigin::Automatic);
    assert_eq!(history[9].status, ActionStatus::Failed);
    assert!(history[9].detail.is_some());
    assert_eq!(state.inner.actions.for_session(&sibling).unwrap().len(), 4);
    assert!(state
        .inner
        .actions
        .for_session(&unrelated)
        .unwrap()
        .is_empty());
    drop(state);
    let reopened = ServerState::with_git(Arc::new(crate::git::FakeGit::new()), store.clone());
    let client = crate::daemon::in_process_client(reopened);
    assert_eq!(client.interaction_actions(&id).unwrap(), history);
    assert_eq!(
        client.interaction_actions(&id).unwrap(),
        history,
        "reading history must not audit itself"
    );
    drop(client);
}

/// `W` may be pressed after a Session has already received several turns.
/// Its linked checkout is still named for the opening task, just like a
/// checkout requested together with that opening prompt.
#[test]
fn an_existing_sessions_worktree_is_named_for_its_first_prompt() {
    let StoredSession {
        store,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("name-existing-worktree");
    let mut journal = Journal::open(&session_path).unwrap();
    journal
        .record_user_message("Fix the flaky checkout test")
        .unwrap();
    journal
        .record_user_message("Then add a regression test")
        .unwrap();
    drop(journal);

    state.create_session_worktree(&id).unwrap();

    let checkout = journal::read_session_checkout(&session_path)
        .unwrap()
        .expect("the new checkout is recorded");
    let expected_suffix = format!("-{id}");
    let path = checkout
        .path
        .expect("a checkout just made has its directory");
    let name = path.file_name().unwrap().to_string_lossy();
    assert!(name.ends_with(&expected_suffix), "{name}");
    assert_ne!(name, id, "the first prompt supplies a branch topic");
    assert_eq!(
        checkout.branch,
        format!("styra/{name}"),
        "the branch and checkout use the same errand-derived name"
    );
    assert_eq!(
        path.parent(),
        Some(crate::workspace::worktrees_dir(&store, &workspace.id).as_path())
    );
}

/// A cleanup pass takes back the checkouts of the Sessions the operator
/// is done with and has committed, and nothing else: work that is still
/// uncommitted stays exactly where the agent left it, and a Session that
/// is still open keeps the directory it is working in.
///
/// What every one of them keeps is the branch — that is the whole bargain
/// the pass makes, so it is asserted for the Session whose directory was
/// actually deleted.
#[test]
fn cleaning_up_takes_the_finished_checkouts_and_leaves_every_branch() {
    let store = temp_path("clean-worktrees-store");
    let host = temp_path("clean-worktrees-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let state = ServerState::with_git(git.clone(), store.clone());
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    // One Session per outcome, each with a checkout of its own.
    let session = |completed: CompletionState| {
        let (journal, id) =
            Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None)
                .unwrap();
        let path = journal.path().parent().unwrap().to_path_buf();
        drop(journal);
        state.create_session_worktree(&id).unwrap();
        journal::store_session_completed(&path, completed).unwrap();
        let checkout = journal::read_session_checkout(&path).unwrap().unwrap();
        (id, path, checkout)
    };
    let (finished, finished_path, finished_checkout) = session(CompletionState::Completed);
    let (sealed, sealed_path, sealed_checkout) = session(CompletionState::Sealed);
    let (dirty, dirty_path, dirty_checkout) = session(CompletionState::Completed);
    let (open, open_path, open_checkout) = session(CompletionState::Active);
    // A fifth, finished and clean, but recording the open Session's
    // checkout the way a Session launched from another one does.
    let (journal, shared) =
        Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None).unwrap();
    let shared_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);
    journal::store_session_checkout(&shared_path, &open_checkout).unwrap();
    journal::store_session_completed(&shared_path, CompletionState::Completed).unwrap();
    let directory = |checkout: &crate::worktree::Checkout| {
        checkout.path.clone().expect("the checkout was just made")
    };
    git.set_uncommitted_changes(&directory(&dirty_checkout), true);

    let cleaned = state.clean_worktrees(None).unwrap();

    let outcome = |id: &str| {
        cleaned
            .iter()
            .find(|entry| entry.session_id == id)
            .map(|entry| entry.outcome.clone())
    };
    assert_eq!(outcome(&finished), Some(WorktreeCleanup::Removed));
    assert_eq!(outcome(&sealed), Some(WorktreeCleanup::Removed));
    assert_eq!(outcome(&dirty), Some(WorktreeCleanup::Uncommitted));
    assert_eq!(
        outcome(&open),
        None,
        "a Session the operator has not finished with is not considered"
    );
    assert_eq!(
        outcome(&shared),
        Some(WorktreeCleanup::Shared {
            session_id: open.clone()
        }),
        "a checkout another Session is still working in is not taken"
    );

    // The two finished checkouts are gone, and the Sessions now record
    // their branches alone — which are still branches.
    for (path, checkout) in [
        (&finished_path, &finished_checkout),
        (&sealed_path, &sealed_checkout),
    ] {
        assert!(!directory(checkout).exists());
        assert_eq!(
            journal::read_session_checkout(path).unwrap(),
            Some(checkout.without_worktree())
        );
        assert!(git.has_branch(&checkout.branch));
    }
    // The other two are untouched, record included.
    for (path, checkout) in [(&dirty_path, &dirty_checkout), (&open_path, &open_checkout)] {
        assert!(directory(checkout).is_dir());
        assert_eq!(
            journal::read_session_checkout(path).unwrap(),
            Some(checkout.clone())
        );
    }

    // And a second pass has nothing left to say about the ones it took.
    let again = state.clean_worktrees(None).unwrap();
    let mut said_about = again
        .iter()
        .map(|entry| entry.session_id.clone())
        .collect::<Vec<_>>();
    said_about.sort();
    let mut kept = vec![dirty, shared];
    kept.sort();
    assert_eq!(said_about, kept);

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// Each checkout is listed once with every Session that records it, a
/// cleaned-up one by its branch, and a directory nothing records with no
/// Sessions at all — and listing changes none of it.
#[test]
fn listing_worktrees_groups_the_sessions_that_share_a_checkout() {
    let store = temp_path("list-worktrees-store");
    let host = temp_path("list-worktrees-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let state = ServerState::with_git(git.clone(), store.clone());
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    let session = || {
        let (journal, id) =
            Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None)
                .unwrap();
        let path = journal.path().parent().unwrap().to_path_buf();
        drop(journal);
        (id, path)
    };
    let (owner, owner_path) = session();
    state.create_session_worktree(&owner).unwrap();
    let checkout = journal::read_session_checkout(&owner_path)
        .unwrap()
        .unwrap();
    // Launched from the owner's checkout, so recording the same one.
    let (sharer, sharer_path) = session();
    journal::store_session_checkout(&sharer_path, &checkout).unwrap();
    journal::store_session_completed(&sharer_path, CompletionState::Completed).unwrap();
    // Finished and cleaned up: a branch with no directory.
    let (cleaned, cleaned_path) = session();
    state.create_session_worktree(&cleaned).unwrap();
    journal::store_session_completed(&cleaned_path, CompletionState::Completed).unwrap();
    state.clean_worktrees(None).unwrap();
    let cleaned_branch = journal::read_session_checkout(&cleaned_path)
        .unwrap()
        .unwrap()
        .branch;
    // One with no worktree at all is not listed.
    let (_plain, _) = session();
    let stray = crate::workspace::worktrees_dir(&store, &workspace.id).join("stray");
    std::fs::create_dir_all(&stray).unwrap();

    let listed = state.list_worktrees(Some(&workspace.id)).unwrap();

    let ids = |summary: &WorktreeSummary| {
        let mut ids = summary
            .sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };
    assert_eq!(listed.len(), 3, "{listed:#?}");
    let shared = listed
        .iter()
        .find(|summary| summary.worktree == checkout.path)
        .expect("the shared checkout is listed");
    let mut both = vec![owner.clone(), sharer.clone()];
    both.sort();
    assert_eq!(ids(shared), both);
    assert_eq!(shared.branch.as_ref(), Some(&checkout.branch));
    assert!(shared.exists);
    let gone = listed
        .iter()
        .find(|summary| summary.worktree.is_none())
        .expect("the cleaned-up branch is listed");
    assert_eq!(gone.branch.as_ref(), Some(&cleaned_branch));
    assert_eq!(ids(gone), vec![cleaned.clone()]);
    assert!(!gone.exists);
    let unrecorded = listed
        .iter()
        .find(|summary| summary.worktree.as_ref() == Some(&stray))
        .expect("a directory nothing records is listed");
    assert!(unrecorded.sessions.is_empty());
    assert_eq!(unrecorded.branch, None);
    assert!(stray.is_dir(), "listing removes nothing");

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// Sealing is only final if nothing starts the Session again, so the
/// resume is refused outright rather than allowed to quietly clear the
/// seal the way it clears an ordinary completion. Refused inside
/// `resume_session`, which is the one path to a live agent — the request
/// handler and the unattended retry after a plan window resets both go
/// through it — and refused before anything is launched.
#[test]
fn a_sealed_session_cannot_be_resumed() {
    let StoredSession {
        state,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("sealed-resume");

    // Completed is not sealed: it is refused for wanting a native
    // transcript this bare journal has never had, which is proof the
    // completion itself did not stop it.
    state
        .handle(Request::SetSessionCompleted {
            id: id.clone(),
            completed: CompletionState::Completed,
        })
        .unwrap();
    let error = state
        .resume_session(ResumeSession {
            id: id.clone(),
            launch: LaunchPolicy::default(),
            selection: None,
        })
        .unwrap_err();
    assert!(
        !error.to_string().contains("sealed"),
        "a merely completed session is not refused for being sealed: {error:#}"
    );

    state
        .handle(Request::SetSessionCompleted {
            id: id.clone(),
            completed: CompletionState::Sealed,
        })
        .unwrap();
    let error = state
        .resume_session(ResumeSession {
            id: id.clone(),
            launch: LaunchPolicy::default(),
            selection: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("sealed"), "{error:#}");

    // And the refusal left the seal alone: a resume that got as far as
    // clearing completion would have unsealed it on the way to failing.
    assert_eq!(
        journal::read_session_completed(&session_path).unwrap(),
        CompletionState::Sealed
    );
}

/// Completion belongs to the stored Session, so the stored-sessions
/// picker can mark one this server never ran — there is no live
/// interaction behind such a row, and demanding one made the whole view's
/// `C` fail.
#[test]
fn a_stored_session_is_completed_without_an_interaction_behind_it() {
    let StoredSession {
        state,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("complete-stored");

    assert_eq!(
        journal::read_session_completed(&session_path).unwrap(),
        CompletionState::Active
    );

    // Completion and abandonment are reversible; sealing is final.
    for completed in [
        CompletionState::Completed,
        CompletionState::Active,
        CompletionState::Abandoned,
        CompletionState::Active,
        CompletionState::Sealed,
    ] {
        state
            .handle(Request::SetSessionCompleted {
                id: id.clone(),
                completed,
            })
            .expect("marking a stored Session needs no live interaction");
        assert_eq!(
            journal::read_session_completed(&session_path).unwrap(),
            completed
        );
    }
    assert!(
        state
            .handle(Request::SetSessionCompleted {
                id,
                completed: CompletionState::Active,
            })
            .is_err(),
        "a sealed session cannot be un-sealed"
    );
}

/// Where a Session was working is said on the wire and written down
/// nowhere else, so reopening one that moved — a Codex told to work
/// elsewhere, a Claude Code that checked out a worktree of its own — means
/// reading it back out of the journal. Without that it comes back naming
/// the directory it was launched in and keeps naming it until the agent
/// mentions a directory again, which for a stopped Session is never.
#[test]
fn a_reopened_session_stands_where_it_was_working() {
    let StoredSession {
        host,
        state,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("replayed-cwd");
    let worktree = host.join(".worktrees/audio");
    {
        let mut journal = Journal::open(&session_path).unwrap();
        journal
            .record_agent_line(&serde_json::json!({ "result": { "cwd": worktree } }).to_string())
            .unwrap();
    }

    let Response::StoredSession(stored) = state
        .handle(Request::StoredSession {
            id: id.clone(),
            raw: false,
        })
        .unwrap()
    else {
        panic!("expected a stored session");
    };
    assert_eq!(stored.working_directory, Some(worktree));

    // The replayed stream ends standing there too, which is what a client
    // attaching to a restored row applies.
    let updates = replayed_session_updates(
        &session_path,
        crate::event::Protocol::CodexAppServer,
        WorkspaceMount {
            host: &host,
            sandbox: &host,
        },
    )
    .unwrap();
    assert!(matches!(
        updates.last().map(|update| &update.update),
        Some(InteractionUpdate::WorkingDirectoryChanged(directory))
            if directory == &host.join(".worktrees/audio")
    ));
}

/// Where the work is happening is two rows, and they have to agree. The
/// directory follows the agent — a Claude sent into a worktree of the
/// project, a Codex told to work elsewhere — and the branch beside it has
/// to follow the same report, or an interaction whose Workspace makes no
/// worktrees names the worktree the agent went to and, beside it, the
/// branch of the Workspace directory it left.
#[test]
fn the_branch_follows_the_agent_out_of_the_directory_it_launched_in() {
    let StoredSession {
        host,
        state: server,
        session_path,
        _directory,
        ..
    } = stored_session("moved-branch");
    let worktree = host.join(".worktrees/audio");
    server
        .inner
        .git
        .create_worktree(&host, "styra/audio", &worktree)
        .unwrap();
    {
        let mut journal = Journal::open(&session_path).unwrap();
        journal
            .record_agent_line(&serde_json::json!({ "result": { "cwd": worktree } }).to_string())
            .unwrap();
    }

    // The server knows where the agent went: this is the directory the
    // working-directory row is drawn from, live and on reopening.
    let moved = replayed_working_directory(
        &session_path,
        crate::event::Protocol::CodexAppServer,
        WorkspaceMount {
            host: &host,
            sandbox: &host,
        },
    )
    .expect("the journal says the agent moved");
    assert_eq!(moved, worktree);

    // The working tree starts where the launch put it — this Workspace
    // makes no worktrees, so that is the Workspace directory — and is told
    // about the move by the same update, which is what the collector
    // thread does with one.
    let tree = WorkingTree::new(Arc::clone(&server.inner.git), host.clone());
    assert_eq!(
        tree.checkout_state()
            .and_then(|state| state.branch)
            .as_deref(),
        Some("main"),
        "the interaction begins in the directory it was launched in"
    );
    tree.moved_to(moved.clone());
    let reported = tree.checkout_state().expect("the workspace is a checkout");

    assert_eq!(
        reported.worktree,
        moved.canonicalize().unwrap(),
        "the checkout reported is the one the agent is working in"
    );
    assert_eq!(reported.branch.as_deref(), Some("styra/audio"));

    // And a resume starts its working tree there rather than at the
    // Session's own checkout, which is what the replayed stream leaves the
    // client standing on.
    let seeded = replayed_session_updates(
        &session_path,
        crate::event::Protocol::CodexAppServer,
        WorkspaceMount {
            host: &host,
            sandbox: &host,
        },
    )
    .unwrap();
    assert_eq!(seeded_working_directory(&seeded), Some(worktree));
}

/// A checkout is only ever read when an interaction stops working, and the
/// roster is only ever mirrored when the list of interactions changes — at
/// a launch, a resume, or a close. An interaction resumed and not yet idle
/// is therefore mirrored with nothing, and the row the next run restores
/// says not that the server has not looked, but that there is no
/// repository at all — while the Session on disk records exactly which
/// checkout it works in.
#[test]
fn entering_a_restored_row_still_says_which_checkout_it_works_in() {
    let store = temp_path("restored-checkout-store");
    let host = temp_path("restored-checkout-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let worktree = host.join(".worktrees/audio");
    git.create_worktree(&host, "styra/audio", &worktree)
        .unwrap();

    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    let (journal, id) =
        Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None).unwrap();
    let session_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);
    journal::store_session_checkout(
        &session_path,
        &crate::worktree::Checkout::at(worktree.clone()),
    )
    .unwrap();

    // The row the previous run left: mirrored at the resume, before the
    // interaction had gone idle even once, so it carries no reading.
    let driva = DrivaOptions {
        working_directory: host.clone(),
        ..Default::default()
    };
    crate::roster::Roster::open(&store).publish(vec![(
        session_path.clone(),
        InteractionSummary {
            selection,
            ..listed_interaction(&id, &workspace.id, &worktree, InteractionActivity::Running)
        },
        driva,
    )]);

    let state = ServerState::with_git(git, store.clone());
    let Response::InteractionLoaded(loaded) =
        state.handle(Request::LoadInteraction { id }).unwrap()
    else {
        panic!("expected the restored row to open");
    };

    let checkout = loaded
        .summary
        .checkout
        .expect("entering after a restart still says where the work is");
    assert_eq!(checkout.branch.as_deref(), Some("styra/audio"));
    assert_eq!(checkout.worktree, worktree.canonicalize().unwrap());

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// A Session that never left its Workspace directory reports no move: the
/// client already stands at the root of the mount when it opens one, and
/// an update restating that is noise in every Session that behaved.
#[test]
fn a_session_that_stayed_put_reports_no_directory() {
    let StoredSession {
        host,
        state,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("unmoved-cwd");
    {
        let mut journal = Journal::open(&session_path).unwrap();
        journal
            .record_agent_line(&serde_json::json!({ "result": { "cwd": host } }).to_string())
            .unwrap();
        // A directory outside the Workspace mount is not the client's to be
        // shown either, for the reason the live reader will not show one.
        journal
            .record_agent_line(&serde_json::json!({ "result": { "cwd": "/etc" } }).to_string())
            .unwrap();
    }

    let Response::StoredSession(stored) = state
        .handle(Request::StoredSession { id, raw: false })
        .unwrap()
    else {
        panic!("expected a stored session");
    };
    assert_eq!(stored.working_directory, None);
    assert!(!replayed_session_updates(
        &session_path,
        crate::event::Protocol::CodexAppServer,
        WorkspaceMount {
            host: &host,
            sandbox: &host,
        },
    )
    .unwrap()
    .iter()
    .any(|update| matches!(update.update, InteractionUpdate::WorkingDirectoryChanged(_))));
}

/// The Session states which checkout it works in, so a resume asks the
/// Session rather than the shape of a directory name. Without it a
/// checkout named after its first prompt is not recognised, and the
/// Session comes back mounted on the operator's own directory with its
/// branch and its uncommitted work left behind.
#[test]
fn a_session_records_the_checkout_it_was_given() {
    let StoredSession {
        store,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("records-checkout");

    // Before anything is checked out there is nothing to report, and
    // asking must not be what brings the parent directory into being.
    assert_eq!(
        state
            .session_checkout(&session_path, &workspace.id, &id)
            .unwrap(),
        None
    );
    assert!(!crate::workspace::worktrees_dir(&store, &workspace.id).exists());

    let worktrees = state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .unwrap();
    let made = worktrees
        .checkout(&id, Some("teach-the-picker-to-filter"))
        .unwrap();
    journal::store_session_checkout(&session_path, &made).unwrap();
    let made_path = made.path.clone().expect("the checkout was just made");

    // Both halves are the Session's own record now, branch included —
    // nothing has to reconstruct either from a path.
    let stored = journal::read_session_checkout(&session_path).unwrap();
    assert_eq!(stored, Some(made.clone()));
    assert_eq!(
        made_path.file_name().unwrap(),
        format!("teach-the-picker-to-filter-{id}").as_str()
    );
    assert_eq!(
        made.branch,
        format!("styra/teach-the-picker-to-filter-{id}")
    );
    assert_eq!(
        state
            .session_checkout(&session_path, &workspace.id, &id)
            .unwrap(),
        Some(made.clone())
    );

    // And the record, not the name, is what answers: a checkout renamed to
    // something the id-suffix scan cannot find is still reported as this
    // Session's.
    let renamed = made_path.with_file_name("renamed-by-the-operator");
    std::fs::rename(&made_path, &renamed).unwrap();
    assert_eq!(
        crate::worktree::existing_checkout(
            &crate::workspace::worktrees_dir(&store, &workspace.id),
            &id
        ),
        None
    );
    assert_eq!(
        state
            .session_checkout(&session_path, &workspace.id, &id)
            .unwrap(),
        Some(made)
    );
}

/// A new Session reached with `n` shares the source Session's checkout,
/// including its branch and uncommitted file tree, instead of falling
/// back to the Workspace's original host directory.
#[test]
fn a_new_session_can_plan_in_an_existing_sessions_checkout() {
    let StoredSession {
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("inherits-checkout");
    let worktrees = state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .unwrap();
    let checkout = worktrees
        .checkout(&id, Some("shared-investigation"))
        .unwrap();
    journal::store_session_checkout(&session_path, &checkout).unwrap();

    let plan = state
        .plan_session(crate::protocol::PlanSession {
            workspace_id: workspace.id,
            selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
            launch: LaunchPolicy::default(),
            create_worktree: false,
            checkout_from: Some(id),
        })
        .unwrap();

    let sandbox = SandboxLayout::default().workspace;
    assert_eq!(plan.working_directory, sandbox);
    assert!(plan.mounts.iter().any(|attributed| matches!(
        &attributed.mount,
        Mount::Bind { source, destination, access: MountAccess::ReadWrite }
            if Some(source) == checkout.path.as_ref() && destination == &sandbox
    )));
}

#[test]
fn a_new_worktree_starts_at_the_source_sessions_current_branch() {
    let StoredSession {
        host,
        workspace,
        id,
        session_path,
        state,
        git,
        _directory,
        ..
    } = stored_session("fork-new-checkout");
    let worktrees = state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .unwrap();
    let source = worktrees.checkout(&id, Some("source-work")).unwrap();
    journal::store_session_checkout(&session_path, &source).unwrap();
    let source_commit = git.commit_on(&source.branch);
    let source_path = source.path.as_ref().unwrap();
    std::fs::write(source_path.join("uncommitted.txt"), "only in source").unwrap();

    let fresh = state
        .new_session_worktree(
            &workspace.id,
            &worktrees,
            "new-session",
            Some("new-work"),
            Some(&id),
            None,
        )
        .unwrap();
    assert_ne!(fresh.path, source.path);
    assert_ne!(fresh.branch, source.branch);
    assert_eq!(
        fresh.branched_from,
        Some(crate::worktree::BranchPoint {
            branch: Some(source.branch.clone()),
            commit: source_commit.clone(),
        })
    );
    assert_eq!(
        git.commit(&host, &fresh.branch).unwrap(),
        Some(source_commit.clone())
    );
    assert!(!fresh
        .path
        .as_ref()
        .unwrap()
        .join("uncommitted.txt")
        .exists());
    assert!(source_path.join("uncommitted.txt").exists());

    // The durable branch remains a usable source after checkout cleanup.
    worktrees.remove(source_path).unwrap();
    journal::clear_session_worktree(&session_path).unwrap();
    let restored_source = state
        .new_session_worktree(
            &workspace.id,
            &worktrees,
            "after-cleanup",
            None,
            Some(&id),
            None,
        )
        .unwrap();
    assert_eq!(restored_source.branched_from, fresh.branched_from);
    assert!(!source_path.exists());

    // Without a linked source, use the Workspace's checked-out HEAD.
    let fallback = state
        .new_session_worktree(
            &workspace.id,
            &worktrees,
            "from-workspace",
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        fallback.branched_from.as_ref().unwrap().commit,
        git.commit(&host, "HEAD").unwrap().unwrap()
    );
    assert_ne!(fallback.branched_from, fresh.branched_from);

    // A chosen base branch wins over the source Session's branch, and
    // becomes what the new branch integrates into.
    let based = state
        .new_session_worktree(
            &workspace.id,
            &worktrees,
            "from-base",
            None,
            Some(&id),
            Some(&fresh.branch),
        )
        .unwrap();
    assert_eq!(
        based.branched_from,
        Some(crate::worktree::BranchPoint {
            branch: Some(fresh.branch.clone()),
            commit: source_commit.clone(),
        })
    );
    let error = state
        .new_session_worktree(
            &workspace.id,
            &worktrees,
            "from-nowhere",
            None,
            None,
            Some("no-such-branch"),
        )
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("base branch no-such-branch does not exist"),
        "{error:#}"
    );
}

#[test]
fn a_fresh_worktree_plan_uses_a_new_directory_and_validates_its_source() {
    let StoredSession {
        store,
        host,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("plan-fork-checkout");
    let worktrees = state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .unwrap();
    let source = worktrees.checkout(&id, Some("source-work")).unwrap();
    journal::store_session_checkout(&session_path, &source).unwrap();
    let mut request = crate::protocol::PlanSession {
        workspace_id: workspace.id.clone(),
        selection: Selection::new(crate::agent::Provider::Codex),
        launch: LaunchPolicy::default(),
        create_worktree: true,
        checkout_from: Some(id),
    };
    let plan = state.plan_session(request.clone()).unwrap();
    let pending_path = worktrees.path(PENDING_SESSION_ID);
    assert!(plan.mounts.iter().any(|attributed| matches!(
        &attributed.mount,
        Mount::Bind { source, .. } if source == &pending_path
    )));
    assert!(!pending_path.exists());
    let other_host = host.join("other-workspace");
    std::fs::create_dir_all(&other_host).unwrap();
    let other = crate::workspace::create(&store, &other_host, None).unwrap();
    request.workspace_id = other.id;
    let error = state.plan_session(request).unwrap_err();
    assert!(
        error.to_string().contains("belongs to Workspace"),
        "{error:#}"
    );
}

/// A Session launched before the record existed has a checkout and no
/// mention of it. It is found the old way — by the id its directory ends
/// with — and written down on the way past, so the scan answers for that
/// Session once and never again.
#[test]
fn a_session_predating_the_record_is_given_one() {
    let StoredSession {
        store,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("backfill-checkout");
    let worktrees = state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .unwrap();
    let path = worktrees
        .checkout(&id, Some("fix-the-flaky-test"))
        .unwrap()
        .path
        .unwrap();
    assert_eq!(journal::read_session_checkout(&session_path).unwrap(), None);

    let found = state
        .session_checkout(&session_path, &workspace.id, &id)
        .unwrap();

    assert_eq!(found, Some(crate::worktree::Checkout::at(path.clone())));
    assert_eq!(
        journal::read_session_checkout(&session_path).unwrap(),
        Some(crate::worktree::Checkout::at(path))
    );
    // A different Session in the same Workspace still has none of its own,
    // and is not handed its neighbour's.
    let other = crate::workspace::sessions_dir(&store, &workspace.id).join("1757000000000-9-9");
    assert_eq!(
        state
            .session_checkout(&other, &workspace.id, "1757000000000-9-9")
            .ok()
            .flatten(),
        None
    );
}

/// A source with no live interaction still learns where its history was
/// continued: the marker is appended to its stored journal, so its next
/// replay shows the link.
#[test]
fn a_stored_source_records_the_branch_taken_from_it() {
    let store = temp_path("source-branch-marker-store");
    let host = temp_path("source-branch-marker-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    let (mut journal, source_id) = Journal::create_in_workspace(
        &store,
        &workspace.id,
        &profile,
        &selection,
        Some("review".into()),
    )
    .unwrap();
    journal.record_user_message("original question").unwrap();
    let source_path = journal.path().parent().unwrap().to_path_buf();
    drop(journal);

    state
        .record_source_branch(&source_id, &source_path, "styra-branch")
        .unwrap();

    assert_eq!(
        journal::replay(&source_path, profile.protocol).unwrap(),
        vec![
            crate::event::AgentEvent::UserMessage {
                text: "original question".into()
            },
            crate::event::AgentEvent::Branched {
                direction: crate::event::BranchDirection::To,
                session: "styra-branch".into(),
                name: None,
            },
        ]
    );

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// A Workspace opened on a linked worktree names the same directory twice:
/// as the Workspace and as the checkout root its repository mounts carry.
/// Planning there has to succeed — the repeat asks for nothing the
/// Workspace mount does not already provide.
#[test]
fn a_workspace_on_a_linked_worktree_plans_without_a_mount_conflict() {
    let store = temp_path("worktree-workspace-store");
    let host = temp_path("worktree-workspace-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let worktree = host.join(".worktrees/sandbox-base");
    git.create_worktree(&host, "sandbox-base", &worktree)
        .unwrap();

    let state = ServerState::with_git(git.clone(), store.clone());
    // Exactly what a client launching from inside the worktree records:
    // the nearest enclosing checkout is the worktree itself.
    let workspace = crate::workspace::create_with_repository(
        git.as_ref(),
        &store,
        &worktree,
        None,
        Some(&worktree),
    )
    .unwrap();

    let plan = state
        .plan_session(crate::protocol::PlanSession {
            workspace_id: workspace.id,
            selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
            launch: LaunchPolicy::default(),
            create_worktree: false,
            checkout_from: None,
        })
        .unwrap();
    let canonical = worktree.canonicalize().unwrap();
    let at_worktree: Vec<_> = plan
        .mounts
        .iter()
        .filter(|attributed| attributed.mount.destination() == canonical)
        .collect();
    assert!(matches!(
        at_worktree.as_slice(),
        [AttributedMount {
            origin: MountOrigin::Workspace,
            mount: Mount::Bind {
                access: MountAccess::ReadWrite,
                ..
            },
        }]
    ));

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// With worktrees enabled the interaction does not get the operator's
/// checkout at all: its sandbox workspace is a checkout of its own, and the
/// only other thing Styra adds is the Git metadata that makes that checkout
/// a repository.
#[test]
fn an_enabled_workspace_launches_in_its_own_checkout() {
    let store = temp_path("worktree-launch-store");
    let host = temp_path("worktree-launch-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);

    let state = ServerState::with_git(git, store.clone());
    let workspace = crate::workspace::create(&store, &host, None).unwrap();

    let plan = state
        .plan_session(crate::protocol::PlanSession {
            workspace_id: workspace.id.clone(),
            selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
            launch: LaunchPolicy::default(),
            create_worktree: true,
            checkout_from: None,
        })
        .unwrap();

    let sandbox = SandboxLayout::default().workspace;
    assert_eq!(plan.working_directory, sandbox);
    let at_workspace: Vec<_> = plan
        .mounts
        .iter()
        .filter(|attributed| attributed.mount.destination() == sandbox)
        .collect();
    assert!(matches!(
        at_workspace.as_slice(),
        [AttributedMount {
            origin: MountOrigin::Workspace,
            mount: Mount::Bind {
                source,
                access: MountAccess::ReadWrite,
                ..
            },
        }] if *source == crate::workspace::worktrees_dir(&store, &workspace.id)
            .join(PENDING_SESSION_ID)
    ));
    let canonical = host.canonicalize().unwrap();
    assert!(plan.mounts.iter().any(|attributed| matches!(
        &attributed.mount,
        Mount::Bind { source, destination, access: MountAccess::ReadWrite }
            if *source == canonical.join(".git") && destination == source
    )));
    // The directory the operator works in is left alone, which is the
    // whole point of checking out somewhere else.
    assert!(!plan.mounts.iter().any(|attributed| matches!(
        &attributed.mount,
        Mount::Bind { source, access: MountAccess::ReadWrite, .. } if *source == canonical
    )));

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// The workspace mount is the one grant no policy row can name, so the
/// policy answer for it has to reach the plan — and the plan is what the
/// operator is shown before their first message.
#[test]
fn a_read_only_workspace_policy_plans_the_workspace_mount_read_only() {
    let store = temp_path("read-only-workspace-store");
    let host = temp_path("read-only-workspace-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();

    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let canonical = host.canonicalize().unwrap();
    let access_at_workspace = |launch: LaunchPolicy| {
        let plan = state
            .plan_session(crate::protocol::PlanSession {
                workspace_id: workspace.id.clone(),
                selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
                launch,
                create_worktree: false,
                checkout_from: None,
            })
            .unwrap();
        plan.mounts
            .iter()
            .find_map(|attributed| match &attributed.mount {
                Mount::Bind {
                    destination,
                    access,
                    ..
                } if *destination == canonical => Some(*access),
                _ => None,
            })
            .expect("the workspace is always mounted")
    };

    // Writable is what a launch does when no layer says otherwise.
    assert_eq!(
        access_at_workspace(LaunchPolicy::default()),
        MountAccess::ReadWrite
    );
    assert_eq!(
        access_at_workspace(LaunchPolicy {
            writable_workspace: Some(false),
            ..LaunchPolicy::default()
        }),
        MountAccess::ReadOnly
    );

    // And the Workspace's standing answer applies to a launch that asks
    // for nothing, because the two are merged before the spec is built.
    crate::workspace::change_launch(
        &store,
        &workspace.id,
        crate::protocol::WorkspaceLaunchChange::SetWritableWorkspace(Some(false)),
    )
    .unwrap();
    assert_eq!(
        access_at_workspace(LaunchPolicy::default()),
        MountAccess::ReadOnly
    );

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// A launch in the repository's main worktree works in the operator's own
/// checkout, so its workspace mount is read-only until a layer grants it;
/// a worktree of the launch's own is writable as before. The plan says
/// which default applied, for the view to name what an unstated answer means.
#[test]
fn the_main_worktree_is_planned_read_only_unless_a_layer_grants_it() {
    let store = temp_path("main-worktree-read-only-store");
    let host = temp_path("main-worktree-read-only-host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&host).ok();
    std::fs::create_dir_all(&host).unwrap();
    let git = Arc::new(crate::git::FakeGit::new());
    git.init(&host);
    let state = ServerState::with_git(git, store.clone());
    let workspace = crate::workspace::create(&store, &host, None).unwrap();
    let plan = |launch: LaunchPolicy, create_worktree: bool| {
        state
            .plan_session(crate::protocol::PlanSession {
                workspace_id: workspace.id.clone(),
                selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
                launch,
                create_worktree,
                checkout_from: None,
            })
            .unwrap()
    };
    let access_of = |plan: &DrivaOptions, checkout: &Path| {
        plan.mounts
            .iter()
            .find_map(|attributed| match &attributed.mount {
                Mount::Bind { source, access, .. } if source == checkout => Some(*access),
                _ => None,
            })
            .expect("the checkout is always mounted")
    };
    let canonical = host.canonicalize().unwrap();

    let in_main = plan(LaunchPolicy::default(), false);
    assert!(in_main.workspace_read_only_by_default);
    assert_eq!(access_of(&in_main, &canonical), MountAccess::ReadOnly);

    let granted = plan(
        LaunchPolicy {
            writable_workspace: Some(true),
            ..LaunchPolicy::default()
        },
        false,
    );
    assert_eq!(access_of(&granted, &canonical), MountAccess::ReadWrite);

    let worktrees = state
        .workspace_worktrees(&workspace, true)
        .unwrap()
        .unwrap();
    let in_worktree = plan(LaunchPolicy::default(), true);
    assert!(!in_worktree.workspace_read_only_by_default);
    assert_eq!(
        access_of(&in_worktree, &worktrees.path(PENDING_SESSION_ID)),
        MountAccess::ReadWrite
    );

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// Which interactions a reset releases: the providers are separate
/// subscriptions and each reports several windows, so waking the wrong
/// sessions would send held-back work into a limit that is still refusing
/// it.
#[test]
fn a_reset_releases_only_the_work_its_own_window_refused() {
    let refusal = |provider, window: &str| crate::protocol::QuotaEvent {
        at_ms: 1_000,
        session_id: "s-1".into(),
        provider,
        window: window.into(),
        status: crate::protocol::QuotaStatus::Exhausted,
        utilization: None,
        resets_at_ms: Some(2_000),
        detail: None,
    };
    let reset = crate::quota::WindowReset {
        provider: crate::agent::Provider::Claude,
        window: "five_hour".into(),
        at_ms: 2_000,
    };

    assert!(reset_releases(
        &refusal(crate::agent::Provider::Claude, "five_hour"),
        &reset
    ));
    // The same account's other window is still full.
    assert!(!reset_releases(
        &refusal(crate::agent::Provider::Claude, "seven_day"),
        &reset
    ));
    // And the other subscription's window of the same name is not this
    // one at all.
    assert!(!reset_releases(
        &refusal(crate::agent::Provider::Codex, "five_hour"),
        &reset
    ));
}

#[test]
fn socket_health_reports_the_service() {
    let socket = temp_path("health");
    std::fs::remove_file(&socket).ok();
    let listener = UnixListener::bind(&socket).unwrap();
    let store = socket.with_extension("store");
    let state = ServerState::new(store.clone(), socket.clone());
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        serve_connection(stream, &state).unwrap();
    });

    let health = Client::new(&socket).health().unwrap();
    assert_eq!(health.service, "styra");

    server.join().unwrap();
    std::fs::remove_file(socket).ok();
    std::fs::remove_dir_all(store).ok();
}

/// The standalone transport answers the same requests as the socket one,
/// with no listener, no daemon and no socket file anywhere.
#[test]
fn an_in_process_client_serves_requests_without_a_socket() {
    let root = std::env::temp_dir().join(format!("styra-in-process-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let store = root.join("store");
    let host = root.join("work");
    std::fs::create_dir_all(&host).unwrap();

    let lock_path = root.join("test.lock");
    let lock = std::fs::File::create(lock_path).unwrap();
    let client = crate::daemon::in_process_client(ServerState::in_process(store.clone(), lock));
    assert!(client.socket_path().is_none());
    assert_eq!(client.health().unwrap().service, "styra");

    let workspace = client
        .create_workspace(&CreateWorkspace {
            host_path: host.clone(),
            name: Some("standalone".into()),
            git_repository: None,
        })
        .unwrap();
    assert_eq!(workspace.host_path, host);
    assert_eq!(
        client
            .list_workspaces()
            .unwrap()
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect::<Vec<_>>(),
        vec![workspace.id]
    );
    // An error comes back as an error, not as a wire-encoded string.
    assert!(client.workspace("styra-nothing").is_err());

    let clone = client.clone();
    client.shutdown().unwrap();
    assert!(clone
        .health()
        .unwrap_err()
        .to_string()
        .contains("shut down"));
    assert!(client
        .shutdown()
        .unwrap_err()
        .to_string()
        .contains("already shut down"));

    std::fs::remove_dir_all(root).ok();
}

/// An operator's mount request is resolved against the host now, so a path
/// that is not there is reported while the policy is being chosen.
#[test]
fn extra_mounts_resolve_their_source_and_default_the_destination_to_it() {
    let root =
        std::env::temp_dir().join(format!("styra-server-extra-mounts-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(root.join("data")).unwrap();

    let resolved = resolve_launch_mounts(&[
        LaunchMount {
            source: root.join("data"),
            destination: None,
            writable: true,
        },
        LaunchMount {
            source: root.join("data"),
            destination: Some(PathBuf::from("/mnt/data")),
            writable: false,
        },
    ])
    .unwrap();

    assert_eq!(
        resolved[0].source,
        root.join("data").canonicalize().unwrap()
    );
    assert_eq!(resolved[0].destination, resolved[0].source);
    assert!(resolved[0].writable);
    assert_eq!(resolved[1].destination, PathBuf::from("/mnt/data"));
    assert!(!resolved[1].writable);

    let missing = resolve_launch_mounts(&[LaunchMount {
        source: root.join("absent"),
        destination: None,
        writable: false,
    }])
    .unwrap_err();
    assert!(
        missing.to_string().contains("invalid extra mount source"),
        "{missing}"
    );

    let relative = resolve_launch_mounts(&[LaunchMount {
        source: root.join("data"),
        destination: Some(PathBuf::from("mnt/data")),
        writable: false,
    }])
    .unwrap_err();
    assert!(relative.to_string().contains("absolute"), "{relative}");

    std::fs::remove_dir_all(root).ok();
}

/// Two mounts landing on the same place inside the sandbox is a policy
/// Driva refuses, so it is caught while planning rather than at spawn.
#[test]
fn a_policy_binding_two_things_at_one_destination_is_rejected() {
    let options = DrivaOptions {
        isolation_backend: "bwrap".into(),
        command: vec!["codex".into()],
        working_directory: PathBuf::from("/tmp/styra/workspace"),
        network: false,
        base: Vec::new(),
        mounts: vec![
            AttributedMount {
                origin: MountOrigin::Workspace,
                mount: driva::Mount::Bind {
                    source: PathBuf::from("/srv/one"),
                    destination: PathBuf::from("/tmp/styra/workspace"),
                    access: driva::MountAccess::ReadWrite,
                },
            },
            AttributedMount {
                origin: MountOrigin::Operator,
                mount: driva::Mount::Bind {
                    source: PathBuf::from("/srv/two"),
                    destination: PathBuf::from("/tmp/styra/workspace"),
                    access: driva::MountAccess::ReadOnly,
                },
            },
        ],
        ..Default::default()
    };
    let error = ensure_distinct_destinations(&options).unwrap_err();
    assert!(
        error.to_string().contains("conflicting mount destination"),
        "{error}"
    );

    let mut fine = options;
    fine.mounts.pop();
    assert!(ensure_distinct_destinations(&fine).is_ok());
}

#[test]
fn stored_ids_are_resolved_from_the_store_listing() {
    let store = std::env::temp_dir().join(format!("styra-server-id-test-{}", std::process::id()));
    std::fs::remove_dir_all(&store).ok();
    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    let error = state.stored_summary("../../etc").unwrap_err();
    assert!(error.to_string().contains("was not found"));
    std::fs::remove_dir_all(store).ok();
}

#[test]
fn broker_is_staged_in_the_sandbox_control_mount() {
    let root =
        std::env::temp_dir().join(format!("styra-server-broker-test-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let socket = root.join("styra.sock");
    let state = ServerState::new(root.join("store"), socket);

    let broker = state
        .prepare_broker("session-1", PathBuf::from("/usr/bin/tmux"))
        .unwrap();

    assert_eq!(
        broker.executable,
        PathBuf::from("/tmp/styra/control/styra-broker")
    );
    let staged = broker.control.source.join("styra-broker");
    let metadata = std::fs::metadata(&staged).unwrap();
    assert!(metadata.len() > 0);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn describing_a_broker_names_the_same_mount_without_staging_anything() {
    let root = std::env::temp_dir().join(format!("styra-server-plan-test-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let state = ServerState::new(root.join("store"), root.join("styra.sock"));

    let planned = state.describe_broker(PENDING_SESSION_ID, PathBuf::from("/usr/bin/tmux"));
    let real = state.describe_broker("session-1", PathBuf::from("/usr/bin/tmux"));

    assert_eq!(planned.control.destination, real.control.destination);
    assert_eq!(planned.executable, real.executable);
    assert_eq!(
        planned.control.source,
        root.join("sandboxes").join(PENDING_SESSION_ID)
    );
    assert!(!planned.control.source.exists());
    assert!(!root.join("sandboxes").exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_legacy_session_without_a_provider_id_is_viewable_but_not_resumable() {
    let store =
        std::env::temp_dir().join(format!("styra-server-legacy-test-{}", std::process::id()));
    let host = store.with_extension("host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::create_dir_all(&host).unwrap();
    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    let workspace = crate::workspace::create(&store, &host, Some("legacy".into())).unwrap();
    let session_dir =
        crate::workspace::sessions_dir(&store, &workspace.id).join("0000000000001-1-1");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
            session_dir.join("journal.jsonl"),
            concat!(
                "{\"source\":\"user\",\"at_ms\":1,\"text\":\"old question\"}\n",
                "{\"source\":\"agent\",\"at_ms\":2,\"raw\":\"{\\\"method\\\":\\\"item/completed\\\",\\\"params\\\":{\\\"item\\\":{\\\"type\\\":\\\"agentMessage\\\",\\\"id\\\":\\\"m\\\",\\\"text\\\":\\\"old answer\\\"}}}\"}\n"
            ),
        )
        .unwrap();
    std::fs::write(
        session_dir.join("session.json"),
        serde_json::json!({
            "workspace_id": workspace.id,
            "selection": {
                "provider": "codex",
                "model": "gpt-5.6-sol",
                "effort": "high"
            },
            "protocol": "codex-app-server"
        })
        .to_string(),
    )
    .unwrap();

    assert!(matches!(
        state
            .handle(Request::StoredSession {
                id: "0000000000001-1-1".into(),
                raw: true,
            })
            .unwrap(),
        Response::StoredSession(_)
    ));
    // The same session asked for without raw lines: the decoded events are
    // still there, the verbatim wire log is not shipped at all.
    let Response::StoredSession(events_only) = state
        .handle(Request::StoredSession {
            id: "0000000000001-1-1".into(),
            raw: false,
        })
        .unwrap()
    else {
        panic!("expected a stored session");
    };
    assert_eq!(events_only.events.len(), 2);
    assert!(events_only.raw.is_empty());
    let replayed = replayed_session_updates(
        &session_dir,
        crate::event::Protocol::CodexAppServer,
        WorkspaceMount {
            host: &host,
            sandbox: &host,
        },
    )
    .unwrap();
    assert_eq!(
        replayed
            .iter()
            .map(|update| update.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    // In live order: the operator's message and then the line carrying it,
    // the agent's line and then what was decoded from it.
    assert!(matches!(
        &replayed[0].update,
        InteractionUpdate::Event(
            crate::event::AgentEvent::UserMessage { text }
        ) if text == "old question"
    ));
    assert!(matches!(
        &replayed[1].update,
        InteractionUpdate::Raw(line) if line.at_ms == 1
    ));
    assert!(matches!(
        &replayed[2].update,
        InteractionUpdate::Raw(line) if line.at_ms == 2
    ));
    assert!(matches!(
        &replayed[3].update,
        InteractionUpdate::Event(
            crate::event::AgentEvent::AgentMessage { text }
        ) if text == "old answer"
    ));
    let error = state
        .resume_session(ResumeSession {
            id: "0000000000001-1-1".into(),
            launch: LaunchPolicy::default(),
            selection: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("can be viewed but not resumed"));

    // Switching providers also needs a native transcript. A legacy journal
    // without one fails before changing the stored selection.
    let error = state
        .resume_session(ResumeSession {
            id: "0000000000001-1-1".into(),
            launch: LaunchPolicy::default(),
            selection: Some(Selection::parse("claude:claude-opus-5/high").unwrap()),
        })
        .unwrap_err();
    assert!(
        error.to_string().contains("no stored provider session id"),
        "{error:#}"
    );

    // The same resume naming this agent's own catalog gets as far as the
    // native transcript this legacy journal does not have.
    let error = state
        .resume_session(ResumeSession {
            id: "0000000000001-1-1".into(),
            launch: LaunchPolicy::default(),
            selection: Some(Selection::parse("codex:gpt-5.6-luna/low").unwrap()),
        })
        .unwrap_err();
    assert!(
        error.to_string().contains("can be viewed but not resumed"),
        "{error:#}"
    );

    // Conversion also fails without changing the legacy Session when no
    // native transcript is available.
    let error = state
        .convert_session_provider("0000000000001-1-1")
        .unwrap_err();
    assert!(
        error.to_string().contains("no stored provider session id"),
        "{error:#}"
    );
    assert!(
        format!("{error:#}").contains("no stored provider session id"),
        "{error:#}"
    );
    assert_eq!(
        crate::workspace::get(&store, &workspace.id)
            .unwrap()
            .session_count,
        1
    );

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// A client attaching to a restored or resumed interaction applies its
/// seeded stream in order and links each event to the newest wire line
/// received before it — or, for an operator message, to the outgoing line
/// that follows it (see the TUI's `apply_update`). That link carries the
/// `at_ms` a branch is taken at, so every replayed entry has to land on its
/// own line; an entry left without one cannot be branched from.
#[test]
fn a_replayed_stream_links_every_entry_to_its_own_wire_line() {
    let directory = std::env::temp_dir().join(format!(
        "styra-server-replay-links-test-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();
    let answer = |text: &str| {
        serde_json::json!({
            "method": "item/completed",
            "params": {"item": {"type": "agentMessage", "id": text, "text": text}}
        })
        .to_string()
    };
    let journal = [
        serde_json::json!({"source": "user", "at_ms": 10, "text": "first question"}),
        serde_json::json!({"source": "agent", "at_ms": 20, "raw": answer("first answer")}),
        serde_json::json!({"source": "user", "at_ms": 30, "text": "second question"}),
        serde_json::json!({"source": "agent", "at_ms": 40, "raw": answer("second answer")}),
    ]
    .iter()
    .map(|record| format!("{record}\n"))
    .collect::<String>();
    std::fs::write(directory.join("journal.jsonl"), journal).unwrap();

    let replayed = replayed_session_updates(
        &directory,
        crate::event::Protocol::CodexAppServer,
        WorkspaceMount {
            host: &directory,
            sandbox: &directory,
        },
    )
    .unwrap();

    // The client's linking rule, applied to the stream as it arrives.
    let mut raw: Vec<&crate::protocol::RawLine> = Vec::new();
    let mut entries: Vec<(&crate::event::AgentEvent, Option<usize>)> = Vec::new();
    for update in &replayed {
        match &update.update {
            InteractionUpdate::Event(event) => {
                entries.push((event, raw.len().checked_sub(1)));
            }
            InteractionUpdate::Raw(line) => {
                raw.push(line);
                if line.direction == crate::protocol::Direction::ToAgent {
                    if let Some((crate::event::AgentEvent::UserMessage { .. }, link)) =
                        entries.last_mut()
                    {
                        *link = Some(raw.len() - 1);
                    }
                }
            }
            _ => {}
        }
    }
    let branch_points = entries
        .iter()
        .map(|(_, link)| link.map(|index| raw[index].at_ms))
        .collect::<Vec<_>>();

    assert_eq!(
        branch_points,
        vec![Some(10), Some(20), Some(30), Some(40)],
        "each replayed entry should be linked to its own wire line"
    );

    std::fs::remove_dir_all(directory).ok();
}

#[test]
fn native_session_lookup_detects_removal() {
    let root = std::env::temp_dir().join(format!("styra-native-test-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/rollout-provider-7.jsonl"), "").unwrap();
    assert!(find_session_file(&root, "provider-7").is_some());
    assert!(find_session_file(&root, "provider-gone").is_none());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn conversion_always_flips_between_styras_two_interactive_providers() {
    assert_eq!(
        other_interactive_provider(crate::agent::Provider::Codex).unwrap(),
        crate::agent::Provider::Claude
    );
    assert_eq!(
        other_interactive_provider(crate::agent::Provider::Claude).unwrap(),
        crate::agent::Provider::Codex
    );
}

#[test]
fn native_session_format_matches_each_providers_own_transcript() {
    assert_eq!(
        native_session_format(crate::agent::Provider::Codex),
        genta::session::SessionFormat::Codex
    );
    assert_eq!(
        native_session_format(crate::agent::Provider::Claude),
        genta::session::SessionFormat::Claude
    );
}

#[test]
fn claude_project_directory_names_replace_every_non_alphanumeric_character() {
    assert_eq!(
        claude_project_directory_name(Path::new("/home/op/.dotfiles/project")),
        "-home-op--dotfiles-project"
    );
}

#[test]
fn a_converted_destination_is_scoped_by_provider() {
    let claude = native_session_destination(
        crate::agent::Provider::Claude,
        "new-id",
        Path::new("/home/op/project"),
    )
    .unwrap();
    assert!(claude.ends_with("-home-op-project/new-id.jsonl"));

    let codex =
        native_session_destination(crate::agent::Provider::Codex, "new-id", Path::new("/x"))
            .unwrap();
    assert!(codex.ends_with("genta-imported/rollout-new-id.jsonl"));
}

/// What a resume is *changing* is measured against the end of the
/// Session's history, not against the selection it was created with — and
/// the history of a converted Session includes the old agent's own model
/// reports, which say nothing about this one.
#[test]
fn the_replayed_selection_is_the_end_of_the_history_and_ignores_a_foreign_agents_reports() {
    let stored = Selection::parse("claude:claude-opus-5/high").unwrap();
    let events = |events: Vec<crate::event::AgentEvent>| {
        events
            .into_iter()
            .enumerate()
            .map(|(index, event)| SequencedUpdate {
                sequence: index as u64 + 1,
                update: InteractionUpdate::Event(event),
            })
            .collect::<Vec<_>>()
    };

    // Nothing reported: the Session is still on what it was created with.
    assert_eq!(replayed_selection(&[], &stored), stored);

    // Its own agent's reports carry, last one winning.
    let own = events(vec![
        crate::event::AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: Some("claude-sonnet-5".into()),
            effort: Some("max".into()),
        },
        crate::event::AgentEvent::ModelChanged {
            model: Some("claude-opus-4-8".into()),
            effort: None,
        },
    ]);
    assert_eq!(
        replayed_selection(&own, &stored),
        Selection::parse("claude:claude-opus-4-8/max").unwrap()
    );

    // The copied history of a conversion does not: its model is a model
    // this agent cannot be running, and its effort describes that same
    // foreign thread.
    let converted = events(vec![crate::event::AgentEvent::ThreadStarted {
        thread_id: "t".into(),
        model: Some("gpt-5.6-terra".into()),
        effort: Some("medium".into()),
    }]);
    assert_eq!(replayed_selection(&converted, &stored), stored);

    // Switching back must override model reports from an earlier run
    // under this same provider, even before the new process starts.
    let mut switched_back = own;
    push_sequenced(
        &mut switched_back,
        InteractionUpdate::Event(crate::event::AgentEvent::ProviderChanged {
            from: "codex".into(),
            to: "claude".into(),
            model: stored.model.clone(),
            effort: stored.effort.as_str().to_owned(),
        }),
    );
    assert_eq!(replayed_selection(&switched_back, &stored), stored);
}

#[test]
fn rfc3339_timestamps_parse_to_milliseconds_since_the_epoch() {
    assert_eq!(
        parse_rfc3339_ms("2026-08-21T10:00:00.000Z"),
        Some(1787306400000)
    );
    assert_eq!(
        parse_rfc3339_ms("2026-08-21T10:00:00.500Z"),
        Some(1787306400500)
    );
    assert_eq!(parse_rfc3339_ms("not a timestamp"), None);
}

fn claude_transcript(answer: &str, last_user: &str, last_timestamp: &str) -> String {
    [
            serde_json::json!({"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"first"}}),
            serde_json::json!({"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":answer}]}}),
            serde_json::json!({"type":"user","uuid":"u2","parentUuid":"a1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":last_timestamp,"message":{"role":"user","content":last_user}}),
        ]
        .into_iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Cutoffs keep the leading history. A named operator message also
/// survives when the host sent it before the provider stamped its copy.
#[test]
fn message_cutoffs_keep_leading_history_and_the_named_operator_message() {
    let on_time = claude_transcript("second", "third", "2026-08-21T10:00:02.000Z");
    let delayed = claude_transcript("answer", "try again", "2026-08-21T10:00:02.500Z");
    let reply_cutoff = parse_rfc3339_ms("2026-08-21T10:00:01.000Z").unwrap();
    // The host sent "try again" half a second before Claude stamped it.
    let operator_cutoff = parse_rfc3339_ms("2026-08-21T10:00:02.000Z").unwrap();
    for (case, transcript, cutoff, selected, expected) in [
        ("through the reply", &on_time, reply_cutoff, None, 2),
        ("before the first message", &on_time, 0, None, 0),
        ("through the end", &on_time, u64::MAX, None, 3),
        ("delayed unnamed turn", &delayed, operator_cutoff, None, 2),
        (
            "delayed named turn",
            &delayed,
            operator_cutoff,
            Some("try again"),
            3,
        ),
        (
            "a different name does not extend the cutoff",
            &delayed,
            operator_cutoff,
            Some("something else"),
            2,
        ),
    ] {
        let count = messages_up_to(
            transcript,
            genta::session::SessionFormat::Claude,
            cutoff,
            selected,
        )
        .unwrap_or_else(|error| panic!("{case}: {error:#}"));
        assert_eq!(count, expected, "{case}");
    }
}

/// The same skew decides which single message `selected_only` takes: named
/// or not, it must be the operator's own turn rather than the reply before
/// it.
#[test]
fn a_selected_only_branch_on_an_operator_message_takes_that_message() {
    let claude = claude_transcript("answer", "try again", "2026-08-21T10:00:02.500Z");
    let cutoff = parse_rfc3339_ms("2026-08-21T10:00:02.000Z").unwrap();
    let branched = branch_native_session(
        &claude,
        genta::session::SessionFormat::Claude,
        genta::session::SessionFormat::Claude,
        "new-id",
        Path::new("/repo"),
        BranchPoint {
            at_ms: Some(cutoff),
            history: crate::protocol::BranchHistory::SelectedOnly,
            selected_operator_message: Some("try again"),
        },
    )
    .unwrap();
    let parsed = genta::session::parse(&branched, genta::session::SessionFormat::Claude).unwrap();

    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].text, "try again");
}

#[test]
fn a_selected_only_native_branch_contains_just_the_selected_message() {
    let claude = claude_transcript("selected", "later", "2026-08-21T10:00:02.000Z");
    let cutoff = parse_rfc3339_ms("2026-08-21T10:00:01.000Z").unwrap();
    let branched = branch_native_session(
        &claude,
        genta::session::SessionFormat::Claude,
        genta::session::SessionFormat::Codex,
        "new-id",
        Path::new("/new/repo"),
        BranchPoint {
            at_ms: Some(cutoff),
            history: crate::protocol::BranchHistory::SelectedOnly,
            selected_operator_message: None,
        },
    )
    .unwrap();
    let parsed = genta::session::parse(&branched, genta::session::SessionFormat::Codex).unwrap();

    assert_eq!(parsed.id, "new-id");
    assert_eq!(parsed.cwd, "/new/repo");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].text, "selected");
}

/// The standing policy is stored with the Workspace, not with the client
/// that set it, so every client launching there reads the same one back.
#[test]
fn a_workspace_launch_policy_is_stored_and_reported_back() {
    let store = std::env::temp_dir().join(format!(
        "styra-server-workspace-launch-{}",
        std::process::id()
    ));
    let host = store.with_extension("host");
    std::fs::remove_dir_all(&store).ok();
    std::fs::create_dir_all(&host).unwrap();
    let state = ServerState::new(store.clone(), store.with_extension("sock"));

    let created = match state
        .handle(Request::CreateWorkspace(CreateWorkspace {
            host_path: host.clone(),
            name: Some("api test".into()),
            git_repository: None,
        }))
        .unwrap()
    {
        Response::WorkspaceCreated(workspace) => workspace,
        other => panic!("unexpected response: {other:?}"),
    };
    assert!(created.launch.is_empty());
    assert_eq!(created.name.as_deref(), Some("api test"));
    assert!(matches!(
        state.handle(Request::ListWorkspaces).unwrap(),
        Response::Workspaces(workspaces) if workspaces == vec![created.clone()]
    ));
    assert!(matches!(
        state
            .handle(Request::ListSessions {
                workspace_id: created.id.clone(),
            })
            .unwrap(),
        Response::StoredSessions(sessions) if sessions.is_empty()
    ));

    let launch = LaunchPolicy {
        network: Some(true),
        writable_workspace: None,
        templates: vec!["rust".into()],
        mounts: vec![LaunchMount {
            source: PathBuf::from("/srv/corpus"),
            destination: None,
            writable: false,
        }],
        ignore_workspace: false,
        ..LaunchPolicy::default()
    };
    let updated = match state
        .handle(Request::ChangeWorkspaceLaunch {
            workspace_id: created.id.clone(),
            change: crate::protocol::WorkspaceLaunchChange::Replace(launch.clone()),
        })
        .unwrap()
    {
        Response::WorkspaceLaunchUpdated(workspace) => workspace,
        other => panic!("unexpected response: {other:?}"),
    };
    assert_eq!(updated, launch);

    match state
        .handle(Request::Workspace {
            id: created.id.clone(),
        })
        .unwrap()
    {
        Response::Workspace(workspace) => assert_eq!(workspace.launch, launch),
        other => panic!("unexpected response: {other:?}"),
    }

    // A policy cannot be stored for a Workspace that is not there.
    assert!(state
        .handle(Request::ChangeWorkspaceLaunch {
            workspace_id: "no-such-workspace".into(),
            change: crate::protocol::WorkspaceLaunchChange::Replace(launch),
        })
        .is_err());

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

/// The policy is kilobytes a row and every list repeats every row, so the
/// list leaves it out and the one row a client shows is asked for it.
#[test]
fn a_listed_rows_driva_policy_is_asked_for_rather_than_listed() {
    let StoredSession {
        store,
        host,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("listed-driva");
    drop(state);
    let driva = DrivaOptions {
        isolation_backend: "bwrap".into(),
        command: vec!["codex".into()],
        working_directory: host.clone(),
        ..Default::default()
    };
    crate::roster::Roster::open(&store).publish(vec![(
        session_path.clone(),
        listed_interaction(&id, &workspace.id, &host, InteractionActivity::Stopped),
        driva.clone(),
    )]);
    let state = ServerState::new(store.clone(), store.with_extension("sock"));

    let listed =
        serde_json::to_value(state.handle(Request::ListInteractions(None)).unwrap()).unwrap();
    let rows = listed["data"].as_array().expect("a list of rows");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("driva").is_none());

    assert_eq!(
        state
            .handle(Request::InteractionDriva { id: id.clone() })
            .unwrap(),
        Response::InteractionDriva(driva)
    );
    assert!(state
        .handle(Request::InteractionDriva {
            id: "no-such-session".into()
        })
        .is_err());
}

#[test]
fn deleted_workspace_api_blocks_sessions_and_restores_after_reopening() {
    let StoredSession {
        store,
        host,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("deleted-workspace-api");
    drop(state);
    crate::roster::Roster::open(&store).publish(vec![(
        session_path.clone(),
        listed_interaction(&id, &workspace.id, &host, InteractionActivity::Stopped),
        DrivaOptions::default(),
    )]);
    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    assert!(
        matches!(state.handle(Request::ListInteractions(None)).unwrap(),
            Response::Interactions(items) if items.len() == 1)
    );
    state
        .handle(Request::DeleteWorkspace {
            id: workspace.id.clone(),
        })
        .unwrap();
    assert!(matches!(state.handle(Request::ListWorkspaces).unwrap(),
            Response::Workspaces(items) if items.is_empty()));
    assert!(
        matches!(state.handle(Request::ListDeletedWorkspaces).unwrap(),
            Response::Workspaces(items) if items.len() == 1 && items[0].id == workspace.id)
    );
    for request in [
        Request::Workspace {
            id: workspace.id.clone(),
        },
        Request::ListSessions {
            workspace_id: workspace.id.clone(),
        },
        Request::StoredSession {
            id: id.clone(),
            raw: false,
        },
        Request::LoadInteraction { id: id.clone() },
        Request::RenameSession(crate::protocol::RenameSession {
            id: id.clone(),
            name: Some("blocked".into()),
        }),
        Request::PlanSession(crate::protocol::PlanSession {
            workspace_id: workspace.id.clone(),
            selection: Selection::new(crate::agent::Provider::Codex),
            launch: LaunchPolicy::default(),
            create_worktree: false,
            checkout_from: None,
        }),
    ] {
        assert!(
            state.handle(request.clone()).is_err(),
            "accepted {request:?}"
        );
    }
    assert!(session_path.is_dir());
    assert!(
        matches!(state.handle(Request::ListInteractions(None)).unwrap(),
            Response::Interactions(items) if items.is_empty())
    );
    assert!(state
        .handle(Request::Updates {
            id: id.clone(),
            after: 0,
            raw: false
        })
        .is_err());
    drop(state);
    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    assert!(matches!(state.handle(Request::ListWorkspaces).unwrap(),
            Response::Workspaces(items) if items.is_empty()));
    state
        .handle(Request::RestoreWorkspace {
            id: workspace.id.clone(),
        })
        .unwrap();
    assert!(
        matches!(state.handle(Request::ListInteractions(None)).unwrap(),
            Response::Interactions(items) if items.len() == 1 && items[0].activity == InteractionActivity::Stopped)
    );
    assert!(
        matches!(state.handle(Request::ListSessions { workspace_id: workspace.id }).unwrap(),
            Response::StoredSessions(items) if items.len() == 1 && items[0].id == id)
    );
    assert!(state
        .handle(Request::StoredSession { id, raw: false })
        .is_ok());
    drop(state);
}

/// A completed Session is reported active again while a branch further
/// down is active, and completed again once that branch is finished, with
/// nothing ever written to the Session itself.
#[test]
fn a_completed_session_reads_active_while_a_branch_of_it_is() {
    let StoredSession {
        store,
        host,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("branch-reopens-parent");
    drop(state);
    journal::store_session_completed(&session_path, CompletionState::Completed).unwrap();
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    let branch = |parent: &str| {
        let (journal, branch) =
            Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None)
                .unwrap();
        let path = journal.path().parent().unwrap().to_path_buf();
        journal::store_session_origin(
            &path,
            SessionOrigin {
                session_id: parent.into(),
                provider: selection.provider,
                at_ms: None,
                history: Default::default(),
            },
        )
        .unwrap();
        journal::store_session_completed(&path, CompletionState::Completed).unwrap();
        (branch, path)
    };
    let (child, _) = branch(&id);
    let (_, grandchild_path) = branch(&child);
    let mut row = listed_interaction(&id, &workspace.id, &host, InteractionActivity::Stopped);
    row.completed = CompletionState::Completed;
    crate::roster::Roster::open(&store).publish(vec![(
        session_path.clone(),
        row,
        DrivaOptions::default(),
    )]);
    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    let reported = |id: &str| {
        let Response::StoredSessions(sessions) = state
            .handle(Request::ListSessions {
                workspace_id: workspace.id.clone(),
            })
            .unwrap()
        else {
            panic!("expected stored sessions");
        };
        let listed = sessions.iter().find(|session| session.id == id).unwrap();
        let Response::StoredSession(stored) = state
            .handle(Request::StoredSession {
                id: id.into(),
                raw: false,
            })
            .unwrap()
        else {
            panic!("expected a stored session");
        };
        assert_eq!(listed.completed, stored.summary.completed);
        listed.completed
    };
    let unfinished_rows = || {
        let Response::Interactions(rows) = state
            .handle(Request::ListInteractions(Some(InteractionQuery {
                skip_done: true,
                ..Default::default()
            })))
            .unwrap()
        else {
            panic!("expected interactions");
        };
        rows.into_iter()
            .map(|row| (row.id, row.completed))
            .collect::<Vec<_>>()
    };

    assert_eq!(reported(&id), CompletionState::Completed);
    assert!(unfinished_rows().is_empty());

    // Resuming the branch further down clears its completion.
    journal::store_session_completed(&grandchild_path, CompletionState::Active).unwrap();
    assert_eq!(reported(&id), CompletionState::Active);
    assert_eq!(reported(&child), CompletionState::Active);
    assert_eq!(
        unfinished_rows(),
        vec![(id.clone(), CompletionState::Active)]
    );
    let Response::InteractionLoaded(loaded) = state
        .handle(Request::LoadInteraction { id: id.clone() })
        .unwrap()
    else {
        panic!("expected a loaded interaction");
    };
    assert_eq!(loaded.summary.completed, CompletionState::Active);
    assert_eq!(
        journal::read_session_completed(&session_path).unwrap(),
        CompletionState::Completed,
        "the parent's own record is left as the operator set it"
    );

    journal::store_session_completed(&grandchild_path, CompletionState::Abandoned).unwrap();
    assert_eq!(reported(&id), CompletionState::Completed);
    assert!(unfinished_rows().is_empty());
}

/// A client drawing the subtree beneath a branch marker is sent every
/// branch in it, finished or not, while other finished rows stay left
/// out.
#[test]
fn the_branches_of_a_kept_session_are_listed_finished_or_not() {
    let StoredSession {
        store,
        host,
        state,
        workspace,
        id,
        session_path,
        _directory,
        ..
    } = stored_session("finished-branches-kept");
    drop(state);
    let selection = Selection::new(crate::agent::Provider::Codex);
    let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
    let session = |parent: Option<&str>| {
        let (journal, session) =
            Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None)
                .unwrap();
        let path = journal.path().parent().unwrap().to_path_buf();
        if let Some(parent) = parent {
            journal::store_session_origin(
                &path,
                SessionOrigin {
                    session_id: parent.into(),
                    provider: selection.provider,
                    at_ms: None,
                    history: Default::default(),
                },
            )
            .unwrap();
        }
        journal::store_session_completed(&path, CompletionState::Completed).unwrap();
        (session, path)
    };
    let (child, child_path) = session(Some(&id));
    let (grandchild, grandchild_path) = session(Some(&child));
    let (unrelated, unrelated_path) = session(None);
    let row = |id: &str, path: &Path| {
        (
            path.to_path_buf(),
            listed_interaction(id, &workspace.id, &host, InteractionActivity::Stopped),
            DrivaOptions::default(),
        )
    };
    crate::roster::Roster::open(&store).publish(vec![
        row(&id, &session_path),
        row(&child, &child_path),
        row(&grandchild, &grandchild_path),
        row(&unrelated, &unrelated_path),
    ]);
    let state = ServerState::new(store.clone(), store.with_extension("sock"));
    let Response::Interactions(rows) = state
        .handle(Request::ListInteractions(Some(InteractionQuery {
            skip_done: true,
            keep_branches_of: vec![id.clone()],
            ..Default::default()
        })))
        .unwrap()
    else {
        panic!("expected interactions");
    };
    let mut listed: Vec<_> = rows.into_iter().map(|row| row.id).collect();
    listed.sort();
    let mut expected = vec![id, child, grandchild];
    expected.sort();
    assert_eq!(listed, expected);
}

#[test]
fn branches_are_found_through_branches_and_a_cycle_ends() {
    let origins = [
        ("child".to_owned(), "root".to_owned()),
        ("grandchild".to_owned(), "child".to_owned()),
        ("other".to_owned(), "elsewhere".to_owned()),
        ("root".to_owned(), "grandchild".to_owned()),
    ];
    let mut branches = branches_of(&["root".to_owned()], &origins);
    branches.sort();
    assert_eq!(branches, ["child", "grandchild"]);
}
