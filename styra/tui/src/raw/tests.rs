use super::*;
use styra_protocol::Direction;

fn line(text: &str) -> RawLine {
    RawLine {
        at_ms: 0,
        direction: Direction::FromAgent,
        text: text.into(),
    }
}

#[test]
fn provider_raw_is_a_separately_navigable_jsonl_document() {
    let mut raw = ProviderRawView::new("{\"type\":\"first\"}\n{\"type\":\"second\"}\n".into());
    assert_eq!(raw.selected(), Some(r#"{"type":"first"}"#));
    raw.select_next();
    assert_eq!(raw.selected(), Some(r#"{"type":"second"}"#));
    raw.select_last();
    assert_eq!(raw.selected_index(), 1);
}

#[test]
fn the_selection_tracks_the_tail_until_the_operator_moves_it() {
    let mut raw = RawView::default();
    for i in 0..5 {
        raw.push(line(&format!("line {i}")));
    }
    assert_eq!(raw.selected_index(), 4, "starts pinned to the tail");
    assert!(raw.is_following());

    raw.select_prev();
    assert_eq!(raw.selected_index(), 3);
    assert!(!raw.is_following());

    // A new line while a specific line is selected keeps that same line
    // in view rather than yanking to the new tail.
    raw.push(line("new"));
    assert_eq!(raw.selected_index(), 3);

    raw.select_last();
    assert_eq!(raw.selected_index(), 5);
    assert!(raw.is_following());
    raw.select_first();
    assert_eq!(raw.selected_index(), 0);
    assert!(!raw.is_following());
}

/// Stepping back down to the last line means the operator has caught up
/// with the stream, so the view resumes tracking it.
#[test]
fn reaching_the_tail_again_resumes_following() {
    let mut raw = RawView::default();
    raw.push(line("one"));
    raw.push(line("two"));
    raw.select_prev();
    assert!(!raw.is_following());

    raw.select_next();

    assert_eq!(raw.selected_index(), 1);
    assert!(raw.is_following());
}

#[test]
fn entering_pins_clamps_or_follows_the_available_lines() {
    for (name, count, entries) in [
        (
            "reenter to follow",
            3,
            vec![(Some(1), 1, false), (None, 2, true)],
        ),
        // An index saved before lazy attach may outlive its line.
        ("clamp stale index", 1, vec![(Some(7), 0, false)]),
        ("empty", 0, vec![(Some(3), 0, true)]),
    ] {
        let mut raw = RawView::default();
        for i in 0..count {
            raw.push(line(&format!("line {i}")));
        }
        for (requested, selected, follows) in entries {
            raw.enter(requested);
            assert_eq!(raw.selected_index(), selected, "{name}: {requested:?}");
            assert_eq!(raw.is_following(), follows, "{name}: {requested:?}");
            if count == 0 {
                assert!(raw.selected().is_none(), "{name}");
            }
        }
    }
}

#[test]
fn navigating_an_empty_view_does_nothing() {
    let mut raw = RawView::default();

    raw.select_first();
    raw.select_last();
    raw.select_prev();

    assert_eq!(raw.selected_index(), 0);
    assert!(raw.is_empty());
}
