use super::*;
use crate::activity::{EndReason, IdleReason, StopReason};
use styra_protocol::event::{TokenUsage, TurnOutcome, TurnUsage};
use styra_protocol::RawLine;
use styra_protocol::{Answer, AnswerValue, FileLocation};

/// A session app with every default these tests would otherwise inherit
/// pinned explicitly: the profile names its model and effort instead of
/// letting the provider resolve them, and both timeline filters are set
/// rather than left to `Timeline::default`. A test that inherits a default
/// is really asserting it, and breaks when the product changes its mind
/// about something the test had no opinion on.
fn app() -> App {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        "session-1",
    );
    app.timeline.all_events = true;
    app.timeline.show_minor = false;
    app
}

/// Switching which Interaction is on screen rebuilds the whole screen from
/// the server, so anything the operator put there has to be carried over
/// deliberately.
#[test]
fn display_choices_survive_the_screen_they_were_made_on() {
    let mut app = app();
    app.outbox.set_contract(Some(Contract::Lines));
    app.preview.show();
    app.toggle_link_display();
    app.files.set_scope(true);
    app.interactions.toggle_workspace_scope();
    app.recent_models = vec!["gpt-5.6-sol".into()];

    let mut next = App::new(app.selection.clone(), "session-2");
    next.adopt(app.take_operator_state());

    assert_eq!(next.outbox.contract(), Some(Contract::Lines));
    assert!(next.preview.open);
    assert_eq!(next.link_display, LinkDisplay::Full);
    assert!(next.files.shows_all());
    assert!(next.interactions.only_current_workspace);
    assert_eq!(next.recent_models, vec!["gpt-5.6-sol".to_owned()]);
}

#[test]
fn the_interactions_listing_keeps_where_branch_markers_lead() {
    use styra_protocol::event::BranchDirection;
    let mut app = App::new(app().selection.clone(), "session-1");
    for (direction, session) in [
        (BranchDirection::From, "source"),
        (BranchDirection::To, "branch"),
        (BranchDirection::To, "branch"),
    ] {
        app.push_event(AgentEvent::Branched {
            direction,
            session: session.into(),
            name: None,
        });
    }
    let query = app.interactions_query();
    assert!(query.skip_done);
    assert_eq!(query.keep, ["session-1", "source", "branch"]);
    assert_eq!(
        query.keep_branches_of,
        ["branch"],
        "the subtree beneath a `branched to` marker, finished branches too"
    );
}

#[test]
fn integration_drafts_follow_their_interaction_across_screen_switches() {
    let mut first = app();
    first.integration_prompt = Some("First draft\nwith details".into());

    let mut second = App::new(first.selection.clone(), "session-2");
    second.workspace.name = Some("payments".into());
    second
        .interactions
        .note_name("session-2", Some("Second interaction".into()));
    second.adopt(first.take_operator_state());
    assert!(second.integration_prompt.is_none());
    second.open_integration_prompt();
    assert_eq!(
        second.integration_prompt.as_deref(),
        Some("payments: Second interaction")
    );
    second.integration_prompt = Some("Second draft".into());
    second.close_integration_prompt();

    first.adopt(second.take_operator_state());
    first.open_integration_prompt();
    assert_eq!(
        first.integration_prompt.as_deref(),
        Some("First draft\nwith details")
    );

    second.adopt(first.take_operator_state());
    second.open_integration_prompt();
    assert_eq!(second.integration_prompt.as_deref(), Some("Second draft"));
}

/// The log is partly the server's, replayed with the rest of an
/// Interaction's history, so a screen that carried it across a re-attach
/// would show every entry twice. The message box is the Session's too: a
/// draft written to one interaction is not sent to the next.
#[test]
fn the_interactions_own_state_is_left_to_be_rebuilt() {
    let mut app = app();
    app.set_input("half a thought".into());
    app.push_log(LogEntry::info("something happened"));
    app.raw.push(RawLine {
        direction: styra_protocol::Direction::FromAgent,
        text: "{}".into(),
        at_ms: 0,
    });

    let mut next = App::new(app.selection.clone(), "session-2");
    next.adopt(app.take_operator_state());

    assert!(next.log.is_empty());
    assert!(next.raw.is_empty());
    assert_eq!(next.composer.parts(), [""]);
    assert_eq!(next.timeline.entries.len(), 0);
}

#[test]
fn following_tracks_visible_entries_and_transfers_their_expansion() {
    for hidden_start in [false, true] {
        let mut app = app();
        if hidden_start {
            app.push_event(AgentEvent::TurnStarted);
        }
        app.push_event(AgentEvent::AgentMessage {
            text: "first".into(),
        });
        let first = app.timeline.selected;
        assert_eq!(first, usize::from(hidden_start));
        assert!(app.timeline.follow);
        app.timeline.entries[first].expanded = true;
        app.push_event(AgentEvent::AgentMessage {
            text: "second".into(),
        });
        assert!(app.timeline.follow);
        assert_eq!(app.timeline.selected, first + 1);
        assert!(!app.timeline.entries[first].expanded);
        assert!(app.timeline.entries[first + 1].expanded);
    }
}

/// Returning to a source selects the marker of the branch just left,
/// whichever other markers the journal carries.
#[test]
fn the_marker_of_the_branch_left_is_selected_on_return() {
    use styra_protocol::event::BranchDirection;

    let mut app = app();
    let marker = |session: &str| AgentEvent::Branched {
        direction: BranchDirection::To,
        session: session.into(),
        name: None,
    };
    app.push_event(marker("branch-2"));
    app.push_event(marker("branch-3"));
    app.push_event(AgentEvent::AgentMessage {
        text: "after".into(),
    });
    app.select_last();

    assert!(app.select_branch_marker("branch-2"));
    assert_eq!(app.timeline.selected, 0);
    assert!(!app.timeline.follow);
    assert_eq!(app.timeline.list_anchor, ListAnchor::Centered);
    assert_eq!(app.selected_branch_target().as_deref(), Some("branch-2"));

    assert!(!app.select_branch_marker("elsewhere"));
    assert_eq!(app.timeline.selected, 0);
}

#[test]
fn following_ignores_hidden_minor_events() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage { text: "hi".into() });
    app.timeline.entries[0].expanded = true;
    app.preview.scroll.offset = 3;

    app.push_event(AgentEvent::TurnStarted);

    assert!(app.timeline.follow);
    assert_eq!(app.timeline.selected, 0);
    assert_eq!(app.preview.scroll.offset, 3);
    assert!(app.timeline.is_visible(app.timeline.selected));
    assert!(app.timeline.entries[0].expanded);
    assert!(!app.timeline.entries[1].expanded);
}

#[test]
fn a_new_message_clears_the_interaction_opening_anchor() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage { text: "hi".into() });
    app.select_last_on_interaction_open();
    assert_eq!(app.timeline.list_anchor, ListAnchor::Centered);

    app.push_event(AgentEvent::UserMessage {
        text: "next".into(),
    });

    assert_eq!(app.timeline.list_anchor, ListAnchor::Selection);
}

#[test]
fn moving_up_pins_the_view_and_reaching_the_tail_resumes_follow() {
    let mut app = app();
    for _ in 0..3 {
        app.push_event(AgentEvent::AgentMessage { text: "x".into() });
    }
    app.select_prev_line();
    assert!(!app.timeline.follow);
    assert_eq!(app.timeline.selected, 1);

    // New events no longer move the selection while pinned.
    app.push_event(AgentEvent::AgentMessage { text: "x".into() });
    assert_eq!(app.timeline.selected, 1);

    // Walking back down to the tail re-enables follow.
    app.select_next_line();
    app.select_next_line();
    app.select_next_line();
    assert!(app.timeline.follow);
    assert_eq!(app.timeline.selected, app.timeline.entries.len() - 1);
}

#[test]
fn scrolling_a_freshly_opened_interaction_releases_its_initial_anchor() {
    for (scroll, delta) in [
        (App::scroll_interaction_down as fn(&mut App), 10),
        (App::scroll_interaction_up as fn(&mut App), -10),
    ] {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage {
            text: "latest response".into(),
        });
        app.select_last_on_interaction_open();
        assert_eq!(app.timeline.list_anchor, ListAnchor::Centered);
        let selected = app.timeline.selected;

        scroll(&mut app);

        assert_eq!(app.timeline.list_anchor, ListAnchor::Free);
        assert_eq!(app.timeline.list_scroll_delta, delta);
        assert_eq!(app.timeline.selected, selected);
    }
}

#[test]
fn scrolling_the_interaction_does_not_move_its_selection() {
    let mut app = app();
    for n in 0..3 {
        app.push_event(AgentEvent::AgentMessage {
            text: format!("message {n}"),
        });
    }
    let selected = app.timeline.selected;

    app.scroll_interaction_up();
    assert_eq!(app.timeline.selected, selected);
    assert_eq!(app.timeline.list_anchor, ListAnchor::Free);
    assert_eq!(app.timeline.list_scroll_delta, -10);

    app.timeline.list_offset = 1;
    app.scroll_interaction_down();
    assert_eq!(app.timeline.list_offset, 1, "only rendering resolves rows");
    assert_eq!(app.timeline.list_scroll_delta, 0);
    assert_eq!(app.timeline.selected, selected);

    // While the selection is cut off, `j`/`k` scroll it as `J`/`K` do,
    // but no further than its edge, even before the next frame.
    app.timeline.rendered_selection = Some(selected);
    app.timeline.selection_overflow = Some(styra_ui::event_list::SelectionOverflow {
        above: 0,
        below: 14,
    });
    app.select_next_line();
    app.select_next_line();
    assert_eq!(app.timeline.list_scroll_delta, 14);
    app.select_prev_line();
    app.select_prev_line();
    assert_eq!(app.timeline.list_scroll_delta, 0);
    assert_eq!(app.timeline.selected, selected);

    // All of it shows, so this step leaves it.
    app.select_prev_line();
    assert_ne!(
        app.timeline.list_anchor,
        ListAnchor::Free,
        "selection navigation reanchors"
    );
}

#[test]
fn a_line_step_past_the_end_leaves_a_scrolled_last_entry_where_it_is() {
    let mut app = app();
    for n in 0..3 {
        app.push_event(AgentEvent::AgentMessage {
            text: format!("message {n}"),
        });
    }
    let last = app.timeline.selected;
    // Scrolled down to the end of the last entry, past its first rows.
    app.timeline.list_anchor = ListAnchor::Free;
    app.timeline.rendered_selection = Some(last);
    app.timeline.selection_overflow = Some(styra_ui::event_list::SelectionOverflow {
        above: 14,
        below: 0,
    });

    app.select_next_line();
    assert_eq!(app.timeline.selected, last);
    assert_eq!(app.timeline.list_anchor, ListAnchor::Free);
    assert_eq!(app.timeline.list_scroll_delta, 0);

    // Scrolled away from the selection entirely, the step still brings
    // the view back to it.
    app.timeline.selection_overflow = None;
    app.select_next_line();
    assert_eq!(app.timeline.list_anchor, ListAnchor::Kept);
}

#[test]
fn thinking_updates_fold_into_one_line_that_adds_up_what_they_spent() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage { text: "a".into() });
    app.push_event(AgentEvent::Thinking {
        text: "weigh the options".into(),
        tokens: None,
    });
    app.push_event(AgentEvent::Thinking {
        text: String::new(),
        tokens: Some(64),
    });
    app.push_event(AgentEvent::Thinking {
        text: String::new(),
        tokens: Some(512),
    });

    // Each tick reports only its own spend, so the one line shows the
    // total for the run rather than whatever the last tick happened to
    // report — a number that only goes up while the agent thinks.
    assert_eq!(app.timeline.entries.len(), 2);
    assert_eq!(
        *app.timeline.entries[1].event(),
        AgentEvent::Thinking {
            text: "weigh the options".into(),
            tokens: Some(576),
        }
    );

    // A new run of thinking after other work starts its own line.
    app.push_event(AgentEvent::AgentMessage { text: "b".into() });
    app.push_event(AgentEvent::Thinking {
        text: String::new(),
        tokens: Some(8),
    });
    assert_eq!(app.timeline.entries.len(), 4);
}

#[test]
fn a_task_keeps_one_row_from_its_start_to_its_end() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage { text: "a".into() });
    app.push_event(AgentEvent::TaskStarted {
        id: "t-1".into(),
        description: "Rebuild and run tests".into(),
        kind: "local_agent".into(),
        agent: Some("Explore".into()),
    });
    // Another task's reports interleave with this one's; each keeps to
    // its own row.
    app.push_event(AgentEvent::TaskStarted {
        id: "t-2".into(),
        description: "Sweep the docs".into(),
        kind: "local_bash".into(),
        agent: None,
    });
    for tool_uses in ["Bash", "Read"] {
        app.push_event(AgentEvent::TaskProgress {
            id: "t-1".into(),
            description: format!("Running {tool_uses}"),
            agent: Some("Explore".into()),
            tool: Some(tool_uses.into()),
            tokens: Some(10_000),
        });
    }
    assert_eq!(app.timeline.entries.len(), 3);
    assert_eq!(
        *app.timeline.entries[1].event(),
        AgentEvent::TaskProgress {
            id: "t-1".into(),
            description: "Running Read".into(),
            agent: Some("Explore".into()),
            tool: Some("Read".into()),
            tokens: Some(10_000),
        }
    );

    // Claude ends a task twice, and neither ending is complete on its
    // own: the notification names the task, the patch says why it failed.
    app.push_event(AgentEvent::TaskCompleted {
        id: "t-1".into(),
        status: "failed".into(),
        summary: "Rebuild and run tests".into(),
        error: None,
    });
    app.push_event(AgentEvent::TaskCompleted {
        id: "t-1".into(),
        status: "failed".into(),
        summary: String::new(),
        error: Some("exit code 101".into()),
    });
    assert_eq!(app.timeline.entries.len(), 3);
    assert_eq!(
        *app.timeline.entries[1].event(),
        AgentEvent::TaskCompleted {
            id: "t-1".into(),
            status: "failed".into(),
            summary: "Rebuild and run tests".into(),
            error: Some("exit code 101".into()),
        }
    );

    // A progress report that arrives after the ending must not put a
    // finished task back to running.
    app.push_event(AgentEvent::TaskProgress {
        id: "t-1".into(),
        description: "Running Bash".into(),
        agent: Some("Explore".into()),
        tool: Some("Bash".into()),
        tokens: Some(20_000),
    });
    assert!(matches!(
        *app.timeline.entries[1].event(),
        AgentEvent::TaskCompleted { .. }
    ));
    assert_eq!(app.timeline.entries.len(), 3);
}

#[test]
fn expansion_is_per_entry_and_bulk_toggles_work() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage { text: "a".into() });
    app.push_event(AgentEvent::AgentMessage { text: "b".into() });

    app.select_first();
    app.timeline.toggle_expand();
    assert!(app.timeline.entries[0].expanded);
    assert!(!app.timeline.entries[1].expanded);

    app.timeline.expand_all();
    assert!(app.timeline.entries.iter().all(|entry| entry.expanded));
    app.timeline.collapse_all();
    assert!(app.timeline.entries.iter().all(|entry| !entry.expanded));
}

#[test]
fn expanding_only_selected_collapses_every_other_entry() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage { text: "a".into() });
    app.push_event(AgentEvent::AgentMessage { text: "b".into() });
    app.push_event(AgentEvent::AgentMessage { text: "c".into() });
    app.timeline.expand_all();

    app.select_first();
    app.select_next_line();
    app.timeline.expand_only_selected();

    assert!(!app.timeline.entries[0].expanded);
    assert!(app.timeline.entries[1].expanded);
    assert!(!app.timeline.entries[2].expanded);
}

/// A turn that failed ends like any other, and the reason it ends with is
/// the one it carries. Claude reports the failure only on the line that
/// ends the turn, so before that ending existed this session sat running
/// forever with nothing to say.
#[test]
fn a_failed_ending_ends_the_turn_with_its_own_reason() {
    let mut app = app();
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Failed {
            message: "hit the turn limit".into(),
        },
        usage: TurnUsage::default(),
    });
    assert_eq!(
        app.activity.status,
        Status::Idle(IdleReason::Failed {
            message: "hit the turn limit".into()
        })
    );
    // And the row is visible with the bookkeeping hidden, which it is here.
    assert!(!app.timeline.show_minor);
    assert!(
        (0..app.timeline.entries.len()).any(|idx| app.timeline.is_visible(idx)
            && matches!(
                app.timeline.entries[idx].event(),
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Failed { .. },
                    ..
                }
            ))
    );
}

#[test]
fn status_follows_turn_lifecycle_and_captures_usage() {
    let mut app = app();
    assert_eq!(app.activity.status, Status::Running);
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage {
            turn: Some(TokenUsage {
                input_tokens: 7,
                ..Default::default()
            }),
            total: None,
        },
    });
    assert_eq!(app.activity.status, Status::Idle(IdleReason::TurnComplete));
    // A provider that reports only what the turn cost still moves the
    // running total the status line shows.
    assert_eq!(app.activity.latest_usage.as_ref().unwrap().input_tokens, 7);

    app.push_event(AgentEvent::UserMessage {
        text: "more".into(),
    });
    assert_eq!(app.activity.status, Status::Running);
}

/// Framed turns show the operator's own message and contract; ordinary
/// messages stay untouched, even when they mention answer blocks.
#[test]
fn operator_messages_preserve_their_text_and_contract() {
    for (text, contract) in [
        ("which files handle auth?", Some(Contract::Files)),
        ("explain the <styra:answer> convention", None),
    ] {
        let mut app = app();
        let received = match contract {
            Some(contract) => styra_protocol::contract::frame(text, contract),
            None => text.into(),
        };
        app.push_event(AgentEvent::UserMessage { text: received });
        let entry = app.timeline.entries.last().expect("the message was pushed");
        assert_eq!(
            *entry.event(),
            AgentEvent::UserMessage { text: text.into() }
        );
        assert_eq!(entry.contract, contract);
    }
}

#[test]
fn background_task_keeps_idle_status_explicitly_active() {
    let mut app = app();
    app.push_event(AgentEvent::ToolStarted {
        id: "bash-1".into(),
        name: "Bash".into(),
        detail: r#"{"command":"cargo test","run_in_background":true}"#.into(),
    });
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });
    assert_eq!(app.activity.status, Status::Background);
    assert_eq!(
        app.activity.status.label(),
        "idle · background work running"
    );

    app.push_event(AgentEvent::ToolStarted {
        id: "poll-1".into(),
        name: "TaskOutput".into(),
        detail: r#"{"task_id":"bash-1"}"#.into(),
    });
    app.push_event(AgentEvent::ToolCompleted {
        id: "poll-1".into(),
        name: "poll-1".into(),
        detail: String::new(),
        status: "completed".into(),
        output: "Task completed successfully".into(),
    });
    assert_eq!(
        app.activity.status,
        Status::Idle(IdleReason::BackgroundFinished)
    );
}

#[test]
fn reported_empty_background_set_clears_the_status_without_a_poll() {
    let mut app = app();
    app.push_event(AgentEvent::ToolStarted {
        id: "bash-1".into(),
        name: "Bash".into(),
        detail: r#"{"command":"cargo test","run_in_background":true}"#.into(),
    });
    app.push_event(AgentEvent::BackgroundTasks { running: 1 });
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });
    assert_eq!(app.activity.status, Status::Background);

    // The agent never polls the task; it reads the output file directly
    // and the provider reports the set is empty. That must clear.
    app.push_event(AgentEvent::BackgroundTasks { running: 0 });
    assert_eq!(
        app.activity.status,
        Status::Idle(IdleReason::BackgroundFinished)
    );
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });
    assert_eq!(app.activity.status, Status::Idle(IdleReason::TurnComplete));
}

#[test]
fn a_poll_of_one_task_does_not_clear_a_reported_second_one() {
    let mut app = app();
    app.push_event(AgentEvent::BackgroundTasks { running: 2 });
    app.push_event(AgentEvent::ToolStarted {
        id: "poll-1".into(),
        name: "TaskOutput".into(),
        detail: r#"{"task_id":"one"}"#.into(),
    });
    app.push_event(AgentEvent::ToolCompleted {
        id: "poll-1".into(),
        name: "poll-1".into(),
        detail: String::new(),
        status: "completed".into(),
        output: "Task completed successfully".into(),
    });
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });
    assert_eq!(app.activity.status, Status::Background);
}

#[test]
fn usage_updates_mid_turn_refresh_the_display_without_ending_the_turn() {
    // The app-server protocol reports a token-usage snapshot after every
    // step within a turn (each tool call, each model round), not just the
    // last one. Only a real `TurnCompleted` should flip the status to
    // idle; `UsageUpdated` must not, or the indicator falsely reads idle
    // while the agent is still actively working.
    let mut app = app();
    app.push_event(AgentEvent::CommandStarted {
        command: "cargo test".into(),
    });
    assert_eq!(app.activity.status, Status::Running);

    app.push_event(AgentEvent::UsageUpdated {
        usage: TokenUsage {
            input_tokens: 10,
            ..Default::default()
        },
    });
    assert_eq!(
        app.activity.status,
        Status::Running,
        "a usage ping mid-turn must not end it"
    );
    assert_eq!(app.activity.latest_usage.as_ref().unwrap().input_tokens, 10);

    app.push_event(AgentEvent::CommandStarted {
        command: "cargo build".into(),
    });
    app.push_event(AgentEvent::UsageUpdated {
        usage: TokenUsage {
            input_tokens: 20,
            ..Default::default()
        },
    });
    assert_eq!(app.activity.status, Status::Running);
    assert_eq!(app.activity.latest_usage.as_ref().unwrap().input_tokens, 20);

    // The app-server's real end-of-turn signal carries no usage of its
    // own; the last reported usage must survive it, not reset to zero.
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });
    assert_eq!(app.activity.status, Status::Idle(IdleReason::TurnComplete));
    assert_eq!(app.activity.latest_usage.as_ref().unwrap().input_tokens, 20);
}

#[test]
fn a_tool_completion_replaces_its_started_row_and_recovers_the_tool_name() {
    // Claude's `tool_result` only ever repeats the `tool_use_id`, never
    // the tool's name, so the merge must pull the name back from the
    // matching `ToolStarted` row rather than leaving the placeholder id
    // on screen, and it must not append a second line.
    let mut app = app();
    app.push_event(AgentEvent::ToolStarted {
        id: "toolu_1".into(),
        name: "Bash".into(),
        detail: "{\"command\":\"cargo test\"}".into(),
    });
    app.push_event(AgentEvent::ToolCompleted {
        id: "toolu_1".into(),
        name: "toolu_1".into(),
        detail: String::new(),
        status: "completed".into(),
        output: "ok".into(),
    });

    assert_eq!(app.timeline.entries.len(), 1);
    assert_eq!(
        *app.timeline.entries[0].event(),
        AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into(),
            status: "completed".into(),
            output: "ok".into(),
        }
    );
}

#[test]
fn edit_completion_keeps_the_change_or_replaces_it_with_the_error() {
    for (id, status, output) in [
        ("toolu_2", "completed", ""),
        ("toolu_3", "error", "old_string not found"),
    ] {
        let mut app = app();
        // Edits arrive as FileChanged rather than ToolStarted; completion
        // must not append a second, id-only entry.
        let changed = AgentEvent::FileChanged {
            id: id.into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ edit @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        };
        app.push_event(changed.clone());
        app.push_event(AgentEvent::ToolCompleted {
            id: id.into(),
            name: id.into(),
            detail: String::new(),
            status: status.into(),
            output: output.into(),
        });
        assert_eq!(app.timeline.entries.len(), 1, "{status}");
        let expected = if status == "error" {
            AgentEvent::Error {
                message: "src/lib.rs: old_string not found".into(),
            }
        } else {
            changed
        };
        assert_eq!(*app.timeline.entries[0].event(), expected, "{status}");
    }
}

#[test]
fn ending_is_terminal_for_events_but_not_can_send() {
    let mut app = app();
    app.on_ended(InteractionEnd {
        exit_code: Some(0),
        error: None,
    });
    assert_eq!(app.activity.status, Status::ended(Some(0), None));
    // `can_send` only flags the input box's title; the event loop still
    // lets a new message resume an ended Session through its provider.
    assert!(!app.can_send());
    // A late event does not revive an ended session.
    app.push_event(AgentEvent::AgentMessage {
        text: "late".into(),
    });
    assert!(matches!(app.activity.status, Status::Ended { .. }));
}

/// The process going after the operator stopped the interaction is the
/// stop finishing, not an agent that quit on its own — and a clean exit
/// code cannot tell those apart, which is why the ending reads the status
/// it displaces.
#[test]
fn a_process_that_goes_after_a_pause_ended_because_of_the_pause() {
    let mut app = app();
    app.activity.status = Status::Stopped(StopReason::Paused);

    app.on_ended(InteractionEnd {
        exit_code: Some(0),
        error: None,
    });

    assert_eq!(
        app.activity.status,
        Status::Ended {
            exit_code: Some(0),
            error: None,
            reason: EndReason::Stopped,
        }
    );
}

/// A spent window is why the work stopped, so the reading that announces
/// it has to reach the status and not just the quota view.
#[test]
fn a_refused_window_becomes_the_reason_the_turn_ended() {
    let mut app = app();
    app.note_quota(QuotaEvent {
        at_ms: 1,
        session_id: "session-1".into(),
        provider: Provider::Claude,
        window: "five_hour".into(),
        status: QuotaStatus::Exhausted,
        utilization: Some(1.0),
        resets_at_ms: Some(2_000),
        detail: None,
    });

    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });

    assert_eq!(
        app.activity.status,
        Status::Idle(IdleReason::RateLimited(RateLimit {
            window: "five_hour".into(),
            resets_at_ms: Some(2_000),
        }))
    );
}

/// Entering an interaction replays its announcements; a warning this
/// client has already shown stays in the log but is not raised again.
#[test]
fn a_quota_warning_is_noticed_once_across_re_entering() {
    let reading = QuotaEvent {
        at_ms: 1,
        session_id: "session-1".into(),
        provider: Provider::Claude,
        window: "five_hour".into(),
        status: QuotaStatus::Warning,
        utilization: Some(0.92),
        resets_at_ms: None,
        detail: None,
    };
    let mut first = app();
    first.note_quota(reading.clone());
    assert_eq!(first.notices.iter().count(), 1);

    let mut entered = app();
    entered.quota_noticed = first.quota_noticed.clone();
    entered.note_quota(reading);
    entered.adopt(first.take_operator_state());
    assert!(entered.notices.is_empty());
    assert_eq!(entered.log.iter().count(), 1);
    assert_eq!(entered.quota_noticed.len(), 1);
}

#[test]
fn pending_opens_in_list_focus_with_no_session_yet_and_allows_sending() {
    let app = App::pending(Selection::new(Provider::Codex));
    assert_eq!(app.activity.status, Status::Pending);
    assert_eq!(app.focus, Focus::List);
    assert!(app.session_id.is_empty());
    assert!(app.composer.text().is_empty());
    // The message box must not read "session ended" before anything ran.
    assert!(app.can_send());
}

#[test]
fn set_input_prefills_the_message_box_for_the_operator_to_edit_or_send() {
    let mut app = App::pending(Selection::new(Provider::Codex));
    app.set_input("earlier session's transcript".into());
    assert_eq!(app.composer.text(), "earlier session's transcript");
    assert_eq!(
        app.take_message(),
        Some("earlier session's transcript".into())
    );
}

/// The whole point of the start screen: all three of agent, model, and
/// effort are still open, and picking them only records what the first
/// message will launch. One row names all three, so the choice is one
/// query rather than three cursors.
#[test]
fn the_launcher_records_a_provider_model_and_effort_without_launching() {
    let mut app = App::pending(Selection::new(Provider::Codex));
    assert!(app.can_configure_launch());
    app.open_launcher();
    let launcher = app.launcher.as_mut().expect("the picker is reachable");
    assert_eq!(
        launcher.selection().provider,
        Provider::Codex,
        "opens on what is chosen"
    );

    // Typing the other agent's name and a rung reaches the whole triple.
    let model = Provider::Claude.default_model();
    let effort = Provider::Claude.efforts_for(model)[0];
    for character in format!("claude:{model}/{}", effort.as_str()).chars() {
        launcher.type_query(styra_ui::query::Edit::Push(character));
    }
    assert_eq!(launcher.selection().provider, Provider::Claude);

    app.confirm_launcher();
    assert!(app.launcher.is_none());
    assert_eq!(app.selection.provider, Provider::Claude);
    assert_eq!(app.selection.model, model);
    assert_eq!(app.selection.effort, effort);
}

#[test]
fn the_launcher_opens_on_the_current_selection_and_cancelling_keeps_it() {
    let selection = Selection::parse("claude:claude-sonnet-5/xhigh").unwrap();
    let mut app = App::pending(selection.clone());
    app.open_launcher();
    let launcher = app.launcher.as_ref().unwrap();
    assert_eq!(launcher.selection(), selection, "opens on what is chosen");

    app.launcher.as_mut().unwrap().next();
    app.cancel_launcher();
    assert!(app.launcher.is_none());
    assert_eq!(app.selection, selection, "cancelling changes nothing");
}

/// Confirming a model moves it to the front of the ordering, and the list
/// never grows a duplicate of a model chosen twice.
#[test]
fn confirming_records_the_model_as_the_most_recent_one() {
    let mut app = App::pending(Selection::parse("claude:claude-sonnet-5").unwrap());
    app.recent_models = vec!["claude-opus-5".into(), "claude-sonnet-5".into()];

    app.open_launcher();
    app.confirm_launcher();

    assert_eq!(
        app.recent_models,
        vec!["claude-sonnet-5".to_owned(), "claude-opus-5".to_owned()]
    );
}

/// Once a session is up, its agent and model are facts about a running
/// process; the picker is a property of the *next* one.
#[test]
fn the_launcher_is_unreachable_once_something_has_been_launched() {
    let mut app = app();
    assert_eq!(app.activity.status, Status::Running);
    assert!(!app.can_configure_launch());
    app.open_launcher();
    assert!(app.launcher.is_none());
}

/// And once that process is gone the picker is a property of the next one
/// again — the agent column included. The next message resumes the
/// Session, and the resume launches under whatever is chosen here, so
/// leaving the column out of reach would be freezing a choice nothing is
/// holding. The launch policy reopens on exactly the same rule; see
/// [`crate::launch::editable`].
#[test]
fn a_stopped_interaction_can_be_configured_again() {
    for status in [
        Status::Stopped(StopReason::Paused),
        Status::ended(Some(0), None),
    ] {
        let mut app = app();
        app.activity.status = status.clone();
        assert!(app.can_configure_launch(), "{status:?}");
        app.open_launcher();
        let launcher = app.launcher.as_ref().expect("the picker is reachable");
        assert!(!launcher.provider_locked, "{status:?}");
        assert!(
            launcher
                .labels()
                .iter()
                .any(|label| label.starts_with("claude:")),
            "every agent is a choice again: {status:?}"
        );
    }
}

/// A model chosen while nothing is running is not asked of the server:
/// there is no process to tell. It rides the resume the next message
/// triggers instead (see [`crate::session::resume_and_send`]).
#[test]
fn a_stopped_interactions_model_change_waits_for_the_resume() {
    let mut app = app();
    app.activity.status = Status::Stopped(StopReason::Paused);
    app.open_launcher();
    let launcher = app.launcher.as_mut().unwrap();
    launcher.next();
    let chosen = launcher.selection();
    app.confirm_launcher();

    assert_eq!(app.selection, chosen);
    assert_eq!(app.take_request(), None);
}

/// A stopped Session's history is written in its agent's own native
/// format, so choosing another agent for it is a conversion — and the
/// whole conversation comes along rather than the operator having to start
/// over to change agent.
#[test]
fn choosing_another_agent_for_a_stopped_session_converts_it() {
    let mut app = app();
    app.activity.status = Status::Stopped(StopReason::Paused);
    assert_eq!(app.selection.provider, Provider::Codex);
    app.open_launcher();
    let launcher = app.launcher.as_mut().unwrap();
    while launcher.selection().provider != Provider::Claude {
        launcher.next();
    }
    let chosen = launcher.selection();
    app.confirm_launcher();

    assert_eq!(app.take_request(), Some(Request::ConvertProvider(chosen)));
    // Keep showing the current selection until the server converts and
    // restarts this same Session.
    assert_eq!(app.selection.provider, Provider::Codex);
}

#[test]
fn switching_a_stopped_claude_session_keeps_the_chosen_codex_model_and_effort() {
    let mut app = App::new(Selection::new(Provider::Claude), "session-1");
    app.activity.status = Status::Stopped(StopReason::Paused);
    let chosen = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
    app.launcher = Some(Launcher::from_selection(&chosen, &[], false));
    app.confirm_launcher();

    assert_eq!(app.take_request(), Some(Request::ConvertProvider(chosen)));
    assert_eq!(app.selection.provider, Provider::Claude);
}

#[test]
fn cycling_provider_converts_a_stopped_interaction_to_the_next_defaults() {
    let mut app = app();
    app.activity.status = Status::Stopped(StopReason::Paused);
    let previous = app.selection.clone();

    app.open_provider_picker();
    assert_eq!(app.selection, previous);
    assert!(app.take_request().is_none());
    app.launcher.as_mut().unwrap().next();
    app.confirm_launcher();

    assert_eq!(
        app.take_request(),
        Some(Request::ConvertProvider(Selection::new(Provider::Claude)))
    );
    assert_eq!(app.selection, previous);
}

#[test]
fn cycling_provider_keeps_a_live_interactions_selection() {
    for status in [Status::Running, Status::Idle(IdleReason::TurnComplete)] {
        let mut app = app();
        app.activity.status = status;
        let previous = app.selection.clone();

        app.open_provider_picker();

        assert_eq!(app.selection, previous);
        assert!(app.take_request().is_none());
    }
}

/// Nothing has been launched, so there is no history to convert and no
/// sibling Session to make — the agent is simply set.
#[test]
fn choosing_another_agent_before_launch_only_records_it() {
    let mut app = App::pending(Selection::new(Provider::Codex));
    app.open_launcher();
    let launcher = app.launcher.as_mut().unwrap();
    while launcher.selection().provider != Provider::Claude {
        launcher.next();
    }
    app.confirm_launcher();

    assert_eq!(app.selection.provider, Provider::Claude);
    assert_eq!(app.take_request(), None);
}

/// A live process *is* the agent, so that column stays out of reach while
/// one is running — and the refusal stands for any other way of asking.
#[test]
fn a_live_interaction_still_cannot_change_agent() {
    let mut app = app();
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    app.open_launcher();
    let launcher = app.launcher.as_mut().unwrap();
    assert!(launcher.provider_locked);
    // The list holds only the agent the process is running, so there is no
    // row naming another one for any key to reach.
    for label in launcher.labels() {
        assert!(label.starts_with("codex:"), "{label}");
    }
    app.confirm_launcher();

    assert_eq!(app.selection.provider, Provider::Codex);
    assert_eq!(app.take_request(), None);
}

#[test]
fn an_idle_codex_launcher_applies_only_a_changed_selection() {
    let mut app = App::new(Selection::new(Provider::Codex), "session-1");
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    assert!(app.can_configure_launch());
    app.open_launcher();
    assert!(app.launcher.is_some());
    app.confirm_launcher();
    assert_eq!(app.take_request(), None, "confirming unchanged selection");
    app.open_launcher();
    app.launcher.as_mut().unwrap().next();
    app.confirm_launcher();
    assert_eq!(
        app.take_request(),
        Some(Request::ApplySelection),
        "changes reach the live agent now"
    );
}

#[test]
fn an_idle_claude_thread_can_change_model_but_not_effort() {
    let original = Selection::parse("claude:claude-sonnet-5/high").unwrap();
    let mut app = App::new(original.clone(), "session-1");
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    app.open_launcher();
    let launcher = app.launcher.as_mut().unwrap();
    while launcher.selection().model == original.model
        || launcher.selection().effort != original.effort
    {
        launcher.next();
    }
    app.confirm_launcher();

    assert_ne!(app.selection.model, original.model);
    assert_eq!(app.selection.effort, original.effort);
}

/// Before launch there is no interaction to tell; the first message
/// carries the selection.
#[test]
fn choosing_a_model_before_launch_asks_the_server_for_nothing() {
    let mut app = App::pending(Selection::new(Provider::Codex));
    app.open_launcher();
    let launcher = app.launcher.as_mut().unwrap();
    launcher.next();
    app.confirm_launcher();

    assert_eq!(app.take_request(), None);
}

#[test]
fn model_pickers_apply_valid_selections_to_idle_providers_only_on_confirmation() {
    for provider in PROVIDERS {
        let mut selection = Selection::new(provider);
        selection.effort = *provider.efforts().last().unwrap();
        let mut app = App::new(selection, "session-1");
        app.activity.status = Status::Idle(IdleReason::TurnComplete);
        app.open_model_picker();
        let labels = app.launcher.as_ref().unwrap().labels();
        for label in labels {
            let previous = app.selection.clone();
            app.open_model_picker();
            let picker = app.launcher.as_mut().unwrap();
            for ch in label.chars() {
                picker.type_query(styra_ui::query::Edit::Push(ch));
            }
            let chosen = picker.selection();
            assert_eq!(app.selection, previous);
            assert!(app.take_request().is_none());
            app.confirm_launcher();
            assert_eq!(app.selection, chosen);
            styra_protocol::agent::validate_selection(&app.selection).unwrap();
            if provider == Provider::Claude {
                assert_eq!(app.selection.effort, previous.effort);
            }
            assert_eq!(
                app.take_request(),
                (app.selection != previous).then_some(Request::ApplySelection)
            );
        }
    }
}

#[test]
fn combined_picker_keeps_live_claude_effort_fixed_and_offers_other_models() {
    let mut app = App::new(Selection::new(Provider::Claude), "session-1");
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    let original = app.selection.clone();
    app.open_model_picker();
    let picker = app.launcher.as_mut().unwrap();
    let labels = picker.labels();
    let mut other_model = false;
    for index in 0..labels.len() {
        picker.list.selected = index;
        let row = picker.selection();
        assert_eq!(row.effort, original.effort);
        other_model |= row.model != original.model;
    }
    assert!(other_model);
    assert_eq!(app.selection, original);
    assert!(app.take_request().is_none());
}

#[test]
fn a_running_turn_cannot_change_model() {
    for open_picker in [App::open_launcher, App::open_model_picker] {
        let mut app = App::new(Selection::new(Provider::Codex), "session-1");
        app.activity.status = Status::Running;
        let original = app.selection.clone();
        assert!(!app.can_configure_launch());
        open_picker(&mut app);
        assert!(app.launcher.is_none());
        assert_eq!(app.selection, original);
        assert!(app.take_request().is_none());
    }
}

/// Reports replace the launch settings they name; absent fields retain
/// the requested selection, including the provider's shorthand defaults.
#[test]
fn launch_labels_follow_reported_settings_and_keep_unreported_ones() {
    for (case, launch, report, agent, model, effort) in [
        (
            "recorded shorthand selection",
            "claude:opus/xhigh",
            None,
            "claude",
            "opus",
            "xhigh",
        ),
        (
            "unreported Claude",
            "claude:opus/max",
            None,
            "claude",
            "opus",
            "max",
        ),
        (
            "default Codex",
            "codex",
            None,
            "codex",
            Provider::Codex.default_model(),
            Provider::Codex.default_effort().as_str(),
        ),
        (
            "reported settings",
            "codex",
            Some((Some("gpt-5.6-sol"), Some("high"))),
            "codex",
            "gpt-5.6-sol",
            "high",
        ),
        (
            "report overrides request",
            "codex:gpt-5.6-luna/low",
            Some((Some("gpt-5.6-sol"), Some("xhigh"))),
            "codex",
            "gpt-5.6-sol",
            "xhigh",
        ),
        (
            "Claude omits effort",
            "claude:opus/max",
            Some((Some("claude-opus-4-8"), None)),
            "claude",
            "claude-opus-4-8",
            "max",
        ),
        (
            "empty report",
            "codex:gpt-5.6-sol/high",
            Some((None, None)),
            "codex",
            "gpt-5.6-sol",
            "high",
        ),
    ] {
        let selection = Selection::parse(launch).unwrap();
        let mut app = App::new(selection.clone(), "s-1");
        assert_eq!(app.selection, selection, "{case}: requested selection");
        if let Some((model, effort)) = report {
            app.push_event(AgentEvent::ThreadStarted {
                thread_id: "s-1".into(),
                model: model.map(str::to_owned),
                effort: effort.map(str::to_owned),
            });
        }
        let label = app.launch_label();
        assert_eq!(label.agent, agent, "{case}");
        assert_eq!(label.model.as_deref(), Some(model), "{case}");
        assert_eq!(label.effort.as_deref(), Some(effort), "{case}");
        assert_eq!(app.selection.model, model, "{case}: recorded model");
        assert_eq!(
            app.selection.effort.as_str(),
            effort,
            "{case}: recorded effort"
        );
    }
}

#[test]
fn the_launcher_opens_on_the_model_reported_by_the_interaction() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("claude:claude-sonnet-5/high").unwrap(),
        "s-1",
    );
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    app.push_event(AgentEvent::ThreadStarted {
        thread_id: "s-1".into(),
        model: Some("claude-opus-4-8".into()),
        effort: None,
    });

    app.open_launcher();
    assert_eq!(
        app.launcher
            .expect("idle interactions are configurable")
            .selection()
            .model,
        "claude-opus-4-8"
    );
}

#[test]
fn replayed_provider_switch_settles_the_chosen_model_after_switching_back() {
    let chosen = Selection::parse("claude:claude-opus-5/high").unwrap();
    let mut app = App::new(chosen.clone(), "s-1");
    app.push_event(AgentEvent::ThreadStarted {
        thread_id: "old-thread".into(),
        model: Some("claude-sonnet-5".into()),
        effort: Some("max".into()),
    });
    app.push_event(AgentEvent::ProviderChanged {
        from: "claude".into(),
        to: "codex".into(),
        model: "gpt-5.6-sol".into(),
        effort: "low".into(),
    });
    assert_eq!(app.selection.provider, Provider::Claude);
    app.push_event(AgentEvent::ProviderChanged {
        from: "codex".into(),
        to: "claude".into(),
        model: chosen.model.clone(),
        effort: chosen.effort.as_str().to_owned(),
    });
    assert_eq!(app.selection, chosen);
}

#[test]
fn replayed_model_changes_override_startup_before_opening_the_picker() {
    let mut app = App::new(Selection::new(Provider::Codex), "s-1");
    app.push_event(AgentEvent::ThreadStarted {
        thread_id: "s-1".into(),
        model: Some("gpt-5.6-sol".into()),
        effort: Some("high".into()),
    });
    app.push_event(AgentEvent::ModelChanged {
        model: Some("gpt-5.6-luna".into()),
        effort: None,
    });
    app.push_event(AgentEvent::ModelChanged {
        model: None,
        effort: Some("low".into()),
    });
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    app.open_launcher();
    assert_eq!(
        app.launcher.as_ref().unwrap().selection(),
        Selection::parse("codex:gpt-5.6-luna/low").unwrap()
    );
    app.confirm_launcher();
    assert!(app.requests.is_empty());
}

/// A converted Session replays the history of the agent it came from, that
/// agent's own model reports included. They say nothing about the agent
/// this Session now runs, so they must not settle its selection: a Session
/// converted to Claude Code that adopted a codex model would name a
/// selection — `claude:gpt-5.6-terra` — that no launch could reproduce,
/// and would offer the picker a row out of the other agent's catalog.
#[test]
fn a_converted_session_keeps_its_own_agents_model() {
    let converted = Selection::parse("claude:claude-opus-5/high").unwrap();
    let mut app = App::new(converted.clone(), "s-1");
    // The copied codex history, as `open_stored` replays it.
    app.push_event(AgentEvent::ThreadStarted {
        thread_id: "t".into(),
        model: Some("gpt-5.6-terra".into()),
        effort: Some("medium".into()),
    });
    app.push_event(AgentEvent::ModelChanged {
        model: Some("gpt-5.6-luna".into()),
        effort: Some("low".into()),
    });
    assert_eq!(app.selection, converted);

    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    app.open_launcher();
    let launcher = app.launcher.as_ref().unwrap();
    assert_eq!(launcher.selection().provider, Provider::Claude);
    for label in launcher.labels() {
        assert!(
            label.starts_with("claude:"),
            "only the agent the Session runs offers rows: {label}"
        );
    }
}

/// The same reports from the agent the Session actually runs still settle
/// it — the guard is about foreign history, not about replay.
#[test]
fn a_replayed_report_from_the_sessions_own_agent_still_settles_it() {
    let mut app = App::new(
        Selection::parse("claude:claude-opus-5/high").unwrap(),
        "s-1",
    );
    app.push_event(AgentEvent::ThreadStarted {
        thread_id: "t".into(),
        model: Some("claude-sonnet-5".into()),
        effort: Some("max".into()),
    });
    assert_eq!(
        app.selection,
        Selection::parse("claude:claude-sonnet-5/max").unwrap()
    );
}

#[test]
fn a_request_is_recorded_for_the_event_loop_and_taken_exactly_once() {
    for request in [
        Request::Quit,
        Request::Workspace,
        Request::Sessions,
        Request::OpenSession("s-1".into()),
        Request::Interactions,
        Request::NewSession,
        Request::EditFile,
    ] {
        let mut app = app();
        assert_eq!(app.take_request(), None);
        app.ask(request.clone());
        assert_eq!(app.take_request(), Some(request));
        // Taking it clears it, so the loop acts on a request once rather
        // than reopening the same picker on the next frame.
        assert_eq!(app.take_request(), None);
    }
}

#[test]
fn requests_are_ordered_and_cannot_overwrite_each_other() {
    let mut app = app();
    app.ask(Request::Templates);
    app.ask(Request::Quit);

    assert_eq!(app.take_request(), Some(Request::Templates));
    assert_eq!(app.take_request(), Some(Request::Quit));
    assert_eq!(app.take_request(), None);
}

#[test]
fn entering_raw_view_focuses_the_selected_entrys_wire_line() {
    use styra_protocol::Direction;
    let mut app = app();
    for i in 0..3 {
        app.raw.push(RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: format!("{{\"n\":{i}}}"),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: format!("message {i}"),
        });
    }
    // Step off the tail so the list is no longer following, landing on
    // the middle entry.
    app.select_prev_line();
    assert_eq!(app.timeline.selected, 1);
    assert!(!app.timeline.follow);

    app.toggle_raw();
    assert_eq!(
        app.raw.selected_index(),
        1,
        "focuses the wire line behind the selected entry"
    );
    assert!(!app.raw.is_following());
}

#[test]
fn log_view_toggles_independently_and_scrolls() {
    use styra_protocol::LogEntry;
    let mut app = app();
    app.toggle_raw();
    assert_eq!(app.view, View::Raw);
    // Toggling the log from the raw view switches to it, not back to events.
    app.toggle_view(View::Log);
    assert_eq!(app.view, View::Log);
    app.toggle_view(View::Log);
    assert_eq!(app.view, View::Events);

    for i in 0..4 {
        app.push_log(LogEntry::info(format!("entry {i}")));
    }
    assert_eq!(app.log.scroll_back(), 0);
    app.log.scroll_up();
    assert_eq!(app.log.scroll_back(), 1);
    app.push_log(LogEntry::warn("more"));
    assert_eq!(app.log.scroll_back(), 2, "scrolled-up view stays put");
    app.log.scroll_to_bottom();
    assert_eq!(app.log.scroll_back(), 0);
}

/// The entry-log pane lives alongside the event list, so opening it keeps
/// that list active and every list move updates the log it follows.
#[test]
fn the_entry_log_pane_follows_list_selection() {
    let mut app = app();
    app.push_event(AgentEvent::UserMessage {
        text: "first".into(),
    });
    app.push_event(AgentEvent::AgentMessage {
        text: "second".into(),
    });
    app.select_first();
    app.toggle_entry_log();
    assert_eq!(app.view, View::Events);
    assert!(app.entry_log.open);

    app.entry_log.scroll.note_limit(10);
    app.entry_log.scroll.line_down();
    app.select_next_line();
    assert_eq!(app.timeline.selected, 1);
    assert_eq!(app.entry_log.scroll.clamped(), 0, "a new selected entry");

    app.toggle_entry_log();
    assert!(!app.entry_log.open);
}

#[test]
fn transcript_view_toggles_independently_and_scrolls_from_the_top() {
    let mut app = app();
    app.toggle_raw();
    assert_eq!(app.view, View::Raw);
    // Toggling the transcript from the raw view switches to it, not back
    // to events.
    app.toggle_view(View::Transcript);
    assert_eq!(app.view, View::Transcript);
    app.toggle_view(View::Transcript);
    assert_eq!(app.view, View::Events);

    app.toggle_view(View::Transcript);
    assert_eq!(
        app.transcript.clamped(),
        0,
        "starts at the beginning, not the tail"
    );

    // The transcript scrolls against what was rendered, so stand in for a
    // render that found ten lines below the fold.
    app.transcript.note_limit(10);
    app.transcript.line_down();
    app.transcript.line_down();
    assert_eq!(app.transcript.clamped(), 2);
    app.transcript.line_up();
    assert_eq!(app.transcript.clamped(), 1);

    app.transcript.scroll_to_end();
    assert_eq!(app.transcript.clamped(), 10, "the last page, not past it");
    app.transcript.reset();
    assert_eq!(app.transcript.clamped(), 0);
}

#[test]
fn copying_the_conversation_leaves_out_everything_that_is_not_it() {
    let mut app = app();
    assert_eq!(app.conversation_text(), None, "nothing said yet");

    app.push_event(AgentEvent::UserMessage {
        text: "make it faster\nthe inner loop especially".into(),
    });
    app.push_event(AgentEvent::CommandStarted {
        command: "cargo bench".into(),
    });
    app.push_event(AgentEvent::Thinking {
        text: "considering the loop".into(),
        tokens: Some(120),
    });
    app.push_event(AgentEvent::AgentMessage {
        text: "hoisted the bounds check\n".into(),
    });

    assert_eq!(
        app.conversation_text().unwrap(),
        concat!(
            "    user make it faster\n",
            "         the inner loop especially\n",
            "   agent hoisted the bounds check\n",
        ),
        "both messages in full, laid out as the transcript lays them out, \
             and neither the command nor the thinking"
    );
    assert!(
        app.transcript_text().contains("cargo bench"),
        "the transcript itself still shows what the conversation copy drops"
    );
}

#[test]
fn a_copied_conversation_says_which_model_answered_and_what_failed() {
    let mut app = app();
    app.push_event(AgentEvent::ModelChanged {
        model: Some("claude-opus-5".into()),
        effort: None,
    });
    app.push_event(AgentEvent::Error {
        message: "the agent stopped".into(),
    });

    assert_eq!(
        app.conversation_text().unwrap(),
        concat!(
            "   model model → claude-opus-5 (same effort)\n",
            "   error the agent stopped\n",
        )
    );
}

#[test]
fn all_events_filters_the_event_list_without_changing_views() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage {
        text: "hello".into(),
    });
    app.push_event(AgentEvent::CommandStarted {
        command: "cargo test".into(),
    });

    // What the filter does, at each of its two states named outright. The
    // toggle's job is to move between them; which one a fresh session
    // starts in is a product decision, not this test's business.
    app.timeline.all_events = false;
    assert!(app.timeline.is_visible(0));
    assert!(!app.timeline.is_visible(1), "the command should be hidden");

    app.toggle_all_events();
    assert!(app.timeline.all_events);
    assert!(app.timeline.is_visible(1));

    app.toggle_all_events();
    assert!(!app.timeline.all_events);
    assert!(!app.timeline.is_visible(1));

    // Filtering the list is not a view change.
    assert_eq!(app.view, View::Events);
}

#[test]
fn conversation_only_still_shows_errors_and_model_changes() {
    let mut app = app();
    app.timeline.all_events = false;
    app.push_event(AgentEvent::Error {
        message: "workspace is out of credits".into(),
    });
    app.push_event(AgentEvent::ModelChanged {
        model: Some("claude-opus-5".into()),
        effort: None,
    });

    assert!(app.timeline.is_visible(0));
    assert!(app.timeline.is_visible(1));
    assert_eq!(
        app.timeline.entries[1].event().summary(),
        "model → claude-opus-5 (same effort)"
    );
}

#[test]
fn minor_events_are_hidden_and_skipped_by_navigation() {
    let mut app = app();
    app.push_event(AgentEvent::ThreadStarted {
        thread_id: "t".into(),
        model: None,
        effort: None,
    });
    // Multi-line so each entry has detail beyond its summary, and so
    // qualifies for the has-detail navigation this test also exercises.
    app.push_event(AgentEvent::AgentMessage {
        text: "a\nmore a".into(),
    });
    app.push_event(AgentEvent::TurnStarted);
    app.push_event(AgentEvent::AgentMessage {
        text: "b\nmore b".into(),
    });
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });

    // Hidden by default; no toggle needed to get here.
    assert!(!app.timeline.show_minor);

    app.select_first();
    assert_eq!(
        *app.timeline.entries[app.timeline.selected].event(),
        AgentEvent::AgentMessage {
            text: "a\nmore a".into()
        }
    );

    app.select_next_line();
    assert_eq!(
        *app.timeline.entries[app.timeline.selected].event(),
        AgentEvent::AgentMessage {
            text: "b\nmore b".into()
        }
    );

    // No more visible entries after "b"; select_next_line is a no-op.
    app.select_next_line();
    assert_eq!(
        *app.timeline.entries[app.timeline.selected].event(),
        AgentEvent::AgentMessage {
            text: "b\nmore b".into()
        }
    );

    app.select_prev_line();
    assert_eq!(
        *app.timeline.entries[app.timeline.selected].event(),
        AgentEvent::AgentMessage {
            text: "a\nmore a".into()
        }
    );

    app.toggle_minor();
    assert!(app.timeline.show_minor);
}

#[test]
fn preview_scroll_resets_when_selection_changes() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage {
        text: "first\nbody".into(),
    });
    app.push_event(AgentEvent::AgentMessage {
        text: "second\nbody".into(),
    });
    app.select_first();
    app.preview.scroll.note_limit(100);
    app.preview.scroll.page_down();
    assert_eq!(app.preview.scroll.offset, 10);

    app.select_next_line();
    assert_eq!(app.timeline.selected, 1);
    assert_eq!(app.preview.scroll.offset, 0);
}

#[test]
fn preview_page_down_does_not_accumulate_past_the_rendered_end() {
    let mut app = app();
    app.preview.scroll.note_limit(23);

    for _ in 0..100 {
        app.preview.scroll.page_down();
    }
    assert_eq!(app.preview.scroll.offset, 23);

    app.preview.scroll.page_up();
    assert_eq!(app.preview.scroll.offset, 13);
}

#[test]
fn toggling_minor_off_moves_selection_off_a_hidden_entry() {
    let mut app = app();
    app.toggle_minor(); // show minor events so follow can land on one
    assert!(app.timeline.show_minor);

    app.push_event(AgentEvent::AgentMessage { text: "a".into() });
    app.push_event(AgentEvent::TurnStarted);
    // Selection sits on the just-pushed minor entry via follow.
    assert_eq!(app.timeline.selected, 1);

    app.toggle_minor(); // hide them again
    assert!(!app.timeline.show_minor);
    assert!(app.timeline.is_visible(app.timeline.selected));
    assert_eq!(
        *app.timeline.entries[app.timeline.selected].event(),
        AgentEvent::AgentMessage { text: "a".into() }
    );
}

#[test]
fn file_view_collects_focused_and_session_paths() {
    let root = std::env::temp_dir().join(format!("styra-file-view-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
    std::fs::write(root.join("README.md"), "read me\n").unwrap();
    let mut app = app();
    app.workspace.enter(root.clone());
    app.push_event(AgentEvent::AgentMessage {
        text: "see README.md".into(),
    });
    app.timeline.follow = false;
    app.push_event(AgentEvent::FileChanged {
        id: "1".into(),
        paths: vec!["src/lib.rs".into()],
        diff: None,
        checkpoint: None,
        checkpoint_error: None,
    });

    assert_eq!(app.file_paths(), vec!["README.md"]);
    app.toggle_file_scope();
    assert_eq!(app.file_paths(), vec!["README.md", "src/lib.rs"]);
    let _ = std::fs::remove_dir_all(root);
}

/// Opening the view asks for the answer; it cannot render one it never
/// fetched, and the operator should not have to press a second key.
#[test]
fn answer_links_open_the_selected_messages_contract_and_section() {
    for contract in styra_protocol::contract::CONTRACTS {
        let mut app = app();
        app.push_event(AgentEvent::UserMessage {
            text: styra_protocol::contract::frame("show me", contract),
        });
        let source = "Before\n<styra:answer>\nfirst\n</styra:answer>\nBetween\n<styra:answer>\nsecond\n</styra:answer>\nAfter";
        app.push_event(AgentEvent::AgentMessage {
            text: source.into(),
        });
        app.push_event(AgentEvent::UserMessage {
            text: "another turn".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "newer reply".into(),
        });
        app.timeline.selected = 1;
        app.highlight_first_link();
        assert_eq!(app.entry_link_count(1), 2);
        app.highlight_next_link();
        assert!(app.highlighted_link_target().is_none());
        app.open_highlighted_link();
        assert_eq!(app.view, View::Answer);
        assert_eq!(app.answer.answer().unwrap().contract, contract);
        assert_eq!(
            app.answer.answer().unwrap().source,
            "<styra:answer>\nsecond\n</styra:answer>"
        );
        assert!(app.link_highlight.is_none());
        assert_eq!(app.take_request(), None);
    }
}

#[test]
fn opening_the_answer_view_requests_the_answer() {
    let mut app = app();
    app.toggle_answer();
    assert_eq!(app.view, View::Answer);
    assert_eq!(app.take_request(), Some(Request::Answer { contract: None }));

    app.toggle_answer();
    assert_eq!(app.view, View::Events);
    // Leaving asks for nothing: there is no answer to fetch for a view
    // that is no longer showing one.
    assert_eq!(app.take_request(), None);
}

#[test]
fn re_reading_names_the_contract_to_read_under() {
    let mut app = app();
    app.reread_answer(Contract::Lines);
    assert_eq!(
        app.take_request(),
        Some(Request::Answer {
            contract: Some(Contract::Lines)
        })
    );
}

/// `e` in the answer view opens what the agent named, so the location has
/// to resolve against the Workspace exactly as the Files view's does.
#[test]
fn a_files_answer_resolves_its_selection_against_the_workspace() {
    let mut app = app();
    app.workspace.enter(PathBuf::from("/work"));
    app.view = View::Answer;
    app.answer.set(Ok(Answer {
        contract: Contract::Files,
        value: Some(AnswerValue::Files(vec![FileLocation {
            path: PathBuf::from("src/auth.rs"),
            line: Some(12),
            column: None,
            end_line: None,
            description: String::new(),
        }])),
        error: None,
        source: "…".into(),
    }));
    assert_eq!(
        app.selected_file_path(),
        Some(PathBuf::from("/work/src/auth.rs"))
    );
    assert_eq!(app.copy_text().as_deref(), Some("src/auth.rs:12"));
}

#[test]
fn view_toggles_return_to_events_and_leave_the_side_preview_alone() {
    for target in [View::Log, View::Transcript, View::Driva, View::Preview] {
        let mut app = app();
        assert_eq!(app.launch.driva, None);
        assert_eq!(app.view, View::Events);
        for expected in [target, View::Events] {
            app.toggle_view(target);
            assert_eq!(app.view, expected, "{target:?}");
            assert!(
                !app.preview.open,
                "{target:?}: the side panel has its own toggle"
            );
        }
        app.toggle_raw();
        assert_eq!(app.view, View::Raw);
        app.toggle_view(target);
        assert_eq!(app.view, target, "{target:?}: switch from another view");
        assert!(!app.preview.open, "{target:?}");
    }
}

#[test]
fn files_view_opens_with_a_togglable_entry_preview() {
    let mut app = app();
    assert!(!app.preview.open);

    app.toggle_files();
    assert_eq!(app.view, View::Files);
    assert!(app.preview.open);

    app.preview.toggle();
    assert!(!app.preview.open);
    app.toggle_files();
    assert_eq!(app.view, View::Events);
}

#[test]
fn focus_toggles_and_a_typed_message_is_taken_for_sending() {
    let mut app = app();
    assert_eq!(app.focus, Focus::List);
    app.enter_input();
    assert_eq!(app.focus, Focus::Input);

    app.composer.char('h');
    app.composer.char('i');
    assert_eq!(app.composer.text(), "hi");
    assert_eq!(app.take_message(), Some("hi".into()));
    assert!(app.composer.text().is_empty());
    assert_eq!(app.take_message(), None);
}

#[test]
fn copy_text_is_the_selected_entrys_full_presented_detail() {
    let mut app = app();
    app.push_event(AgentEvent::CommandCompleted {
        command: "cargo test".into(),
        status: "completed".into(),
        exit_code: Some(0),
        output: "24 passed".into(),
    });
    let text = app.copy_text().expect("an entry is selected");
    assert!(text.contains("cargo test"));
    assert!(text.contains("24 passed"));
}

#[test]
fn copy_text_falls_back_to_the_summary_when_there_is_no_detail() {
    let mut app = app();
    app.timeline.show_minor = true;
    app.push_event(AgentEvent::TurnStarted);
    app.select_first();
    let text = app.copy_text().expect("a minor entry is selected");
    assert_eq!(text, AgentEvent::TurnStarted.summary());
}

#[test]
fn copy_text_is_none_for_views_with_no_discrete_entry() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage { text: "hi".into() });
    for view in [View::Log, View::Transcript, View::Driva] {
        app.view = view;
        assert!(app.copy_text().is_none(), "{view:?}");
    }
}

#[test]
fn copy_text_in_the_raw_view_is_the_selected_wire_line() {
    use styra_protocol::Direction;
    let mut app = app();
    app.raw.push(RawLine {
        at_ms: 0,
        direction: Direction::FromAgent,
        text: r#"{"type":"turn.started"}"#.into(),
    });
    app.view = View::Raw;
    assert_eq!(
        app.copy_text().as_deref(),
        Some(r#"{"type":"turn.started"}"#)
    );
}

/// An empty file in the temporary directory, by its absolute path, for a
/// link to name: a file link only opens if there is a file there.
fn scratch_file(name: &str) -> String {
    let path = std::env::temp_dir().join(format!("styra-app-{name}"));
    std::fs::write(&path, "").unwrap();
    path.display().to_string()
}

#[test]
fn a_file_link_is_looked_for_from_the_working_directory_up_to_the_workspace() {
    let root = std::env::temp_dir().join("styra-app-link-search");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("crate/src")).unwrap();
    std::fs::write(root.join("crate/Cargo.toml"), "").unwrap();
    let mut app = app();
    app.workspace.enter(root.clone());
    app.workspace.change_directory(root.join("crate/src"));
    app.push_event(AgentEvent::AgentMessage {
        text: "[manifest](Cargo.toml)".into(),
    });
    app.timeline.selected = 0;
    app.highlight_first_link();

    app.open_highlighted_link();

    assert_eq!(
        app.take_request(),
        Some(Request::OpenPath(root.join("crate/Cargo.toml")))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_file_link_is_also_looked_for_from_the_top_of_the_git_checkout() {
    let repository = std::env::temp_dir().join("styra-app-link-repository");
    let _ = std::fs::remove_dir_all(&repository);
    std::fs::create_dir_all(repository.join(".git")).unwrap();
    std::fs::create_dir_all(repository.join("styra/tui")).unwrap();
    std::fs::write(repository.join("README.md"), "").unwrap();
    let mut app = app();
    app.workspace.enter(repository.join("styra/tui"));
    app.push_event(AgentEvent::AgentMessage {
        text: "[readme](README.md)".into(),
    });
    app.timeline.selected = 0;
    app.highlight_first_link();

    // Without an associated repository the search stops at the Workspace.
    assert!(app.highlighted_link_target().unwrap().1.is_err());

    app.workspace.git_repository = Some(repository.clone());
    assert_eq!(
        app.highlighted_link_target().unwrap().1,
        Ok(repository.join("README.md"))
    );
    let _ = std::fs::remove_dir_all(&repository);
}

#[test]
fn a_file_link_that_names_no_file_is_reported_not_opened() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage {
        text: "[gone](/no/such/styra-file.md)".into(),
    });
    app.timeline.selected = 0;
    app.highlight_first_link();

    app.open_highlighted_link();

    assert!(app.take_request().is_none());
    assert!(app.link_highlight.is_some(), "the link stays highlighted");
    assert!(app
        .notices
        .iter()
        .any(|message| message.text.contains("no such file")));
}

#[test]
fn link_highlight_restores_preview_on_view_or_interaction_switch() {
    for open in [false, true] {
        let mut screen = app();
        screen.preview.open = open;
        screen.push_event(AgentEvent::AgentMessage {
            text: "No references here.".into(),
        });
        screen.highlight_first_link();
        assert!(screen.link_highlight.is_none());
        assert_eq!(screen.preview.open, open);

        screen.push_event(AgentEvent::AgentMessage {
            text: "[link](https://example.com)".into(),
        });
        screen.highlight_first_link();
        assert!(screen.preview.open);
        screen.toggle_view(View::Messages);
        assert!(screen.link_highlight.is_none());
        assert_eq!(screen.preview.open, open);

        screen.highlight_first_link();
        assert!(screen.preview.open);
        let state = screen.take_operator_state();
        let mut next = app();
        next.adopt(state);
        assert!(next.link_highlight.is_none());
        assert_eq!(next.preview.open, open);
    }
}

/// Stepping through the live list with the full-screen preview or the diff
/// open keeps it open, now on the Interaction stepped to.
#[test]
fn the_open_preview_or_diff_stays_open_on_the_next_interaction() {
    let target = styra_diff::DiffTarget {
        worktree: std::path::PathBuf::from("/state/worktrees/session-1"),
        branch: None,
        base_branch: None,
        base: "4bf5c35d".into(),
        branch_point: true,
    };
    let next_screen = |checkout: bool| {
        let mut next = app();
        next.session_id = "session-2".into();
        let mut interaction = crate::interactions::tests::interaction(
            "session-2",
            styra_protocol::InteractionActivity::Pending,
        );
        interaction.checkout = checkout.then(|| styra_protocol::CheckoutState {
            worktree: std::path::PathBuf::from("/state/worktrees/session-2"),
            repository: std::path::PathBuf::from("/home/me/project"),
            branch: None,
        });
        next.interactions.items = vec![interaction];
        next
    };
    let carry = |screen: &mut App, checkout: bool| {
        let mut next = next_screen(checkout);
        let mut state = screen.take_operator_state();
        state.interactions = std::mem::take(&mut next.interactions);
        next.adopt(state);
        next
    };

    let mut screen = app();
    screen.toggle_view(View::Preview);
    let mut next = carry(&mut screen, true);
    assert_eq!(next.view, View::Preview);
    assert_eq!(next.take_request(), None);

    let mut screen = app();
    screen.checkout_diff = Some(styra_diff::CheckoutDiff::read(target.clone()));
    screen.diff_preview = true;
    let mut next = carry(&mut screen, true);
    assert_eq!(
        next.take_request(),
        Some(Request::ShowDiff("session-2".into()))
    );

    // One without a checkout has no diff to show, so none is asked for.
    let mut screen = app();
    screen.checkout_diff = Some(styra_diff::CheckoutDiff::read(target));
    screen.diff_preview = true;
    let mut next = carry(&mut screen, false);
    assert_eq!(next.take_request(), None);
    assert!(next.diff_preview);
    assert!(next.checkout_diff.is_none());
    // The pane preference survives a row which cannot load any diff.
    let mut after = carry(&mut next, true);
    assert_eq!(
        after.take_request(),
        Some(Request::ShowDiff("session-2".into()))
    );
}

#[test]
fn link_highlight_starts_at_the_selected_entry_and_jk_walks_links() {
    let mut app = app();
    let first = scratch_file("walk-first.md");
    let second = scratch_file("walk-second.md");
    app.push_event(AgentEvent::AgentMessage {
        text: "[old](https://old.example)".into(),
    });
    app.push_event(AgentEvent::AgentMessage {
        text: format!("[first]({first}) then [second]({second})"),
    });
    app.timeline.selected = 1;

    app.highlight_first_link();
    assert_eq!(
        app.link_highlight,
        Some(LinkHighlight { entry: 1, link: 0 })
    );
    assert_eq!(app.timeline.selected, 1);
    assert!(app.timeline.entries[1].expanded);

    app.highlight_next_link();
    assert_eq!(
        app.link_highlight,
        Some(LinkHighlight { entry: 1, link: 1 })
    );
    app.highlight_next_link();
    assert_eq!(
        app.link_highlight,
        Some(LinkHighlight { entry: 1, link: 1 })
    );
    app.highlight_prev_link();
    assert_eq!(
        app.link_highlight,
        Some(LinkHighlight { entry: 1, link: 0 })
    );

    app.open_highlighted_link();
    assert_eq!(
        app.take_request(),
        Some(Request::OpenPath(PathBuf::from(&first)))
    );
    assert!(app.link_highlight.is_none());

    app.highlight_first_link();
    app.clear_link_highlight();
    assert!(app.link_highlight.is_none());
}

#[test]
fn the_link_menu_names_the_highlighted_link() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage {
        text: "[first](/tmp/first.md)".into(),
    });
    app.timeline.selected = 0;
    app.highlight_first_link();

    app.open_link_menu();
    let menu = app.link_menu.as_ref().unwrap();
    assert_eq!(menu.destination(), "/tmp/first.md");
    assert_eq!(menu.labels()[0], "open in editor");
    assert!(app.take_request().is_none());
}

#[test]
fn link_highlighting_wraps_when_started_below_the_last_reference() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage {
        text: "see [first](src/lib.rs:4) then [last](src/main.rs:12)".into(),
    });
    app.push_event(AgentEvent::CommandStarted {
        command: "cargo test".into(),
    });
    app.timeline.selected = 1;

    app.highlight_first_link();

    assert_eq!(
        app.link_highlight,
        Some(LinkHighlight { entry: 0, link: 1 })
    );
}

#[test]
fn a_web_link_opens_as_an_address_not_a_file() {
    let mut app = app();
    let local = scratch_file("web-local.md");
    app.push_event(AgentEvent::AgentMessage {
        text: format!("[docs](https://example.com/docs) and [local]({local})"),
    });
    app.timeline.selected = 0;

    app.highlight_first_link();
    assert!(app.highlighted_link_target().is_none());
    app.open_highlighted_link();
    assert_eq!(
        app.take_request(),
        Some(Request::OpenUrl("https://example.com/docs".into()))
    );

    app.highlight_first_link();
    app.highlight_next_link();
    app.open_highlighted_link();
    assert_eq!(
        app.take_request(),
        Some(Request::OpenPath(PathBuf::from(&local)))
    );
}

#[test]
fn a_backticked_file_reference_is_selectable_like_a_link() {
    let mut app = app();
    app.push_event(AgentEvent::AgentMessage {
        text: "- **Field:** `/tmp/spec.rs:44` declares `default_model: &str`".into(),
    });
    app.timeline.selected = 0;

    app.highlight_first_link();
    assert_eq!(
        app.link_highlight,
        Some(LinkHighlight { entry: 0, link: 0 })
    );
    let (written, path, line) = app.highlighted_link_target().unwrap();
    assert_eq!(written, "/tmp/spec.rs:44");
    assert!(
        path.is_err(),
        "a citation of a file that is not there finds none"
    );
    assert_eq!(line, Some(44));

    // The code that is not a filename is not a second entry.
    app.highlight_next_link();
    assert_eq!(
        app.link_highlight,
        Some(LinkHighlight { entry: 0, link: 0 })
    );
}

#[test]
fn link_locations_separate_a_line_and_optional_column_from_the_path() {
    assert_eq!(split_link_location("src/main.rs"), ("src/main.rs", None));
    assert_eq!(
        split_link_location("src/main.rs:12"),
        ("src/main.rs", Some(12))
    );
    assert_eq!(
        split_link_location("src/main.rs:12:4"),
        ("src/main.rs", Some(12))
    );
    assert_eq!(
        split_link_location("C:\\work\\main.rs:9"),
        ("C:\\work\\main.rs", Some(9))
    );
}

/// The replacement paths in `ingest` are why an entry needs a revision at
/// all: each of them rewrites a row that is already on the list rather
/// than appending a second one. A rewrite that did not advance the
/// version would leave anything holding work for that row showing the
/// command as still running.
#[test]
fn a_command_finishing_rewrites_its_row_as_a_new_version() {
    let mut app = app();
    app.push_event(AgentEvent::CommandStarted {
        command: "cargo test".into(),
    });
    let running = app.timeline.entries[0].version();

    app.push_event(AgentEvent::CommandCompleted {
        command: "cargo test".into(),
        status: "ok".into(),
        exit_code: Some(0),
        output: String::new(),
    });

    assert_eq!(app.timeline.entries.len(), 1, "one row, not two");
    let finished = app.timeline.entries[0].version();
    assert_eq!(finished.id, running.id);
    assert_ne!(finished, running);
}

/// A task's reports all refresh one row, so each of them is a new version
/// of it — except the one `ingest` deliberately ignores. A progress report
/// that arrives after the ending changes nothing, and must not claim to.
#[test]
fn a_task_report_that_changes_nothing_leaves_the_version_alone() {
    let mut app = app();
    app.push_event(AgentEvent::TaskStarted {
        id: "t1".into(),
        description: "search".into(),
        kind: "local_agent".into(),
        agent: None,
    });
    let started = app.timeline.entries[0].version();

    app.push_event(AgentEvent::TaskCompleted {
        id: "t1".into(),
        status: "completed".into(),
        summary: "found it".into(),
        error: None,
    });
    let ended = app.timeline.entries[0].version();
    assert_eq!(ended.id, started.id);
    assert_ne!(ended, started, "an ending is a new version of the row");

    // Late, and about a task that has already finished: dropped by
    // `refresh_task`, so the row is untouched.
    app.push_event(AgentEvent::TaskProgress {
        id: "t1".into(),
        description: "still searching".into(),
        agent: None,
        tool: None,
        tokens: None,
    });
    assert_eq!(app.timeline.entries.len(), 1);
    assert_eq!(app.timeline.entries[0].version(), ended);
}
