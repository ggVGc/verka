use super::*;
use crate::interactions::tests::interaction;
use styra_protocol::InteractionActivity;

fn fleet(activities: &[InteractionActivity]) -> LiveInteractions {
    let mut interactions = LiveInteractions::default();
    let items = activities
        .iter()
        .enumerate()
        // Ids lead with their creation time, and the list is newest
        // first, so counting down keeps the tiles in the order given.
        .map(|(index, activity)| interaction(&format!("{}-tile", 100 - index), *activity))
        .collect();
    interactions.open(items, Vec::new());
    interactions
}

fn ids(interactions: &LiveInteractions) -> Vec<String> {
    interactions
        .overview_indices(None)
        .into_iter()
        .map(|index| interactions.items[index].id.clone())
        .collect()
}

#[test]
fn tiles_filter_stopped_work_and_order_newest_first_whatever_the_state() {
    use InteractionActivity::{Background, Pending, Running, Stopped};
    for (name, activities, unseen, expected) in [
        (
            "exclude stopped",
            vec![Running, Stopped, Pending, Background],
            None,
            vec!["100-tile", "98-tile", "97-tile"],
        ),
        (
            "working does not jump ahead",
            vec![Pending, Background, Running, Pending],
            None,
            vec!["100-tile", "99-tile", "98-tile", "97-tile"],
        ),
        (
            "newly idle does not jump ahead",
            vec![Pending, Running, Pending, Pending],
            Some(2),
            vec!["100-tile", "99-tile", "98-tile", "97-tile"],
        ),
    ] {
        let mut interactions = fleet(&activities);
        if let Some(index) = unseen {
            interactions.items[index].idle_unseen = true;
        }
        assert_eq!(ids(&interactions), expected, "{name}");
    }
}

#[test]
fn next_live_prioritizes_newly_idle_from_the_selected_tile() {
    let mut interactions = fleet(&[
        InteractionActivity::Running,
        InteractionActivity::Background,
        InteractionActivity::Pending,
        InteractionActivity::Pending,
    ]);
    interactions.items[2].idle_unseen = true;
    interactions.items[3].idle_unseen = true;
    let mut overview = Overview::default();
    let current = "100-tile";
    assert_eq!(
        overview
            .next_attention_or_live(&interactions, current, None)
            .unwrap()
            .id,
        "98-tile"
    );
    overview.last(&interactions, current, None);
    assert_eq!(
        overview
            .next_attention_or_live(&interactions, current, None)
            .unwrap()
            .id,
        "98-tile"
    );
    overview.prev(&interactions, current, None);
    assert_eq!(
        overview
            .next_attention_or_live(&interactions, current, None)
            .unwrap()
            .id,
        "97-tile"
    );

    // Newly idle tiles survive running only, so the cursor stays put.
    overview.toggle_running_only();
    assert_eq!(
        overview
            .next_attention_or_live(&interactions, current, None)
            .unwrap()
            .id,
        "97-tile"
    );
    interactions.items[2].idle_unseen = false;
    interactions.items[3].idle_unseen = false;
    assert_eq!(
        overview
            .next_attention_or_live(&interactions, current, None)
            .unwrap()
            .id,
        "99-tile"
    );
}

#[test]
fn workspace_scope_is_shared_by_tiles_cursor_and_attention_steps() {
    let mut interactions = fleet(&[
        InteractionActivity::Running,
        InteractionActivity::Running,
        InteractionActivity::Pending,
    ]);
    interactions.items[1].workspace_id = "other".into();
    interactions.items[2].workspace_id = "other".into();
    interactions.items[2].idle_unseen = true;
    let mut overview = Overview::default();
    let scope = Some("workspace");
    let current = "100-tile";
    assert_eq!(overview.indices(&interactions, current, scope).len(), 3);
    assert_eq!(
        overview
            .next_attention_or_live(&interactions, current, scope)
            .unwrap()
            .id,
        "98-tile"
    );
    overview.last(&interactions, current, scope);
    interactions.toggle_workspace_scope();
    assert_eq!(overview.indices(&interactions, current, scope), vec![0]);
    assert_eq!(
        overview.selected_id(&interactions, current, scope),
        Some(current)
    );
    overview.next(&interactions, current, scope);
    overview.prev(&interactions, current, scope);
    assert_eq!(
        overview.selected_id(&interactions, current, scope),
        Some(current)
    );
    assert!(overview
        .next_attention_or_live(&interactions, current, scope)
        .is_none());
    assert!(interactions
        .next_attention_or_live(current, scope)
        .is_none());
    assert!(interactions.next_active(current, scope).is_none());
    assert!(overview.indices(&interactions, current, None).is_empty());

    interactions.toggle_workspace_scope();
    assert_eq!(overview.indices(&interactions, current, scope).len(), 3);
    assert_eq!(
        overview
            .next_attention_or_live(&interactions, current, scope)
            .unwrap()
            .id,
        "98-tile"
    );
    assert_eq!(
        interactions.next_active(current, scope).unwrap().id,
        "99-tile"
    );
}

#[test]
fn running_only_filters_navigation_and_tracks_activity_changes() {
    let mut interactions = fleet(&[
        InteractionActivity::Pending,
        InteractionActivity::Running,
        InteractionActivity::Background,
    ]);
    let mut overview = Overview::default();
    let current = "elsewhere";
    overview.toggle_running_only();
    assert_eq!(overview.indices(&interactions, current, None).len(), 2);
    assert_eq!(
        overview.selected_id(&interactions, current, None),
        Some("99-tile")
    );
    overview.next(&interactions, current, None);
    assert_eq!(
        overview.selected_id(&interactions, current, None),
        Some("98-tile")
    );
    overview.next(&interactions, current, None);
    assert_eq!(
        overview.selected_id(&interactions, current, None),
        Some("99-tile")
    );

    for item in &mut interactions.items {
        item.activity = InteractionActivity::Pending;
    }
    assert!(overview.indices(&interactions, current, None).is_empty());
    assert_eq!(overview.selected_id(&interactions, current, None), None);
    overview.next(&interactions, current, None);
    overview.prev(&interactions, current, None);
    overview.toggle_running_only();
    assert_eq!(overview.indices(&interactions, current, None).len(), 3);
    assert_eq!(
        overview.selected_id(&interactions, current, None),
        Some("99-tile")
    );
}

/// Running only must not lose what needs the operator: newly idle work,
/// and the interaction on screen, stay on the grid.
#[test]
fn running_only_keeps_newly_idle_work_and_the_interaction_on_screen() {
    let mut interactions = fleet(&[
        InteractionActivity::Pending,
        InteractionActivity::Pending,
        InteractionActivity::Running,
        InteractionActivity::Pending,
    ]);
    interactions.items[1].idle_unseen = true;
    let mut overview = Overview::default();
    overview.toggle_running_only();
    let tiles = |overview: &Overview, interactions: &LiveInteractions, current| {
        overview
            .indices(interactions, current, None)
            .into_iter()
            .map(|index| interactions.items[index].id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        tiles(&overview, &interactions, "100-tile"),
        vec!["100-tile", "99-tile", "98-tile"]
    );
    assert_eq!(
        tiles(&overview, &interactions, "elsewhere"),
        vec!["99-tile", "98-tile"]
    );
    // Once seen it is no longer newly idle, and goes unless it is on screen.
    interactions.items[1].idle_unseen = false;
    assert_eq!(
        tiles(&overview, &interactions, "99-tile"),
        vec!["99-tile", "98-tile"]
    );
    assert_eq!(
        tiles(&overview, &interactions, "100-tile"),
        vec!["100-tile", "98-tile"]
    );
}

#[test]
fn the_cursor_starts_on_the_interaction_on_screen() {
    let interactions = fleet(&[InteractionActivity::Pending; 3]);
    let mut overview = Overview::default();
    overview.open();
    assert_eq!(overview.selected(&interactions, "99-tile", None), 1);
    assert_eq!(overview.selected(&interactions, "elsewhere", None), 0);
}

/// Five tiles three over two: down and up land on the tile under the
/// middle of the one the cursor leaves, the bottom row's being wider.
#[test]
fn the_cursor_moves_across_and_down_the_grid() {
    let interactions = fleet(&[InteractionActivity::Pending; 5]);
    let mut overview = Overview::default();
    overview.toggle_layout();
    overview.note_rows(&[3, 2]);
    let current = "100-tile";

    overview.down(&interactions, current, None);
    assert_eq!(overview.selected(&interactions, current, None), 3);
    overview.up(&interactions, current, None);
    assert_eq!(overview.selected(&interactions, current, None), 0);

    overview.right(&interactions, current, None);
    assert_eq!(overview.selected(&interactions, current, None), 1);
    overview.down(&interactions, current, None);
    assert_eq!(overview.selected(&interactions, current, None), 4);
    overview.down(&interactions, current, None);
    assert_eq!(
        overview.selected(&interactions, current, None),
        4,
        "no row below"
    );
    overview.up(&interactions, current, None);
    assert_eq!(
        overview.selected(&interactions, current, None),
        2,
        "the right half of the top row"
    );
    overview.up(&interactions, current, None);
    assert_eq!(
        overview.selected(&interactions, current, None),
        2,
        "no row above"
    );
    overview.down(&interactions, current, None);
    assert_eq!(overview.selected(&interactions, current, None), 4);
    overview.right(&interactions, current, None);
    assert_eq!(
        overview.selected(&interactions, current, None),
        4,
        "at the end"
    );

    overview.first(&interactions, current, None);
    assert_eq!(
        overview.selected_id(&interactions, current, None),
        Some("100-tile")
    );
    overview.last(&interactions, current, None);
    assert_eq!(
        overview.selected_id(&interactions, current, None),
        Some("96-tile")
    );
}

#[test]
fn tab_walks_the_tiles_in_order_and_wraps() {
    let interactions = fleet(&[InteractionActivity::Pending; 3]);
    let mut overview = Overview::default();
    overview.note_rows(&[2, 1]);
    let current = "99-tile";

    overview.next(&interactions, current, None);
    assert_eq!(
        overview.selected(&interactions, current, None),
        2,
        "across the row break"
    );
    overview.next(&interactions, current, None);
    assert_eq!(
        overview.selected(&interactions, current, None),
        0,
        "wraps to the first"
    );
    overview.prev(&interactions, current, None);
    assert_eq!(
        overview.selected(&interactions, current, None),
        2,
        "wraps to the last"
    );
    overview.prev(&interactions, current, None);
    assert_eq!(overview.selected(&interactions, current, None), 1);
}

#[test]
fn two_column_navigation_keeps_the_column_above_an_odd_last_tile() {
    let interactions = fleet(&[InteractionActivity::Pending; 5]);
    let mut overview = Overview::default();
    overview.note_rows(&[2, 2, 1]);
    overview.last(&interactions, "100-tile", None);
    overview.up(&interactions, "100-tile", None);
    assert_eq!(overview.selected(&interactions, "100-tile", None), 2);
    overview.down(&interactions, "100-tile", None);
    assert_eq!(overview.selected(&interactions, "100-tile", None), 4);
    overview.open();
    assert_eq!(
        overview.layout,
        styra_ui::overview::OverviewLayout::TwoColumns
    );
    overview.toggle_layout();
    assert_eq!(overview.layout, styra_ui::overview::OverviewLayout::Auto);
}
