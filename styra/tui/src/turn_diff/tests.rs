use super::*;
use crate::presentation::test_support;

fn changed(path: &str, old: &str, new: &str) -> AgentEvent {
    AgentEvent::FileChanged {
        id: String::new(),
        paths: vec![path.into()],
        diff: Some(format!("@@ edit @@\n-{old}\n+{new}")),
        checkpoint: None,
        checkpoint_error: None,
    }
}

/// Two turns, each a request and the work done for it, with the side
/// diff panel showing the turn diff.
fn app_with_two_turns() -> App {
    let mut app = test_support::app("s1");
    app.timeline.all_events = false;
    app.push_event(AgentEvent::UserMessage {
        text: "fix the retry backoff".into(),
    });
    app.push_event(AgentEvent::CommandStarted {
        command: "cargo test backoff".into(),
    });
    app.push_event(changed("src/retry.rs", "delay * 3", "delay * 2"));
    app.push_event(AgentEvent::UserMessage {
        text: "and rename the helper".into(),
    });
    app.push_event(changed("src/helper.rs", "fn old_name", "fn new_name"));
    app.select_first();
    app.diff_preview = true;
    app.diff_turn = true;
    app.sync_turn_diff();
    app
}

#[test]
fn highlighting_follows_the_turn_while_keeping_the_full_diff_and_scroll() {
    use crate::keybindings::{press, DIFF_TURN_HIGHLIGHT};
    let mut app = app_with_two_turns();
    let full = "diff --git a/src/retry.rs b/src/retry.rs\n@@ -1 +1 @@\n-delay * 3\n+delay * 2\ndiff --git a/src/helper.rs b/src/helper.rs\n@@ -1 +1 @@\n-fn old_name\n+fn new_name\n";
    app.checkout_diff = Some(CheckoutDiff::supplied(
        full.into(),
        Supplied {
            label: "full diff".into(),
            root: None,
        },
    ));
    app.checkout_diff.as_mut().unwrap().scroll.offset = 3;
    crate::input::handle_diff_key(&mut app, press(&DIFF_TURN_HIGHLIGHT));
    assert!(app.diff_turn_highlight);
    assert!(!app.turn_diff_shown());
    assert_eq!(shown(&app), full);
    assert_eq!(app.shown_diff().unwrap().scroll.offset, 3);
    assert!(app
        .turn_diff
        .as_ref()
        .unwrap()
        .diff
        .diff
        .as_ref()
        .unwrap()
        .contains("+delay * 2"));
    app.select_next_line();
    assert!(app.sync_turn_diff());
    assert_eq!(app.turn_diff.as_ref().unwrap().turn, Some(2));
    assert!(app
        .turn_diff
        .as_ref()
        .unwrap()
        .diff
        .diff
        .as_ref()
        .unwrap()
        .contains("+fn new_name"));
    assert_eq!(shown(&app), full);
    crate::input::handle_diff_key(&mut app, press(&DIFF_TURN_HIGHLIGHT));
    assert!(!app.diff_turn_highlight);
    assert_eq!(shown(&app), full);
}

fn shown(app: &App) -> &str {
    app.shown_diff().unwrap().diff.as_deref().unwrap()
}

/// Reported changes are given the file headers the view lists files by,
/// and the turn's message goes above them.
#[test]
fn the_turn_diff_is_the_selected_turns_reported_changes() {
    let app = app_with_two_turns();
    assert_eq!(app.selected_turn(), Some(1));
    let files = styra_diff::files::file_diffs(shown(&app));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/retry.rs");
    assert!(shown(&app).contains("+delay * 2"));
    assert!(
        matches!(app.turn_message(), Some(AgentEvent::UserMessage { text }) if text == "fix the retry backoff")
    );
    let Source::Supplied(supplied) = &app.shown_diff().unwrap().source else {
        panic!()
    };
    assert_eq!(supplied.label, "turn 1 · as the agent reported its changes");
    assert!(!shown(&app).contains("new_name"));
}

/// Moving to the next turn starts its diff from the top, with the view's
/// toggles kept.
#[test]
fn the_turn_diff_follows_the_selection() {
    let mut app = app_with_two_turns();
    let panel = app.shown_diff_mut().unwrap();
    panel.per_file = true;
    panel.scroll.offset = 2;
    app.select_next_line();
    assert!(app.sync_turn_diff());
    assert_eq!(app.selected_turn(), Some(2));
    assert!(shown(&app).contains("+fn new_name"));
    let panel = app.shown_diff().unwrap();
    assert!(panel.per_file);
    assert_eq!(panel.scroll.offset, 0);
    assert!(!app.sync_turn_diff(), "nothing changed since");
}

/// Once its snapshots are read, a turn is shown as they say, and called
/// so; a turn still running says it is only so far.
#[test]
fn a_snapshotted_turn_shows_its_snapshot_diff() {
    let mut app = app_with_two_turns();
    app.select_last();
    for (ended, label) in [
        (false, "turn 2 · from its snapshots, so far"),
        (true, "turn 2 · from its snapshots"),
    ] {
        app.turn_snapshot = Some(crate::turn_snapshot::TurnSnapshot::loaded_as(
                "s1",
                2,
                Some(
                    "diff --git a/src/helper.rs b/src/helper.rs\n@@ -1 +1 @@\n-fn old_name\n+fn snapshot_name\n",
                ),
                ended,
            ));
        assert!(app.sync_turn_diff());
        assert!(shown(&app).contains("snapshot_name"));
        assert!(!shown(&app).contains("new_name\n"));
        let Source::Supplied(supplied) = &app.shown_diff().unwrap().source else {
            panic!()
        };
        assert_eq!(supplied.label, label);
    }
}

/// Snapshots that agree say the turn changed nothing, whatever the agent
/// reported.
#[test]
fn a_snapshotted_turn_that_changed_nothing_says_so() {
    let mut app = app_with_two_turns();
    app.turn_snapshot = Some(crate::turn_snapshot::TurnSnapshot::loaded(
        "s1",
        1,
        Some(""),
    ));
    app.sync_turn_diff();
    assert_eq!(shown(&app), "");
    test_support::inspect(&app, |v| {
        assert!(matches!(v.main, styra_ui::application::MainView::Events(_)))
    });
}

/// Codex's whole-turn diff already names its files, and the newest one
/// stands for the items before it.
#[test]
fn a_reported_turn_diff_is_kept_as_it_is() {
    let mut app = test_support::app("s1");
    app.push_event(AgentEvent::UserMessage {
        text: "fix it".into(),
    });
    app.push_event(changed("src/a.rs", "item-old", "item-new"));
    app.push_event(AgentEvent::DiffUpdated {
        diff: "diff --git a/src/a.rs b/src/a.rs\n@@ -1 +1 @@\n-first-old\n+first-new\n".into(),
    });
    let latest = "diff --git a/src/a.rs b/src/a.rs\n@@ -1 +1 @@\n-latest-old\n+latest-new\n";
    app.push_event(AgentEvent::DiffUpdated {
        diff: latest.into(),
    });
    app.select_first();
    app.diff_preview = true;
    app.diff_turn = true;
    app.sync_turn_diff();
    assert_eq!(shown(&app), latest);
}

/// A path in the Workspace is named from the Workspace on; one elsewhere
/// as reported. A change reported without a diff is still listed.
#[test]
fn reported_paths_are_named_from_the_workspace_on() {
    let workspace = SandboxLayout::default().workspace;
    let app = test_support::app("s1");
    let inside = changed(
        &workspace.join("src/retry.rs").display().to_string(),
        "a",
        "b",
    );
    let bare = AgentEvent::FileChanged {
        id: String::new(),
        paths: vec!["/etc/hosts".into()],
        diff: None,
        checkpoint: None,
        checkpoint_error: None,
    };
    let text = patch(&app, &[&inside, &bare]);
    let files = styra_diff::files::file_diffs(&text);
    let paths: Vec<_> = files.iter().map(|file| file.path).collect();
    assert_eq!(paths, ["src/retry.rs", "/etc/hosts"]);
}

/// Before the first message there is no turn to show.
#[test]
fn before_any_message_there_is_no_turn() {
    let mut app = test_support::app("s1");
    app.push_event(AgentEvent::AgentMessage {
        text: "hello".into(),
    });
    app.select_first();
    app.diff_preview = true;
    app.diff_turn = true;
    app.sync_turn_diff();
    assert!(app.shown_diff().unwrap().diff.is_err());
    assert!(app.turn_message().is_none());
}
