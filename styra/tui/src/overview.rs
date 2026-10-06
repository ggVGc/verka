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
    /// How many tiles a row of the grid held when it was last drawn, which is
    /// what moving up or down a column steps over. The renderer decides it
    /// from the terminal's width, so it is learned from the draw.
    columns: usize,
}

impl Overview {
    /// Start from the interaction on screen, wherever the cursor was left.
    pub fn open(&mut self) {
        self.cursor = None;
    }

    pub fn note_columns(&mut self, columns: usize) {
        self.columns = columns;
    }

    /// The cursor's position in [`LiveInteractions::overview_indices`]. A
    /// cursor whose interaction has stopped since, and so left the grid,
    /// falls back as an unmoved one does.
    pub fn selected(&self, interactions: &LiveInteractions, current: &str) -> usize {
        let tiles = interactions.overview_indices();
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
        let tiles = interactions.overview_indices();
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

    /// Down a column. From the row above a short last row, where there is no
    /// tile straight below, the cursor drops onto that row's last tile rather
    /// than refusing to leave.
    pub fn down(&mut self, interactions: &LiveInteractions, current: &str) {
        let columns = self.columns.max(1);
        let at = self.selected(interactions, current);
        let last = interactions.overview_indices().len().saturating_sub(1);
        if at / columns < last / columns {
            self.select(interactions, at + columns);
        }
    }

    pub fn up(&mut self, interactions: &LiveInteractions, current: &str) {
        let columns = self.columns.max(1);
        let at = self.selected(interactions, current);
        if at >= columns {
            self.select(interactions, at - columns);
        }
    }

    pub fn first(&mut self, interactions: &LiveInteractions) {
        self.select(interactions, 0);
    }

    pub fn last(&mut self, interactions: &LiveInteractions) {
        self.select(interactions, usize::MAX);
    }

    /// Put the cursor on tile `position`, or on the last tile past the end.
    fn select(&mut self, interactions: &LiveInteractions, position: usize) {
        let tiles = interactions.overview_indices();
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
        assert_eq!(ids(&interactions), ["100-tile", "98-tile", "97-tile"]);
    }

    #[test]
    fn the_cursor_starts_on_the_interaction_on_screen() {
        let interactions = fleet(&[InteractionActivity::Pending; 3]);
        let mut overview = Overview::default();
        overview.open();
        assert_eq!(overview.selected(&interactions, "99-tile"), 1);
        assert_eq!(overview.selected(&interactions, "elsewhere"), 0);
    }

    /// Five tiles three to a row: the second row is short, and down from
    /// above its gap lands on its last tile.
    #[test]
    fn the_cursor_moves_across_and_down_the_grid() {
        let interactions = fleet(&[InteractionActivity::Pending; 5]);
        let mut overview = Overview::default();
        overview.note_columns(3);
        let current = "100-tile";

        overview.right(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 1);
        overview.down(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 4);
        overview.down(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 4, "no row below");
        overview.up(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 1);
        overview.up(&interactions, current);
        assert_eq!(overview.selected(&interactions, current), 1, "no row above");

        overview.right(&interactions, current);
        overview.down(&interactions, current);
        assert_eq!(
            overview.selected(&interactions, current),
            4,
            "nothing straight below the third tile"
        );
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
}
