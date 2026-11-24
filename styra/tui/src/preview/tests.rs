use super::*;

/// The offset counts lines of whatever is currently rendered. Changing
/// what that is without resetting it would leave the panel scrolled to an
/// arbitrary point in content the operator has not seen.
#[test]
fn changing_what_is_shown_returns_to_the_top() {
    let mut preview = Preview::default();
    preview.scroll.note_limit(100);
    preview.scroll.page_down();
    assert!(preview.scroll.offset > 0);

    preview.toggle_target();
    assert_eq!(preview.scroll.offset, 0);
}

/// Opening and closing the panel does not change what it would show, so an
/// operator who closes it and opens it again gets what they had.
#[test]
fn opening_the_panel_leaves_the_target_alone() {
    let mut preview = Preview::default();
    preview.toggle_target();

    preview.toggle();
    preview.toggle();

    assert!(!preview.open);
    assert!(preview.follows_command());
}

#[test]
fn adopting_a_previous_screens_choices_starts_its_content_from_the_top() {
    let mut preview = Preview::default();
    preview.scroll.note_limit(100);
    preview.scroll.page_down();

    let mut chosen = Preview::default();
    chosen.show();
    chosen.toggle_target();
    preview.adopt(chosen.choices());

    assert!(preview.open);
    assert!(preview.follows_command());
    assert_eq!(preview.scroll.offset, 0, "a different screen's content");
}
