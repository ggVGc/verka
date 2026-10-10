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
}

impl Overview {
    /// Start from the interaction on screen, wherever the cursor was left.
    pub fn open(&mut self) {
        self.cursor = None;
    }

    pub fn toggle_running_only(&mut self) {
        self.running_only = !self.running_only;
        self.rows.clear();
    }

    /// The visible tiles, shared by drawing and cursor navigation.
    pub fn indices(&self, interactions: &LiveInteractions) -> Vec<usize> {
        interactions
            .overview_indices()
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
    pub fn selected(&self, interactions: &LiveInteractions, current: &str) -> usize {
        let tiles = self.indices(interactions);
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
    ) -> Option<&'a str> {
        let tiles = self.indices(interactions);
        tiles
            .get(self.selected(interactions, current))
            .map(|index| interactions.items[*index].id.as_str())
    }

    pub fn left(&mut self, interactions: &LiveInteractions, current: &str) {
        let at = self.selected(interactions, current);
        self.select(interactions, at.saturating_sub(1));
    }

    pub fn right(&mut self, interactions: &LiveInteractions, current: &str) {
        let at = self.selected(interactions, current);
        self.select(interactions, at + 1);
    }

    /// Onto the tile in the row below that sits most nearly under this one.
    /// Rows of different lengths have tiles of different widths, so that is
    /// the one under this tile's middle.
    pub fn down(&mut self, interactions: &LiveInteractions, current: &str) {
        let at = self.selected(interactions, current);
        if let Some(position) = self.vertical(at, 1) {
            self.select(interactions, position);
        }
    }

    pub fn up(&mut self, interactions: &LiveInteractions, current: &str) {
        let at = self.selected(interactions, current);
        if let Some(position) = self.vertical(at, -1) {
            self.select(interactions, position);
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
    pub fn next(&mut self, interactions: &LiveInteractions, current: &str) {
        let tiles = self.indices(interactions).len().max(1);
        let at = self.selected(interactions, current);
        self.select(interactions, (at + 1) % tiles);
    }

    /// The previous tile, wrapping from the first to the last.
    pub fn prev(&mut self, interactions: &LiveInteractions, current: &str) {
        let tiles = self.indices(interactions).len().max(1);
        let at = self.selected(interactions, current);
        self.select(interactions, (at + tiles - 1) % tiles);
    }

    pub fn first(&mut self, interactions: &LiveInteractions) {
        self.select(interactions, 0);
    }

    pub fn last(&mut self, interactions: &LiveInteractions) {
        self.select(interactions, usize::MAX);
    }

    /// Put the cursor on tile `position`, or on the last tile past the end.
    fn select(&mut self, interactions: &LiveInteractions, position: usize) {
        let tiles = self.indices(interactions);
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
            .overview_indices()
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
        assert_eq!(ids(&interactions), ["100-tile", "97-tile", "98-tile"]);
    }

    #[test]
    fn working_tiles_precede_idle_tiles_in_existing_order() {
        let interactions = fleet(&[
            InteractionActivity::Pending,
            InteractionActivity::Background,
            InteractionActivity::Running,
            InteractionActivity::Pending,
        ]);
        assert_eq!(
            ids(&interactions),
            ["99-tile", "98-tile", "100-tile", "97-tile"]
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
        assert_eq!(overview.indices(&interactions).len(), 2);
        assert_eq!(
            overview.selected_id(&interactions, current),
            Some("99-tile")
        );
        overview.next(&interactions, current);
        assert_eq!(
            overview.selected_id(&interactions, current),
            Some("98-tile")
        );
        overview.next(&interactions, current);
        assert_eq!(
            overview.selected_id(&interactions, current),
            Some("99-tile")
        );

        for item in &mut interactions.items {
            item.activity = InteractionActivity::Pending;
        }
        assert!(overview.indices(&interactions).is_empty());
        assert_eq!(overview.selected_id(&interactions, current), None);
        overview.next(&interactions, current);
        overview.prev(&interactions, current);
        overview.toggle_running_only();
        assert_eq!(overview.indices(&interactions).len(), 3);
        assert_eq!(
            overview.selected_id(&interactions, current),
            Some("99-tile")
        );
    }

    #[test]
    fn the_cursor_starts_on_the_interaction_on_screen() {
        let interactions = fleet(&[InteractionActivity::Pending; 3]);
        let mut overview = Overview::default();
        overview.open();
        assert_eq!(overview.selected(&interactions, "99-tile"), 1);
        assert_eq!(overview.selected(&interactions, "elsewhere"), 0);
    }

    /// Five tiles three over two: down and up land on the tile under the
    /// middle of the one the cursor leaves, the bottom row's being wider.
    #[test]
    fn the_cursor_moves_across_and_down_the_grid() {
        let interactions = fleet(&[InteractionActivity::Pending; 5]);
        let mut overview = Overview::default();
        overview.note_rows(&[3, 2]);
        let current = "100-tile";

        overview.down(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 3);
        overview.up(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 0);

        overview.right(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 1);
        overview.down(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 4);
        overview.down(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 4, "no row below");
        overview.up(&interactions, current);
        assert_eq!(
            overview.selected(&interactions, current),
            2,
            "the right half of the top row"
        );
        overview.up(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 2, "no row above");
        overview.down(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 4);
        overview.right(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 4, "at the end");

        overview.first(&interactions);
        assert_eq!(
            overview.selected_id(&interactions, current),
            Some("100-tile")
        );
        overview.last(&interactions);
        assert_eq!(
            overview.selected_id(&interactions, current),
            Some("96-tile")
        );
    }

    #[test]
    fn tab_walks_the_tiles_in_order_and_wraps() {
        let interactions = fleet(&[InteractionActivity::Pending; 3]);
        let mut overview = Overview::default();
        overview.note_rows(&[2, 1]);
        let current = "99-tile";

        overview.next(&interactions, current);
        assert_eq!(
            overview.selected(&interactions, current),
            2,
            "across the row break"
        );
        overview.next(&interactions, current);
        assert_eq!(
            overview.selected(&interactions, current),
            0,
            "wraps to the first"
        );
        overview.prev(&interactions, current);
        assert_eq!(
            overview.selected(&interactions, current),
            2,
            "wraps to the last"
        );
        overview.prev(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 1);
    }
}
