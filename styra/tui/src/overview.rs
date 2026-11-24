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
    pub layout: styra_ui::overview::OverviewLayout,
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

    pub fn toggle_layout(&mut self) {
        use styra_ui::overview::OverviewLayout;
        self.layout = match self.layout {
            OverviewLayout::Auto => OverviewLayout::TwoColumns,
            OverviewLayout::TwoColumns => OverviewLayout::Auto,
        };
        self.rows.clear();
    }

    /// The visible tiles, shared by drawing and cursor navigation. Running
    /// only still keeps what needs attention: newly idle work, and `current`,
    /// the interaction on screen.
    pub fn indices(
        &self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Vec<usize> {
        interactions
            .overview_indices(self.scope(workspace_id))
            .into_iter()
            .filter(|index| {
                let interaction = &interactions.items[*index];
                !self.running_only
                    || interaction.id == current
                    || matches!(
                        interaction.activity,
                        styra_protocol::InteractionActivity::Running
                            | styra_protocol::InteractionActivity::Background
                    )
                    || (interaction.activity == styra_protocol::InteractionActivity::Pending
                        && interaction.idle_unseen)
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
        let tiles = self.indices(interactions, current, workspace_id);
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
        let tiles = self.indices(interactions, current, workspace_id);
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
        interactions.next_attention_or_live_in_order(
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
        self.select(interactions, at.saturating_sub(1), current, workspace_id);
    }

    pub fn right(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let at = self.selected(interactions, current, workspace_id);
        self.select(interactions, at + 1, current, workspace_id);
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
            self.select(interactions, position, current, workspace_id);
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
            self.select(interactions, position, current, workspace_id);
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
        let target_column = if self.layout == styra_ui::overview::OverviewLayout::TwoColumns {
            column.min(target_length - 1)
        } else {
            ((2 * column + 1) * target_length / (2 * length)).min(target_length - 1)
        };
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
        let tiles = self
            .indices(interactions, current, workspace_id)
            .len()
            .max(1);
        let at = self.selected(interactions, current, workspace_id);
        self.select(interactions, (at + 1) % tiles, current, workspace_id);
    }

    /// The previous tile, wrapping from the first to the last.
    pub fn prev(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let tiles = self
            .indices(interactions, current, workspace_id)
            .len()
            .max(1);
        let at = self.selected(interactions, current, workspace_id);
        self.select(
            interactions,
            (at + tiles - 1) % tiles,
            current,
            workspace_id,
        );
    }

    pub fn first(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        self.select(interactions, 0, current, workspace_id);
    }

    pub fn last(
        &mut self,
        interactions: &LiveInteractions,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        self.select(interactions, usize::MAX, current, workspace_id);
    }

    /// Onto tile `position` in reading order, counted from zero. A position
    /// past the last tile leaves the cursor where it is, so a count that
    /// misses does not land somewhere unasked for.
    pub fn nth(
        &mut self,
        interactions: &LiveInteractions,
        position: usize,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        if position < self.indices(interactions, current, workspace_id).len() {
            self.select(interactions, position, current, workspace_id);
        }
    }

    /// Put the cursor on the first of `target`'s tiles, for the Workspace
    /// jump. Reports whether `target` has a tile to land on.
    pub fn select_workspace(
        &mut self,
        interactions: &LiveInteractions,
        target: &str,
        current: &str,
        workspace_id: Option<&str>,
    ) -> bool {
        let Some(index) = self
            .indices(interactions, current, workspace_id)
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
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let tiles = self.indices(interactions, current, workspace_id);
        if let Some(index) = tiles.get(position).or(tiles.last()) {
            self.cursor = Some(interactions.items[*index].id.clone());
        }
    }
}

#[cfg(test)]
mod tests;
