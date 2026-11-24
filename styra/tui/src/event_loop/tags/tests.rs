use super::*;
use crate::event_loop::navigation::cursored_interaction_is_loadable;
use crate::tag_picker::TagPicker;
use std::time::Duration;
use styra_protocol::InteractionActivity;

#[test]
fn toggling_a_tag_filter_applies_it_immediately() {
    let mut app =
        App::pending(styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap());
    app.session_id = "one".into();
    let current = crate::interactions::tests::interaction("one", InteractionActivity::Pending);
    let mut matching = crate::interactions::tests::interaction("two", InteractionActivity::Pending);
    matching.tags = vec!["bug".into()];
    app.interactions.open(vec![current, matching], Vec::new());
    app.tag_picker = Some(TagPicker::filter(vec!["bug".into()], Vec::new()));

    toggle_tag_picker_selection(&mut app);
    assert_eq!(app.interactions.tag_filter(), ["bug"]);
    assert!(app.tag_picker.is_some());
    assert!(app.interactions.pending("one").is_some());
    std::thread::sleep(Duration::from_millis(140));
    assert!(!cursored_interaction_is_loadable(&app));

    toggle_tag_picker_selection(&mut app);
    assert!(app.interactions.tag_filter().is_empty());
    assert!(app.tag_picker.is_some());

    toggle_tag_picker_selection(&mut app);
    clear_tag_picker_selection(&mut app);
    assert!(app.interactions.tag_filter().is_empty());
    assert!(app
        .tag_picker
        .as_ref()
        .is_some_and(|picker| picker.selected.is_empty()));

    app.tag_picker = Some(TagPicker::filter(vec!["bug".into()], vec!["bug".into()]));
    clear_tag_picker_selection(&mut app);
    restore_tag_picker_selection(&mut app);
    assert_eq!(app.interactions.tag_filter(), ["bug"]);
    assert_eq!(app.tag_picker.as_ref().unwrap().selected, ["bug"]);

    app.tag_picker = None;
    assert!(cursored_interaction_is_loadable(&app));
}

#[test]
fn excluding_a_tag_filter_applies_it_immediately() {
    let mut app =
        App::pending(styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap());
    app.session_id = "one".into();
    let current = crate::interactions::tests::interaction("one", InteractionActivity::Pending);
    let mut excluded = crate::interactions::tests::interaction("two", InteractionActivity::Pending);
    excluded.tags = vec!["bug".into()];
    app.interactions.open(vec![current, excluded], Vec::new());
    app.tag_picker = Some(TagPicker::filter(vec!["bug".into()], Vec::new()));

    toggle_tag_picker_exclusion(&mut app);
    assert_eq!(app.interactions.tag_filter(), ["-bug"]);
    assert_eq!(app.interactions.visible_indices("", None), vec![0]);

    toggle_tag_picker_exclusion(&mut app);
    assert!(app.interactions.tag_filter().is_empty());
    assert_eq!(app.interactions.visible_indices("", None), vec![0, 1]);
}
