use super::*;

/// Half a page is half of what the renderer last showed, and a page
/// until it has said how much that is.
#[test]
fn half_a_page_is_half_the_viewport() {
    let mut scroll = Scroll::default();
    scroll.note_limit(100);
    scroll.half_page_down();
    assert_eq!(scroll.offset, SCROLL_PAGE, "no viewport reported yet");

    scroll.note_viewport(9);
    scroll.half_page_down();
    assert_eq!(scroll.offset, SCROLL_PAGE + 4);
    scroll.half_page_up();
    assert_eq!(scroll.offset, SCROLL_PAGE);

    scroll.note_viewport(1);
    scroll.half_page_down();
    assert_eq!(scroll.offset, SCROLL_PAGE + 1, "always at least a line");
}

/// Revealing a row scrolls no further than it takes to bring it in.
#[test]
fn revealing_a_row_scrolls_just_enough() {
    let mut scroll = Scroll::default();
    scroll.note_limit(100);
    scroll.reveal(30);
    assert_eq!(scroll.offset, 30, "no viewport reported yet");
    scroll.reveal(0);

    scroll.note_viewport(10);
    scroll.reveal(9);
    assert_eq!(scroll.offset, 0);
    scroll.reveal(10);
    assert_eq!(scroll.offset, 1);
    scroll.reveal(30);
    assert_eq!(scroll.offset, 21);
    scroll.reveal(25);
    assert_eq!(scroll.offset, 21);
    scroll.reveal(4);
    assert_eq!(scroll.offset, 4);
}
