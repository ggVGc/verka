use super::*;

impl Weigh for usize {
    fn weight(&self) -> usize {
        1
    }
}

#[test]
fn cache_hits_reuse_values_and_new_keys_build_once() {
    let mut memo: Memo<String, usize> = Memo::default();
    let mut built = 0;
    for (key, new_value, expected, expected_builds) in [
        ("a", 7, 7, 1),
        ("a", 8, 7, 1),
        ("a", 9, 7, 1),
        ("b", 10, 10, 2),
        ("a", 11, 7, 2),
    ] {
        let value = memo.get_or_insert_with(key.to_owned(), || {
            built += 1;
            new_value
        });
        assert_eq!(value, expected, "{key}: cached value");
        assert_eq!(built, expected_builds, "{key}: build count");
    }
}

#[test]
fn the_table_stays_bounded_and_keeps_what_was_asked_for_last() {
    let mut memo: Memo<usize, usize> = Memo::default();
    for key in 0..CAPACITY_BLOCKS + 10 {
        memo.get_or_insert_with(key, || key);
    }
    assert!(
        memo.entries.len() <= CAPACITY_BLOCKS,
        "{}",
        memo.entries.len()
    );
    assert_eq!(
        memo.lines,
        memo.entries.len(),
        "the line count has to track what is actually held"
    );
    let mut built = 0;
    memo.get_or_insert_with(CAPACITY_BLOCKS + 9, || {
        built += 1;
        0
    });
    assert_eq!(built, 0, "the most recent insertion survived the sweep");
}

/// A rendering too big to cache is still returned, just not stored.
#[test]
fn an_oversized_rendering_is_passed_through() {
    struct Huge;
    impl Weigh for Huge {
        fn weight(&self) -> usize {
            CAPACITY_LINES + 1
        }
    }
    impl Clone for Huge {
        fn clone(&self) -> Self {
            Self
        }
    }
    let mut memo: Memo<&str, Huge> = Memo::default();
    let mut built = 0;
    for _ in 0..2 {
        memo.get_or_insert_with("big", || {
            built += 1;
            Huge
        });
    }
    assert_eq!(built, 2);
    assert!(memo.entries.is_empty());
}
