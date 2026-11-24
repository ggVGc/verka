use super::*;

fn scrolled(limit: u16) -> Help {
    let mut help = Help::default();
    help.open();
    help.note_limit(limit);
    help.page_down();
    help
}

/// A section the operator scrolled to once is not where they want to be
/// the next time they reach for the reference.
#[test]
fn closing_the_reference_returns_it_to_the_top() {
    let mut help = scrolled(100);
    assert!(help.offset() > 0);

    help.close();
    help.open();

    assert!(help.is_open());
    assert_eq!(help.offset(), 0);
}

/// The offset is only meaningful against a rendered height, so repeated
/// presses at the bottom must not bank an offset that scrolling up would
/// then have to unwind.
#[test]
fn scrolling_past_the_end_does_not_accumulate() {
    let mut help = Help::default();
    help.open();
    help.note_limit(12);
    for _ in 0..10 {
        help.page_down();
    }
    assert_eq!(help.offset(), 12, "held at the last page");

    help.page_up();

    assert_eq!(
        help.offset(),
        2,
        "one page back from the end, not from far past it"
    );
}

#[test]
fn the_reference_opens_closed_and_at_the_top() {
    let help = Help::default();

    assert!(!help.is_open());
    assert_eq!(help.offset(), 0);
}

#[test]
fn g_returns_to_the_top_without_closing() {
    let mut help = scrolled(100);

    help.scroll_to_top();

    assert!(help.is_open());
    assert_eq!(help.offset(), 0);
}
