use super::*;

fn timeline(events: Vec<AgentEvent>) -> Timeline {
    let mut timeline = Timeline::default();
    for event in events {
        timeline.push(event, false, None, None);
    }
    timeline
}

fn message(text: &str) -> AgentEvent {
    AgentEvent::AgentMessage {
        text: text.to_owned(),
    }
}

/// The point of an id: it names a row, not a position. Everything a
/// renderer might key on it survives entries arriving around it.
#[test]
fn an_id_names_one_row_for_the_life_of_the_list() {
    let mut list = timeline(vec![message("first"), message("second")]);
    let ids: Vec<EventId> = list.entries.iter().map(Entry::id).collect();
    assert_ne!(ids[0], ids[1], "two rows must not share an id");

    list.push(message("third"), false, None, None);
    let after: Vec<EventId> = list.entries.iter().map(Entry::id).collect();
    assert_eq!(&after[..2], &ids[..], "appending must not renumber rows");
    assert!(
        !ids.contains(&after[2]),
        "a new row must not reuse an id already handed out"
    );
}

/// The revision is what keeps the id honest: a row that is rewritten in
/// place is the same row showing something else, and anything holding
/// rendered work for it has to be able to tell.
#[test]
fn rewriting_a_row_keeps_its_id_and_advances_its_revision() {
    let mut list = timeline(vec![message("thinking")]);
    let before = list.entries[0].version();

    list.entries[0].set_event(message("done"));
    let after = list.entries[0].version();

    assert_eq!(after.id, before.id, "a rewrite is the same row");
    assert_ne!(after, before, "a rewrite is a different version of it");

    list.entries[0].update_event(|event| {
        if let AgentEvent::AgentMessage { text } = event {
            text.push('!');
        }
    });
    assert_ne!(
        list.entries[0].version(),
        after,
        "an in-place edit is a rewrite too"
    );
}

/// Folding is the operator's view of a row, not a change to what it
/// shows — and the two are keyed separately, so a fold must not spend a
/// revision.
#[test]
fn folding_a_row_does_not_make_it_a_new_version() {
    let mut list = timeline(vec![message("first")]);
    let before = list.entries[0].version();
    list.toggle_expand();
    assert_eq!(list.entries[0].version(), before);
}

/// A branch of a branch carries its source's markers in the history it
/// copied; only the last one says where this Session's own work begins.
#[test]
fn the_branch_point_is_the_last_branched_from_marker() {
    let from = |session: &str| AgentEvent::Branched {
        direction: BranchDirection::From,
        session: session.into(),
        name: None,
    };
    let list = timeline(vec![
        from("b"),
        from("a"),
        message("a's work"),
        from("a"),
        message("b's work"),
        from("b"),
        message("this session's work"),
        AgentEvent::Branched {
            direction: BranchDirection::To,
            session: "d".into(),
            name: None,
        },
    ]);
    assert_eq!(list.branch_point, Some(5));
    assert_eq!(timeline(vec![message("fresh")]).branch_point, None);
}

/// A branch's log starts at its branch point: what it inherited, and the
/// `branched from` marker at the very top, are the source's to show.
#[test]
fn inherited_history_is_hidden_and_first_lands_on_the_branch_point() {
    let from = |session: &str| AgentEvent::Branched {
        direction: BranchDirection::From,
        session: session.into(),
        name: None,
    };
    let mut list = timeline(vec![
        from("parent"),
        from("grandparent"),
        message("inherited work"),
        from("parent"),
        message("own work"),
        AgentEvent::Branched {
            direction: BranchDirection::To,
            session: "child".into(),
            name: None,
        },
    ]);
    list.all_events = true;
    list.show_minor = true;
    assert_eq!(
        (0..list.entries.len())
            .filter(|&idx| list.is_visible(idx))
            .collect::<Vec<_>>(),
        [3, 4, 5]
    );
    list.select_last();
    assert!(list.select_first());
    assert_eq!(list.selected, 3);
    assert!(!list.follow);
    assert!(!list.select_backward());
    assert_eq!(list.selected, 3);

    let mut list = timeline(vec![message("first"), message("last")]);
    list.select_last();
    list.select_first();
    assert_eq!(list.selected, 0);
    assert!(!list.follow);
}

/// The caller resets the preview scroll on a move, so a key that could not
/// move must say so rather than reporting every press as a move.
#[test]
fn a_move_reports_whether_the_selection_actually_changed() {
    let mut list = timeline(vec![message("one"), message("two")]);
    list.selected = 0;
    assert!(list.select_forward());
    assert_eq!(list.selected, 1);
    // At the tail there is nowhere to go, and following resumes.
    assert!(!list.select_forward());
    assert!(list.follow);

    assert!(list.select_backward());
    assert_eq!(list.selected, 0);
    // Leaving the tail pins the view whether or not the move landed.
    assert!(!list.follow);
    assert!(!list.select_backward());
}

fn shell(command: &str) -> AgentEvent {
    AgentEvent::CommandStarted {
        command: command.to_owned(),
    }
}

fn user(text: &str) -> AgentEvent {
    AgentEvent::UserMessage {
        text: text.to_owned(),
    }
}

#[test]
fn conversation_spans_include_the_message_and_its_work() {
    for (name, events, selections) in [
        (
            "messages delimit spans",
            vec![
                user("ask"),
                shell("one"),
                shell("two"),
                message("answer"),
                shell("three"),
            ],
            vec![(0, 0..3), (3, 3..5)],
        ),
        (
            "tool belongs to preceding message",
            vec![user("ask"), shell("one"), shell("two")],
            vec![(2, 0..3)],
        ),
        (
            "work before first message",
            vec![shell("one"), shell("two"), message("hello")],
            vec![(1, 0..2)],
        ),
        ("empty only when the log is", vec![], vec![(0, 0..0)]),
    ] {
        let mut list = timeline(events);
        for (selected, expected) in selections {
            list.selected = selected;
            assert_eq!(list.conversation_span(), expected, "{name}: {selected}");
        }
    }
}

/// A filter that hides the selected row has to pull the selection onto one
/// that is still shown — and say that it did, since the preview it was
/// showing is no longer the selected entry's.
#[test]
fn a_filter_that_hides_the_selection_moves_it_to_a_visible_row() {
    let mut list = timeline(vec![
        message("kept"),
        AgentEvent::TurnStarted,
        AgentEvent::TurnStarted,
    ]);
    list.show_minor = true;
    list.selected = 2;

    assert!(list.toggle_minor());
    assert_eq!(list.selected, 0, "back to the nearest visible row");
    assert!(list.is_visible(list.selected));

    // With the selection already visible there is nothing to move.
    assert!(!list.toggle_minor());
    assert_eq!(list.selected, 0);
}
