use super::*;

fn hunk(old: (usize, usize), new: (usize, usize)) -> Hunk {
    Hunk {
        old_start: old.0,
        old_count: old.1,
        new_start: new.0,
        new_count: new.1,
        old: Vec::new(),
        new: Vec::new(),
    }
}

#[test]
fn lines_move_by_the_changes_before_them() {
    // Two lines inserted after line 1, line 5 replaced by three lines.
    let hunks = [hunk((1, 0), (2, 2)), hunk((5, 1), (7, 3))];
    assert_eq!(map_line(&hunks, 1, Side::Old), 1);
    assert_eq!(map_line(&hunks, 2, Side::Old), 4);
    assert_eq!(map_line(&hunks, 5, Side::Old), 7);
    assert_eq!(map_line(&hunks, 6, Side::Old), 10);
    // Back again; inserted lines go to the line they follow.
    assert_eq!(map_line(&hunks, 3, Side::New), 1);
    assert_eq!(map_line(&hunks, 4, Side::New), 2);
    assert_eq!(map_line(&hunks, 9, Side::New), 5);
    assert_eq!(map_line(&hunks, 10, Side::New), 6);
}

#[test]
fn deleted_lines_go_to_the_line_before_them() {
    let hunks = [hunk((3, 2), (2, 0))];
    assert_eq!(map_line(&hunks, 4, Side::Old), 2);
    assert_eq!(map_line(&hunks, 5, Side::Old), 3);
    assert_eq!(map_gap(&hunks, 2, Side::Old), 2);
    assert_eq!(map_gap(&hunks, 4, Side::Old), 2);
    assert_eq!(map_gap(&hunks, 5, Side::Old), 3);
}

#[test]
fn ranges_are_intact_unless_changed_or_split() {
    let hunks = [hunk((2, 1), (2, 1)), hunk((5, 0), (6, 1))];
    assert!(intact(&hunks, (1, 1)));
    assert!(!intact(&hunks, (1, 2)));
    assert!(intact(&hunks, (3, 3)));
    assert!(!intact(&hunks, (5, 2)));
    assert!(intact(&hunks, (6, 2)));
    // Gaps: nothing inserted there, neighbours unchanged.
    assert!(!intact(&hunks, (5, 0)));
    assert!(!intact(&hunks, (1, 0)));
    assert!(!intact(&hunks, (2, 0)));
    assert!(intact(&hunks, (3, 0)));
    assert!(intact(&[], (0, 0)));
}
