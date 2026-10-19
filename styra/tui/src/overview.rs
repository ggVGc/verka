//! The overview's cursor: which tile of the grid the operator is on.
//!
//! The tiles themselves are the live-interaction snapshot the event loop
//! already keeps fresh — see [`LiveInteractions::overview_indices`] — so this
//! holds only the place in them and the shape the grid was last drawn in.

use crate::interactions::LiveInteractions;

#[derive(Clone, Debug, Default)]
pub struct Overview {
    /// The interaction under the cursor. `None` — what opening resets it to —
    /// means the one on screen, or the first tile when that is not one.
    cursor: Option<String>,
    pub running_only: bool,
    /// How many tiles each row of the grid held when it was last drawn, top
    /// to bottom, which is what moving up or down steps between. The renderer
    /// decides it from the terminal's size, so it is learned from the draw.
    rows: Vec<usize>,
    /// The Workspace a Workspace scope shows in place of the one on screen,
    /// while the Workspace jump previews one and until its choice has loaded.
    pub workspace: Option<String>,
}

impl Overview {
    /// Start from the interaction on screen, wherever the cursor was left.
    pub fn open(&mut self) {
        self.cursor = None;
        self.rows.clear();
        self.workspace = None;
    }

    pub fn toggle_running_only(&mut self) {
        self.running_only = !self.running_only;
        self.rows.clear();
    }

    /// The visible tiles, shared by drawing and cursor navigation.
    pub fn indices(
        &self,
        interactions: &LiveInteractions,
        workspace_id: Option<&str>,
    ) -> Vec<usize> {
        interactions
            .overview_indices(self.scope(workspace_id))
            .into_iter()
            .filter(|index| {
                !self.running_only
                    || matches!(
                        interactions.items[*index].activity,
                        styra_protocol::InteractionActivity::Running
                            | styra_protocol::InteractionActivity::Background
                    )
            })
            .collect()
    }

    pub fn note_rows(&mut self, rows: &[usize]) {
        self.rows = rows.to_vec();
    }

    /// The cursor's position in [`LiveInteractions::overview_indices`]. A
    /// cursor whose interaction has stopped since, and so left the grid,
    /// falls back as an unmoved one does.
    pub fn selected(
        &self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) -> usize {
        let tiles = self.indices(interactions, workspace_id);
        let position = |id: &str| {
            tiles
                .iter()
                .position(|index| interactions.items[*index].id == id)
        };
        self.cursor
            .as_deref()
            .and_then(position)
            .or_else(|| position(current))
            .unwrap_or_default()
    }

    /// The id of the interaction under the cursor, if there is any tile.
    pub fn selected_id<'a>(
        &self,
        interactions: &'a LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<&'a str> {
        let tiles = self.indices(interactions, workspace_id);
        tiles
            .get(self.selected(interactions, current, workspace_id))
            .map(|index| interactions.items[*index].id.as_str())
    }

    /// Follow newly idle work first, then live work, from the selected tile.
    /// Like the navigator's `n`, this reveals destinations hidden by a filter.
    pub fn next_attention_or_live(
        &self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<styra_protocol::InteractionSummary> {
        let from = self
            .selected_id(interactions, current, workspace_id)
            .unwrap_or(current);
        interactions
            .next_attention_or_live_in_order(
                &interactions.overview_indices(self.scope(workspace_id)),
                from,
            )
    }

    /// The Workspace a Workspace scope shows: the previewed one, if any, else
    /// `workspace_id`, the one on screen.
    pub(crate) fn scope<'a>(&'a self, workspace_id: Option<&'a str>) -> Option<&'a str> {
        self.workspace.as_deref().or(workspace_id)
    }

    pub fn left(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let at = self.selected(interactions, current, workspace_id);
        self.select(interactions, at.saturating_sub(1), workspace_id);
    }

    pub fn right(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let at = self.selected(interactions, current, workspace_id);
        self.select(interactions, at + 1, workspace_id);
    }

    /// Onto the tile in the row below that sits most nearly under this one.
    /// Rows of different lengths have tiles of different widths, so that is
    /// the one under this tile's middle.
    pub fn down(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let at = self.selected(interactions, current, workspace_id);
        if let Some(position) = self.vertical(at, 1) {
            self.select(interactions, position, workspace_id);
        }
    }

    pub fn up(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let at = self.selected(interactions, current, workspace_id);
        if let Some(position) = self.vertical(at, -1) {
            self.select(interactions, position, workspace_id);
        }
    }

    /// The tile `step` rows from tile `at` under its middle, or `None` when
    /// there is no such row.
    fn vertical(&self, at: usize, step: isize) -> Option<usize> {
        let mut start = 0;
        let row = self.rows.iter().position(|length| {
            start += length;
            at < start
        })?;
        let length = self.rows[row];
        let column = at - (start - length);
        let target = row.checked_add_signed(step)?;
        let target_length = *self.rows.get(target)?;
        let target_start = self.rows[..target].iter().sum::<usize>();
        // The middle of this tile, as a fraction of the width, is
        // (column + ½) / length; the tile below it is that times the target
        // row's length, rounded down.
        let target_column =
            ((2 * column + 1) * target_length / (2 * length)).min(target_length - 1);
        Some(target_start + target_column)
    }

    /// The next tile in reading order, wrapping from the last to the first,
    /// as Tab moves between windows.
    pub fn next(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let tiles = self.indices(interactions, workspace_id).len().max(1);
        let at = self.selected(interactions, current, workspace_id);
        self.select(interactions, (at + 1) % tiles, workspace_id);
    }

    /// The previous tile, wrapping from the first to the last.
    pub fn prev(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let tiles = self.indices(interactions, workspace_id).len().max(1);
        let at = self.selected(interactions, current, workspace_id);
        self.select(interactions, (at + tiles - 1) % tiles, workspace_id);
    }

    pub fn first(&mut self, interactions: &LiveInteractions, workspace_id: Option<&str>) {
        self.select(interactions, 0, workspace_id);
    }

    pub fn last(&mut self, interactions: &LiveInteractions, workspace_id: Option<&str>) {
        self.select(interactions, usize::MAX, workspace_id);
    }

    /// Onto tile `position` in reading order, counted from zero. A position
    /// past the last tile leaves the cursor where it is, so a count that
    /// misses does not land somewhere unasked for.
    pub fn nth(
        &mut self,
        interactions: &LiveInteractions,
        position: usize,
        workspace_id: Option<&str>,
    ) {
        if position < self.indices(interactions, workspace_id).len() {
            self.select(interactions, position, workspace_id);
        }
    }

    /// Put the cursor on the first of `target`'s tiles, for the Workspace
    /// jump. Reports whether `target` has a tile to land on.
    pub fn select_workspace(
        &mut self,
        interactions: &LiveInteractions,
        target: &str,
        workspace_id: Option<&str>,
    ) -> bool {
        let Some(index) = self
            .indices(interactions, workspace_id)
            .into_iter()
            .find(|index| interactions.items[*index].workspace_id == target)
        else {
            return false;
        };
        self.cursor = Some(interactions.items[index].id.clone());
        true
    }

    /// Put the cursor on tile `position`, or on the last tile past the end.
    fn select(
        &mut self,
        interactions: &LiveInteractions,
        position: usize,
        workspace_id: Option<&str>,
    ) {
        let tiles = self.indices(interactions, workspace_id);
        if let Some(index) = tiles.get(position).or(tiles.last()) {
            self.cursor = Some(interactions.items[*index].id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
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
    fn only_interactions_still_taking_turns_are_tiled() {
        let interactions = fleet(&[
            InteractionActivity::Running,
            InteractionActivity::Stopped,
            InteractionActivity::Pending,
            InteractionActivity::Background,
        ]);
        assert_eq!(ids(&interactions), ["100-tile", "98-tile", "97-tile"]);
    }

    #[test]
    fn tiles_follow_creation_order_regardless_of_activity() {
        let interactions = fleet(&[
            InteractionActivity::Pending,
            InteractionActivity::Background,
            InteractionActivity::Running,
            InteractionActivity::Pending,
        ]);
        assert_eq!(
            ids(&interactions),
            ["100-tile", "99-tile", "98-tile", "97-tile"]
        );
    }

    #[test]
    fn newly_idle_tiles_keep_their_creation_order() {
        let mut interactions = fleet(&[
            InteractionActivity::Pending,
            InteractionActivity::Running,
            InteractionActivity::Pending,
            InteractionActivity::Pending,
        ]);
        interactions.items[2].idle_unseen = true;
        assert_eq!(
            ids(&interactions),
            ["100-tile", "99-tile", "98-tile", "97-tile"]
        );
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
        overview.last(&interactions, None);
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

        overview.toggle_running_only();
        assert_eq!(
            overview
                .next_attention_or_live(&interactions, current, None)
                .unwrap()
                .id,
            "98-tile"
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
        assert_eq!(overview.indices(&interactions, scope).len(), 3);
        assert_eq!(
            overview
                .next_attention_or_live(&interactions, current, scope)
                .unwrap()
                .id,
            "98-tile"
        );
        overview.last(&interactions, scope);
        interactions.toggle_workspace_scope();
        assert_eq!(overview.indices(&interactions, scope), vec![0]);
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
        assert!(overview.indices(&interactions, None).is_empty());

        interactions.toggle_workspace_scope();
        assert_eq!(overview.indices(&interactions, scope).len(), 3);
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
        let current = "100-tile";
        overview.toggle_running_only();
        assert_eq!(overview.indices(&interactions, None).len(), 2);
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
        assert!(overview.indices(&interactions, None).is_empty());
        assert_eq!(overview.selected_id(&interactions, current, None), None);
        overview.next(&interactions, current, None);
        overview.prev(&interactions, current, None);
        overview.toggle_running_only();
        assert_eq!(overview.indices(&interactions, None).len(), 3);
        assert_eq!(
            overview.selected_id(&interactions, current, None),
            Some("99-tile")
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

        overview.first(&interactions, None);
        assert_eq!(
            overview.selected_id(&interactions, current, None),
            Some("100-tile")
        );
        overview.last(&interactions, None);
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
}
