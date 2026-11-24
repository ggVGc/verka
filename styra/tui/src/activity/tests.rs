use super::*;

fn in_status(status: Status) -> Activity {
    Activity {
        status,
        ..Default::default()
    }
}

/// A provider that reports its background set owns the answer from then
/// on, so a heuristic guess afterwards must not contradict it.
#[test]
fn a_reported_count_takes_over_from_the_tool_call_heuristic() {
    let mut activity = Activity::default();
    activity.note_background_started();
    assert_eq!(
        activity.idle_or_background(IdleReason::TurnComplete),
        Status::Background
    );

    activity.note_background_count(2);
    activity.note_background_finished();

    assert_eq!(
        activity.idle_or_background(IdleReason::TurnComplete),
        Status::Background,
        "one finished tool call does not clear a reported set of two"
    );
}

#[test]
fn a_reported_empty_set_clears_a_background_status_on_the_spot() {
    let mut activity = Activity::default();
    activity.note_background_started();
    activity.status = Status::Background;

    activity.note_background_count(0);

    assert_eq!(
        activity.status,
        Status::Idle(IdleReason::BackgroundFinished)
    );
    assert_eq!(
        activity.idle_or_background(IdleReason::TurnComplete),
        Status::Idle(IdleReason::TurnComplete)
    );
}

/// Only a Background status is displaced by the count reaching zero; an
/// Interaction that is mid-turn stays running.
#[test]
fn a_cleared_count_leaves_a_running_turn_alone() {
    let mut activity = in_status(Status::Running);

    activity.note_background_count(0);

    assert_eq!(activity.status, Status::Running);
}

#[test]
fn the_heuristic_still_applies_while_the_provider_stays_silent() {
    let mut activity = Activity::default();
    activity.note_background_started();
    activity.status = Status::Background;

    activity.note_background_finished();

    assert_eq!(
        activity.status,
        Status::Idle(IdleReason::BackgroundFinished)
    );
}

/// Attaching to an interaction that has been working for a while shows how
/// long *it* has been working, not how long this client has been looking.
#[test]
fn an_adopted_status_is_dated_from_the_servers_moment_not_the_attachment() {
    let mut activity = Activity::default();

    activity.adopt_server_status_at(Status::Running, 1_000_000, 1_600_000);

    assert_eq!(activity.status, Status::Running);
    assert!(
        activity.progress().in_status >= Duration::from_secs(600),
        "the turn started ten minutes before this client saw it"
    );
    // Adopting is the whole transition, so the next frame must not read as
    // a fresh one and reset the clock it just set.
    activity.note_progress();
    assert!(activity.progress().in_status >= Duration::from_secs(600));
}

/// A server too old to date the activity sends `0`, and clocks that
/// disagree can date it in the future. Neither may be turned into an age.
#[test]
fn an_unreported_or_future_moment_dates_the_status_now() {
    for since_ms in [0, 2_000_000] {
        let mut activity = Activity::default();
        activity.adopt_server_status_at(Status::Running, since_ms, 1_000_000);
        assert!(activity.progress().in_status < Duration::from_secs(1));
    }
}

/// Adopting Background brings the flag it stands for with it: otherwise
/// the turn that follows would fall back to plain Idle and lose the
/// interaction's background work.
#[test]
fn adopting_background_carries_the_background_work_it_reports() {
    let mut activity = Activity::default();

    activity.adopt_server_status_at(Status::Background, 0, 0);

    assert_eq!(
        activity.idle_or_background(IdleReason::TurnComplete),
        Status::Background
    );

    activity.adopt_server_status_at(Status::Idle(IdleReason::Reported), 0, 0);

    assert_eq!(
        activity.idle_or_background(IdleReason::TurnComplete),
        Status::Idle(IdleReason::TurnComplete)
    );
}

#[test]
fn progress_counts_what_has_arrived_and_when_the_status_last_changed() {
    let mut activity = Activity::default();
    assert_eq!(activity.progress().events, 0);
    assert!(activity.progress().since_event.is_none());

    activity.note_event_received();
    activity.note_event_received();

    assert_eq!(activity.progress().events, 2);
    assert!(activity.progress().since_event.is_some());
}

/// The clock restarts only when the status actually changes, so a frame
/// that changed nothing does not read as a fresh transition.
#[test]
fn the_status_clock_restarts_only_on_a_real_change() {
    let mut activity = in_status(Status::Idle(IdleReason::TurnComplete));
    activity.note_progress();
    let first = activity.progress().in_status;

    activity.note_progress();

    assert!(
        activity.progress().in_status >= first,
        "same status, same clock"
    );
}

/// Two sessions both sitting idle are not in the same situation, and the
/// status has to be able to say so — that is the whole point of carrying
/// the reason.
#[test]
fn two_idle_statuses_with_different_reasons_are_different_statuses() {
    assert_ne!(
        Status::Idle(IdleReason::TurnComplete),
        Status::Idle(IdleReason::Interrupted)
    );
    assert_ne!(
        Status::Stopped(StopReason::Paused),
        Status::Stopped(StopReason::NotAccepting)
    );
    // Both are still idle, though, for every caller that only asks that.
    assert!(Status::Idle(IdleReason::Interrupted).is_idle());
}

/// The reason joins the word the operator already reads, and the ordinary
/// cases stay the bare word they have always been.
#[test]
fn a_reason_worth_naming_reaches_the_label() {
    assert_eq!(Status::Idle(IdleReason::TurnComplete).label(), "idle");
    assert_eq!(
        Status::Idle(IdleReason::Interrupted).label(),
        "idle · interrupted"
    );
    assert_eq!(
        Status::Idle(IdleReason::RateLimited(RateLimit {
            window: "five_hour".into(),
            resets_at_ms: None,
        }))
        .label(),
        "idle · rate limited (five_hour)"
    );
    assert_eq!(
        Status::Stopped(StopReason::Paused).label(),
        "stopped · paused"
    );
    assert_eq!(
        Status::Stopped(StopReason::ServerRestarted).label(),
        "idle · no agent since server restart"
    );
    assert_eq!(
        Status::Stopped(StopReason::ServerRestartedMidTurn).label(),
        "idle · interrupted"
    );
    assert_eq!(
        Status::Stopped(StopReason::Branched).label(),
        "idle · branched, not started"
    );
}

fn reported(
    activity: InteractionActivity,
    reason: Option<InteractionActivityReason>,
) -> InteractionSummary {
    InteractionSummary {
        id: "s1".into(),
        name: None,
        tags: Vec::new(),
        workspace_id: "w1".into(),
        selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
        workspace: std::path::PathBuf::from("/workspace"),
        activity,
        activity_reason: reason,
        activity_since_ms: 0,
        idle_unseen: false,
        interacted_at_ms: 0,
        last_user_message_at_ms: 0,
        uncommitted_changes: false,
        expected_branch: None,
        checkout: None,
        last_message: None,
        recent_messages: Vec::new(),
        auto_retry: false,
        auto_commit: false,
        events: 0,
        branched_from: None,
        completed: CompletionState::Active,
        focus_requested: false,
        origin: None,
        integration: None,
        restored: false,
    }
}

#[test]
fn reported_status_adopts_only_reasons_that_explain_its_state() {
    use InteractionActivity::{Pending, Stopped};
    use InteractionActivityReason::{Interrupted, Paused, RateLimited, TurnCompleted};
    for (name, activity, reason, expected) in [
        (
            "interrupted",
            Pending,
            Some(Interrupted),
            Status::Idle(IdleReason::Interrupted),
        ),
        (
            "quota refusal",
            Pending,
            Some(RateLimited {
                window: "five_hour".into(),
                resets_at_ms: Some(1_000),
            }),
            Status::Idle(IdleReason::RateLimited(limit())),
        ),
        (
            "unexplained idle",
            Pending,
            None,
            Status::Idle(IdleReason::Reported),
        ),
        (
            "unexplained stop",
            Stopped,
            None,
            Status::Stopped(StopReason::NotAccepting),
        ),
        (
            "paused stop",
            Stopped,
            Some(Paused),
            Status::Stopped(StopReason::Paused),
        ),
        (
            "turn reason cannot explain stop",
            Stopped,
            Some(TurnCompleted),
            Status::Stopped(StopReason::NotAccepting),
        ),
        (
            "stop reason cannot explain idle",
            Pending,
            Some(Paused),
            Status::Idle(IdleReason::Reported),
        ),
    ] {
        assert_eq!(
            Status::reported(&reported(activity, reason)),
            expected,
            "{name}"
        );
    }
}

/// Integration is what sealed the Session, so it is what the status names;
/// until integration has cleaned up after it, the status says that is
/// still to come, whatever else stopped the interaction.
#[test]
fn an_integrated_interaction_reads_as_integrated() {
    let mut interaction = reported(
        InteractionActivity::Stopped,
        Some(InteractionActivityReason::Paused),
    );
    interaction.integration = Some(Integration {
        branch: Some("main".into()),
        commit: "0123456789abcdef".into(),
    });
    assert_eq!(
        Status::reported(&interaction).label(),
        "integrated · cleanup pending"
    );
    interaction.completed = CompletionState::Sealed;
    assert_eq!(
        Status::reported(&interaction).label(),
        "integrated into main · 0123456"
    );
}

fn limit() -> RateLimit {
    RateLimit {
        window: "five_hour".into(),
        resets_at_ms: Some(1_000),
    }
}

/// The agent reports the same ending whichever of these happened, so what
/// the client noticed on the way there is the only thing that can tell
/// them apart — and it has to survive until the ending arrives.
#[test]
fn a_turns_ending_is_read_from_what_happened_during_it() {
    let mut activity = Activity::default();
    assert_eq!(activity.take_turn_end_reason(), IdleReason::TurnComplete);

    activity.note_interrupt_requested();
    assert_eq!(activity.take_turn_end_reason(), IdleReason::Interrupted);

    activity.note_turn_error("context window exceeded");
    assert_eq!(
        activity.take_turn_end_reason(),
        IdleReason::Failed {
            message: "context window exceeded".into()
        }
    );

    // An ending that reports its own failure fills the gap for a provider
    // that never announced one separately...
    activity.note_turn_error_unless_known("the turn failed");
    assert_eq!(
        activity.take_turn_end_reason(),
        IdleReason::Failed {
            message: "the turn failed".into()
        }
    );
    // ...but the announcement, where there was one, is the account kept.
    activity.note_turn_error("context window exceeded");
    activity.note_turn_error_unless_known("the turn failed");
    assert_eq!(
        activity.take_turn_end_reason(),
        IdleReason::Failed {
            message: "context window exceeded".into()
        }
    );

    // A refused window outranks both: nothing ran to be interrupted or to
    // fail.
    activity.note_interrupt_requested();
    activity.note_turn_error("refused");
    activity.note_refused(limit());
    assert_eq!(
        activity.take_turn_end_reason(),
        IdleReason::RateLimited(limit())
    );
}

/// Each of those facts was about the turn that just ended. The next turn
/// ending normally has to read as one.
#[test]
fn a_turns_ending_does_not_carry_over_to_the_next_one() {
    let mut activity = Activity::default();
    activity.note_interrupt_requested();
    activity.note_turn_error("broken");
    activity.note_refused(limit());

    activity.take_turn_end_reason();

    assert_eq!(activity.take_turn_end_reason(), IdleReason::TurnComplete);
}

/// An ending nobody attributed still says as much as the exit code allows.
#[test]
fn an_ending_reads_its_reason_off_the_exit() {
    assert_eq!(EndReason::infer(Some(0), None), EndReason::Completed);
    assert_eq!(EndReason::infer(Some(1), None), EndReason::Failed);
    assert_eq!(
        EndReason::infer(Some(0), Some("broken pipe")),
        EndReason::Failed
    );
    assert_eq!(EndReason::infer(None, None), EndReason::Unknown);
}
