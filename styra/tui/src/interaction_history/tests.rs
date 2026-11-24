use super::*;

#[test]
fn back_and_forward_preserve_entries_until_a_new_choice() {
    let mut history = InteractionHistory::default();
    for id in ["a", "b", "c"] {
        history.record(id);
    }
    assert_eq!(history.target("c", false), Some((1, "b")));
    history.arrive(1);
    history.record("b");
    assert_eq!(history.target("b", true), Some((2, "c")));
    history.record("d");
    assert_eq!(history.entries, ["a", "b", "d"]);
    assert_eq!(history.target("d", true), None);
}

#[test]
fn previews_and_failed_loads_leave_the_committed_position_intact() {
    let mut history = InteractionHistory::default();
    history.record("a");
    history.record("b");
    assert_eq!(history.target("preview", false), Some((1, "b")));
    assert_eq!(history.target("b", false), Some((0, "a")));
    assert_eq!(history.target("b", false), Some((0, "a")));
}

#[test]
fn history_is_bounded_and_ignores_blank_and_repeated_entries() {
    let mut history = InteractionHistory::default();
    history.record("");
    assert_eq!(history.target("", false), None);
    for index in 0..MAX_ENTRIES + 10 {
        history.record(&index.to_string());
        history.record(&index.to_string());
    }
    assert_eq!(history.entries.len(), MAX_ENTRIES);
    assert_eq!(history.entries[0], "10");
    for index in (0..MAX_ENTRIES - 1).rev() {
        let current = history.entries[history.cursor.unwrap()].clone();
        assert_eq!(history.target(&current, false).unwrap().0, index);
        history.arrive(index);
    }
    assert_eq!(history.target("10", false), None);
}
