use super::*;
use crate::interactions::tests::interaction;
use styra_protocol::InteractionActivity;

fn shown<'a>(live: &'a LiveInteractions, workspace_id: Option<&str>) -> Vec<&'a str> {
    live.display_indices("", workspace_id)
        .into_iter()
        .map(|index| live.items[index].id.as_str())
        .collect()
}

#[test]
fn time_windows_include_the_boundary_and_use_the_latest_user_message() {
    const DAY: u64 = 86_400_000;
    let now = 20 * DAY;
    let mut item = interaction("1-0-0", InteractionActivity::Pending);
    for (filter, days) in [
        (Window::PastDay, 1),
        (Window::Past3Days, 3),
        (Window::PastWeek, 7),
    ] {
        item.last_user_message_at_ms = now - days * DAY;
        assert!(filter.includes_at(&item, now));
        item.last_user_message_at_ms -= 1;
        assert!(!filter.includes_at(&item, now));
        item.last_user_message_at_ms = now + 1;
        assert!(filter.includes_at(&item, now));
        item.last_user_message_at_ms = 0;
        item.id = format!("{}-0-0", now - days * DAY);
        assert!(filter.includes_at(&item, now));
        item.id = "1-0-0".into();
        assert!(!filter.includes_at(&item, now));
    }
    item.id = "unknown".into();
    assert!(Window::Disabled.includes_at(&item, now));
    assert!(!Window::PastWeek.includes_at(&item, now));
}

#[test]
fn time_filter_cycles_retains_current_and_composes_with_other_filters() {
    let mut live = LiveInteractions::default();
    let mut recent = interaction("recent", InteractionActivity::Pending);
    recent.last_user_message_at_ms = u64::MAX;
    recent.tags = vec!["keep".into()];
    let old = interaction("1-0-0", InteractionActivity::Pending);
    live.open(vec![recent, old], vec![]);
    assert_eq!(live.time_filter.window, Window::Disabled);
    live.move_cursor_to("1-0-0".into(), "recent");
    for filter in [Window::PastDay, Window::Past3Days, Window::PastWeek] {
        live.cycle_time_filter("recent", None);
        assert_eq!(live.time_filter.window, filter);
        assert_eq!(shown(&live, None), ["recent"]);
        assert_eq!(live.cursor("recent"), "recent");
        // The current screen is kept even when it is outside the window.
        assert_eq!(live.visible_indices("1-0-0", None).len(), 2);
    }
    live.close();
    live.show(vec![]);
    assert_eq!(live.time_filter.window, Window::PastWeek);
    live.set_tag_filter(vec!["missing".into()], "", None);
    assert!(shown(&live, None).is_empty());
    live.set_tag_filter(vec![], "", None);
    live.cycle_time_filter("", None);
    assert_eq!(live.time_filter.window, Window::Disabled);
    assert_eq!(shown(&live, None), ["recent", "1-0-0"]);
}

#[test]
fn the_overview_shares_the_time_window() {
    let mut live = LiveInteractions::default();
    let mut recent = interaction("recent", InteractionActivity::Pending);
    recent.last_user_message_at_ms = u64::MAX;
    let old = interaction("1-0-0", InteractionActivity::Pending);
    live.open(vec![recent, old], vec![]);
    assert_eq!(live.overview_indices(None).len(), 2);
    live.cycle_time_filter("1-0-0", None);
    assert_eq!(live.overview_indices(None), [0]);
}

#[test]
fn a_branch_group_is_shown_whole_when_any_of_it_is_in_the_window() {
    const DAY: u64 = 86_400_000;
    let now = 20 * DAY;
    let branch = |id: &str, from: &str| {
        let mut item = interaction(id, InteractionActivity::Pending);
        item.origin = Some(styra_protocol::SessionOrigin {
            session_id: from.into(),
            provider: item.selection.provider,
            at_ms: None,
            history: Default::default(),
        });
        item
    };
    let source = interaction("1-0-0", InteractionActivity::Pending);
    let mut recent = branch("2-0-0", "1-0-0");
    recent.last_user_message_at_ms = now;
    let sibling = branch("3-0-0", "1-0-0");
    let nephew = branch("4-0-0", "3-0-0");
    let unrelated = interaction("5-0-0", InteractionActivity::Pending);
    let mut live = LiveInteractions::default();
    live.open(vec![source, recent, sibling, nephew, unrelated], vec![]);
    live.time_filter.window = Window::PastDay;
    fn included(live: &LiveInteractions, now: u64) -> Vec<&str> {
        let mut ids: Vec<&str> = live
            .items
            .iter()
            .filter(|item| live.in_time_window_at(item, now))
            .map(|item| item.id.as_str())
            .collect();
        ids.sort();
        ids
    }
    assert_eq!(included(&live, now), ["1-0-0", "2-0-0", "3-0-0", "4-0-0"]);
    // Siblings stay grouped once their source has left the listing.
    live.items.retain(|item| item.id != "1-0-0");
    assert_eq!(included(&live, now), ["2-0-0", "3-0-0", "4-0-0"]);
    live.items.retain(|item| item.id != "2-0-0");
    assert!(included(&live, now).is_empty());
}
