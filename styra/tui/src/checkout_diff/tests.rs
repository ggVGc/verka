use super::*;
use crossterm::event::{KeyCode, KeyEvent};

/// Coming back to the same comparison keeps the view where it was left;
/// another comparison starts over.
#[test]
fn reopening_the_same_comparison_keeps_the_place() {
    let target = |base: &str| DiffTarget {
        worktree: std::env::temp_dir().join("styra-reopen-no-such-checkout"),
        branch: None,
        base_branch: None,
        base: base.into(),
        branch_point: true,
    };
    let mut shown = CheckoutDiff::read(target("4bf5c35d"));
    shown.per_file = true;
    shown.selected_file = 1;
    shown.scroll.offset = 7;
    shown.line_cursor = Some(3);

    let kept = reopen(Some(shown), target("4bf5c35d"));
    assert!(kept.per_file);
    assert_eq!(kept.selected_file, 1);
    assert_eq!(kept.scroll.offset, 7);
    assert_eq!(kept.line_cursor, Some(3));

    let fresh = reopen(Some(kept), target("0000000"));
    assert!(fresh.per_file, "a fresh read opens on its file list");
    assert_eq!(fresh.scroll.offset, 0);
    assert_eq!(fresh.line_cursor, None);
}

#[test]
fn fuzzy_filter_preserves_recency_and_selects_the_matching_branch() {
    let mut picker = BranchPicker::new(
        "interaction".into(),
        vec!["z-new-topic".into(), "a-old-topic".into(), "other".into()],
    );
    for c in "tpc".chars() {
        picker.handle_key(KeyEvent::from(KeyCode::Char(c)));
    }
    let indices: Vec<_> = picker
        .list
        .matches(&picker.rows)
        .iter()
        .map(|m| m.index)
        .collect();
    assert_eq!(indices, vec![1, 2]);
    picker.handle_key(KeyEvent::from(KeyCode::Down));
    assert!(matches!(picker.handle_key(KeyEvent::from(KeyCode::Enter)),
            BranchChoice::Select(Some(branch)) if branch == "a-old-topic"));
    picker.list.query = "nonexistent".into();
    assert!(matches!(
        picker.handle_key(KeyEvent::from(KeyCode::Enter)),
        BranchChoice::Stay
    ));
    assert!(matches!(
        picker.handle_key(KeyEvent::from(KeyCode::Esc)),
        BranchChoice::Cancel
    ));
}
