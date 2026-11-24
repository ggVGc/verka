use super::*;

#[test]
fn closing_the_pane_hands_the_keys_back_to_the_list() {
    let mut pane = EntryLog::default();
    pane.toggle(3);
    pane.focus();
    assert!(pane.focused());

    pane.toggle(3);
    assert!(!pane.open);
    assert!(!pane.focused(), "a closed pane holds no keys");

    // Opening it again starts as the follower it is described as.
    pane.toggle(3);
    assert!(pane.open);
    assert!(!pane.focused());
}

/// An operator opening the log wants the newest entry, which is the last
/// row: that is where the cursor starts, and the preview with it.
#[test]
fn opening_the_pane_puts_the_cursor_on_the_newest_entry() {
    let mut pane = EntryLog::default();
    pane.toggle(4);
    assert_eq!(pane.cursor(4), Some(3));

    // An empty stretch has no row to start on.
    pane.toggle(4);
    pane.toggle(0);
    assert_eq!(pane.cursor(0), None);
}

/// The pane is open under a stretch that shrinks — the operator moved to a
/// message with less work under it. The cursor is held, not lost, so it
/// still points inside what is on screen.
#[test]
fn the_cursor_is_held_to_the_stretch_it_is_read_against() {
    let mut pane = EntryLog::default();
    pane.toggle(1);
    pane.focus();
    pane.select_last(10);
    assert_eq!(pane.cursor(10), Some(9));
    assert_eq!(pane.cursor(3), Some(2));
    assert_eq!(pane.cursor(0), None, "an empty stretch points at nothing");
}

#[test]
fn walking_the_stretch_stops_at_both_ends() {
    let mut pane = EntryLog::default();
    pane.toggle(1);
    pane.focus();
    pane.select_prev(3);
    assert_eq!(pane.cursor(3), Some(0));

    pane.select_next(3);
    pane.select_next(3);
    pane.select_next(3);
    assert_eq!(pane.cursor(3), Some(2));

    pane.select_first();
    assert_eq!(pane.cursor(3), Some(0));
}

#[test]
fn paging_moves_the_cursor_and_stops_at_both_ends() {
    let mut pane = EntryLog::default();
    pane.toggle(1);
    pane.focus();
    pane.page_down(40);
    assert_eq!(pane.cursor(40), Some(PAGE));

    pane.page_down(12);
    assert_eq!(pane.cursor(12), Some(11), "held to the shorter stretch");

    pane.page_up(12);
    assert_eq!(pane.cursor(12), Some(1));
    pane.page_up(12);
    assert_eq!(pane.cursor(12), Some(0));
}

/// While the pane holds the keys the list is standing still, so the
/// follow-the-list reset must not pull its cursor back to the top under
/// the operator as events arrive.
#[test]
fn following_the_list_leaves_the_focused_panes_cursor_alone() {
    let mut pane = EntryLog::default();
    pane.toggle(1);
    pane.focus();
    pane.select_next(5);

    pane.follow_list(4);
    assert_eq!(pane.cursor(5), Some(1));

    pane.unfocus();
    pane.follow_list(4);
    assert_eq!(pane.cursor(5), Some(4), "an unfocused pane follows again");
}
