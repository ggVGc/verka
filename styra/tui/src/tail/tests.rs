use super::*;

fn tail(entries: usize) -> Tail<usize> {
    let mut tail = Tail::default();
    for entry in 0..entries {
        tail.push(entry);
    }
    tail
}

#[test]
fn appended_entries_preserve_the_view_or_follow_the_tail() {
    for (name, count, scrolls, appended, expected) in [
        ("scrolled back", 5, 2, vec![5, 6], 4),
        ("following", 3, 0, vec![3], 0),
    ] {
        let mut tail = tail(count);
        for _ in 0..scrolls {
            tail.scroll_up();
        }
        assert_eq!(tail.scroll_back(), scrolls, "{name}: before append");
        for entry in appended {
            tail.push(entry);
        }
        assert_eq!(tail.scroll_back(), expected, "{name}: after append");
    }
}

#[test]
fn scrolling_stops_at_the_first_entry_and_at_the_newest() {
    let mut tail = tail(3);
    for _ in 0..10 {
        tail.scroll_up();
    }
    assert_eq!(tail.scroll_back(), 2, "held at the first entry");

    for _ in 0..10 {
        tail.scroll_down();
    }
    assert_eq!(tail.scroll_back(), 0, "held at the newest");
}

#[test]
fn an_empty_tail_cannot_be_scrolled_anywhere() {
    let mut tail: Tail<usize> = Tail::default();

    tail.scroll_up();
    tail.scroll_to_top();

    assert_eq!(tail.scroll_back(), 0);
    assert!(tail.is_empty());
}

/// The timer refill must not fight an operator reading back through the
/// readings, which a plain `replace` would do every few seconds.
#[test]
fn restocking_keeps_the_operators_place() {
    let mut tail = tail(5);
    tail.scroll_up();
    tail.scroll_up();

    tail.restock(vec![0, 1, 2, 3, 4, 5, 6]);

    assert_eq!(tail.scroll_back(), 4, "still on the same two entries back");

    tail.scroll_to_bottom();
    tail.restock(vec![0, 1, 2, 3, 4, 5, 6, 7]);

    assert_eq!(tail.scroll_back(), 0, "a view at the tail follows it");
}

/// A shorter reading than the last one — the server trimmed its log —
/// cannot leave the view pointing past the first entry.
/// The quota timer refills this list every few seconds with, almost
/// always, the readings it already held. The event loop paints only on a
/// change, so an identical refill must report itself as one.
#[test]
fn restocking_reports_only_a_reading_that_moved() {
    let mut tail = tail(5);

    assert!(
        !tail.restock(vec![0, 1, 2, 3, 4]),
        "the same readings, fetched again"
    );
    assert!(
        tail.restock(vec![0, 1, 2, 3, 4, 5]),
        "a new reading arrived"
    );
}

#[test]
fn restocking_a_shorter_list_clamps_the_view() {
    let mut tail = tail(5);
    tail.scroll_to_top();

    tail.restock(vec![3, 4]);

    assert_eq!(tail.scroll_back(), 1);
}

#[test]
fn replacing_the_contents_returns_to_the_tail() {
    let mut tail = tail(5);
    tail.scroll_to_top();
    assert_eq!(tail.scroll_back(), 4);

    tail.replace(vec![10, 11]);

    assert_eq!(tail.iter().count(), 2);
    assert_eq!(tail.scroll_back(), 0);
}
