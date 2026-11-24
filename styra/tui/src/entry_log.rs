//! The entry-log pane's state: whether it is open, whether it is the window
//! taking the navigation keys, where its cursor is, and how far it is
//! scrolled.
//!
//! The pane opens as a follower of the event list, but it is also a list in
//! its own right: with the conversation-only filter on it holds every entry
//! the list is hiding, and those are the entries an operator wants to read one
//! at a time. `Tab` hands the navigation keys to it and back, so the two
//! windows are read with the same keys without either losing its place.
//!
//! While the pane is open its cursor is what the preview shows, whichever
//! window holds the keys: the pane is the finer-grained of the two lists, so
//! an open one is what the operator is reading, and a preview that switched
//! panes on `Tab` would move under them. An unfocused pane keeps its cursor on
//! the list's selection, so the pair still reads as one.
//!
//! The cursor is held as an offset into the selected message's stretch rather
//! than as an index into the timeline: the stretch is recomputed from the list
//! selection on every draw, and an absolute index would point somewhere else
//! as soon as an event arrived ahead of it.

use styra_ui::scroll::Scroll;

/// Entries the cursor moves by one PageUp/PageDown press, matching the lines
/// [`Scroll`] pages by: the rows here are one line each.
const PAGE: usize = 10;

#[derive(Default)]
pub struct EntryLog {
    /// Whether the pane is open below the event list.
    pub open: bool,
    /// How far through the scoped entries the pane is scrolled.
    pub scroll: Scroll,
    /// Whether the pane, rather than the event list, is taking the navigation
    /// keys. Only meaningful while the pane is open; closing it hands the keys
    /// back, so this never outlives the window it belongs to.
    focused: bool,
    /// The cursor, as an offset from the start of the scoped stretch. Held
    /// unclamped: the stretch grows and shrinks under it as events arrive, so
    /// the length is only known where it is read (see [`Self::cursor`]).
    cursor: usize,
}

impl EntryLog {
    /// Open or close the pane. Either way the event list gets the navigation
    /// keys back: a closed pane cannot hold them, and a freshly opened one
    /// starts as the follower it is described as.
    ///
    /// Opening it puts the cursor on the last of the `len` entries it is about
    /// to show: the newest entry is the one an operator opening the log wants
    /// to read, and the preview beside it starts there too.
    pub fn toggle(&mut self, len: usize) {
        self.open = !self.open;
        self.focused = false;
        self.cursor = len.saturating_sub(1);
        self.scroll.reset();
    }

    /// Whether the pane is the window the navigation keys act on.
    pub fn focused(&self) -> bool {
        self.open && self.focused
    }

    /// Hand the navigation keys to the pane. The cursor stays where it is —
    /// an unfocused pane's cursor is already the one the preview is reading —
    /// so `Tab` moves the focus without moving what the preview is showing.
    pub fn focus(&mut self) {
        self.focused = true;
    }

    /// Hand the navigation keys back to the event list.
    pub fn unfocus(&mut self) {
        self.focused = false;
    }

    /// The cursor, held to a stretch of `len` entries. `None` for an empty
    /// stretch, which has nothing to point at.
    pub fn cursor(&self, len: usize) -> Option<usize> {
        (len > 0).then(|| self.cursor.min(len - 1))
    }

    /// Follow the event list's selection: a new stretch, so the scroll offset
    /// taken against the old one means nothing, and the cursor takes `cursor`
    /// — where that selection sits in the new stretch. The preview reads this
    /// cursor whenever the pane is open, so leaving it behind would show an
    /// entry from the stretch the operator just left.
    ///
    /// Does nothing while the pane holds the keys, since then it is the pane's
    /// own cursor that is being moved and the list is standing still.
    pub fn follow_list(&mut self, cursor: usize) {
        if !self.focused() {
            self.cursor = cursor;
            self.scroll.reset();
        }
    }

    pub fn select_next(&mut self, len: usize) {
        if let Some(cursor) = self.cursor(len) {
            self.cursor = (cursor + 1).min(len - 1);
        }
    }

    pub fn select_prev(&mut self, len: usize) {
        if let Some(cursor) = self.cursor(len) {
            self.cursor = cursor.saturating_sub(1);
        }
    }

    /// Move the cursor a page — the same step the scroll offset takes — so a
    /// long stretch can be crossed without holding a movement key down.
    pub fn page_down(&mut self, len: usize) {
        if let Some(cursor) = self.cursor(len) {
            self.cursor = cursor.saturating_add(PAGE).min(len - 1);
        }
    }

    pub fn page_up(&mut self, len: usize) {
        if let Some(cursor) = self.cursor(len) {
            self.cursor = cursor.saturating_sub(PAGE);
        }
    }

    pub fn select_first(&mut self) {
        self.cursor = 0;
    }

    pub fn select_last(&mut self, len: usize) {
        self.cursor = len.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests;
