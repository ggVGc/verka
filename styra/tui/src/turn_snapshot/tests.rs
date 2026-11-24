use super::*;

#[test]
fn full_diff_highlights_request_the_selected_turns_snapshots() {
    let mut app = crate::presentation::test_support::app("s1");
    app.push_event(styra_protocol::event::AgentEvent::UserMessage {
        text: "edit a file".into(),
    });
    app.select_first();
    app.diff_preview = true;
    assert_eq!(app.snapshot_turn(), None);
    app.diff_turn_highlight = true;
    assert_eq!(app.snapshot_turn(), Some(1));
    app.diff_preview = false;
    assert_eq!(app.snapshot_turn(), None);
}
