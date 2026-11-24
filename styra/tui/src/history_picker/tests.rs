use super::*;
use crate::keybindings::press;
use crossterm::event::{KeyCode, KeyEvent};

fn history() -> InteractionHistory {
    let mut history = InteractionHistory::default();
    for id in ["session-a", "session-b", "session-c"] {
        history.record(id);
    }
    history
}

fn run_keys(rows: &Rows, keys: impl IntoIterator<Item = KeyEvent>) -> Option<usize> {
    let mut terminal = crate::test_ui::TestUi::default();
    terminal.script(keys.into_iter().map(Event::Key));
    run(&mut terminal, rows).unwrap()
}

/// The keypresses that type `query` and then open the row it found.
fn search_and_open(query: &str) -> Vec<KeyEvent> {
    query
        .chars()
        .map(|character| KeyEvent::from(KeyCode::Char(character)))
        .chain([press(&keys::ACTION_MENU_RUN)])
        .collect()
}

#[test]
fn visits_are_newest_first_and_names_are_fuzzy_searchable() {
    let history = history();
    let mut interactions = LiveInteractions::default();
    interactions.note_name("session-b", Some("Fix retry timeout".into()));
    let rows = rows(&history, &interactions, &[], &[]);
    assert_eq!(rows.indices, [2, 1, 0]);
    assert_eq!(run_keys(&rows, [press(&keys::ACTION_MENU_RUN)]), Some(2));
    assert_eq!(run_keys(&rows, search_and_open("frt")), Some(1));
    assert_eq!(history.target("session-c", false), Some((1, "session-b")));
}

#[test]
fn stored_sessions_remain_searchable_by_workspace_name_and_id() {
    let history = history();
    let workspaces = [WorkspaceSummary {
        path: "/workspaces/compiler".into(),
        session_count: 1,
        age: String::new(),
        created_at_ms: 0,
        last_accessed_at_ms: 0,
        ..crate::workspace::tests::workspace("workspace-id", Some("Compiler tools"), "/compiler")
    }];
    let sessions = [SessionSummary {
        id: "session-a".into(),
        name: Some("Repair parser".into()),
        first_prompt: None,
        tags: Vec::new(),
        workspace_id: "workspace-id".into(),
        path: "/workspaces/compiler/session-a".into(),
        selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
        age: String::new(),
        created_at_ms: None,
        last_event_at_ms: None,
        last_event_age: String::new(),
        origin: None,
        branched_from: None,
        completed: Default::default(),
        integration: None,
    }];
    let rows = rows(
        &history,
        &LiveInteractions::default(),
        &workspaces,
        &sessions,
    );
    for query in ["cptl", "rpp", "session-a"] {
        assert_eq!(run_keys(&rows, search_and_open(query)), Some(0), "{query}");
    }
    // Entries without server metadata are still selectable by stable id.
    assert_eq!(run_keys(&rows, search_and_open("session-b")), Some(1));
}

#[test]
fn cancel_and_empty_matches_do_not_choose_or_change_history() {
    let history = history();
    let rows = rows(&history, &LiveInteractions::default(), &[], &[]);
    assert_eq!(
        run_keys(
            &rows,
            [
                press(&keys::ACTION_MENU_NEXT),
                press(&keys::ACTION_MENU_CANCEL)
            ]
        ),
        None
    );
    let mut no_match = search_and_open("z");
    no_match.push(press(&keys::ACTION_MENU_CANCEL));
    assert_eq!(run_keys(&rows, no_match), None);
    assert_eq!(history.target("session-c", false), Some((1, "session-b")));
}

#[test]
fn choosing_a_visit_keeps_the_forward_trail() {
    let mut history = history();
    let rows = rows(&history, &LiveInteractions::default(), &[], &[]);
    let index = run_keys(
        &rows,
        [
            press(&keys::ACTION_MENU_NEXT),
            press(&keys::ACTION_MENU_RUN),
        ],
    )
    .unwrap();
    history.arrive(index);
    assert_eq!(history.target("session-b", true), Some((2, "session-c")));
    assert_eq!(history.entries().len(), 3);
}
