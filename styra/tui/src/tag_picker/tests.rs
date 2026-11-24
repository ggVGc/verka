use super::TagPicker;
use styra_ui::query::Edit;

#[test]
fn a_filter_cannot_start_a_new_tag() {
    let mut picker = TagPicker::filter(vec!["bug".into()], vec![]);
    picker.type_query(Edit::Push('x'));
    picker.start_new();
    assert!(picker.new_tag.is_none());
}

#[test]
fn adding_a_tag_selects_it_and_makes_it_available() {
    let mut picker = TagPicker::new(vec!["bug".into()], vec![]);
    picker.new_tag = Some("urgent".into());
    picker.add_new();
    assert_eq!(picker.available, ["bug", "urgent"]);
    assert_eq!(picker.selected, ["urgent"]);
}

#[test]
fn typing_narrows_the_list_and_toggle_acts_on_the_match() {
    let mut picker = TagPicker::new(
        vec!["bug".into(), "feature".into(), "urgent".into()],
        vec![],
    );
    for character in "urg".chars() {
        picker.type_query(Edit::Push(character));
    }
    picker.toggle();
    assert_eq!(picker.selected, ["urgent"]);
    assert!(!picker.list.is_filtering());
}

#[test]
fn excluding_a_tag_replaces_an_inclusion_and_toggles_off() {
    let mut picker = TagPicker::filter(vec!["bug".into()], vec!["bug".into()]);
    picker.toggle_excluded();
    assert_eq!(picker.selected, ["-bug"]);

    picker.toggle_excluded();
    assert!(picker.selected.is_empty());
}

#[test]
fn including_a_tag_replaces_an_exclusion() {
    let mut picker = TagPicker::filter(vec!["bug".into()], vec!["-bug".into()]);
    picker.toggle();
    assert_eq!(picker.selected, ["bug"]);
}

#[test]
fn a_filter_does_not_add_a_stale_selection_to_the_available_tags() {
    let picker = TagPicker::filter(vec!["live".into()], vec!["-old".into()]);
    assert_eq!(picker.available, ["live"]);
    assert_eq!(picker.selected, ["-old"]);
}

#[test]
fn toggling_with_nothing_matching_changes_nothing() {
    let mut picker = TagPicker::new(vec!["bug".into()], vec![]);
    for character in "zzz".chars() {
        picker.type_query(Edit::Push(character));
    }
    picker.toggle();
    assert!(picker.selected.is_empty());
    assert_eq!(picker.list.query, "zzz");
}

#[test]
fn a_new_tag_starts_from_the_filter() {
    let mut picker = TagPicker::new(vec!["bug".into()], vec![]);
    for character in "docs".chars() {
        picker.type_query(Edit::Push(character));
    }
    picker.start_new();
    assert_eq!(picker.new_tag.as_deref(), Some("docs"));
}
