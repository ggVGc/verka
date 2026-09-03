//! The verbatim wire interaction and the operator's place in it.
//!
//! Held apart from [`App`](crate::app::App) because the whole of it is one
//! rule: the selection tracks the tail until the operator moves it, and
//! resumes tracking when they return to it. That rule was previously
//! re-established at six separate call sites over four public fields, any of
//! which could leave the selection following a line that is no longer the
//! last one. Here it is stated once, and the fields it holds are private.
//!
//! [`crate::ui::raw`] renders it.

use styra_server::RawLine;

use crate::app::Scroll;

/// The wire lines, in occurrence order, and which one is selected.
pub struct RawView {
    lines: Vec<RawLine>,
    /// Which wire line the view has selected. Always a valid index into
    /// `lines` while there is one to be valid.
    selected: usize,
    /// When true, `selected` tracks the newest line as it arrives.
    follow: bool,
    /// How far the selected line's pretty-printed preview is scrolled.
    pub preview: Scroll,
}

impl Default for RawView {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            selected: 0,
            // A screen with no lines yet is at its tail by definition, so the
            // first line to arrive is the one shown.
            follow: true,
            preview: Scroll::default(),
        }
    }
}

impl RawView {
    /// Append a wire line. When the operator has selected a line explicitly,
    /// the view stays pinned to it; otherwise the selection tracks the new
    /// tail.
    pub fn push(&mut self, line: RawLine) {
        self.lines.push(line);
        if self.follow {
            self.selected = self.lines.len() - 1;
            self.preview.reset();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&RawLine> {
        self.lines.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RawLine> {
        self.lines.iter()
    }

    /// The index the line pushed next will land on, for an event that wants to
    /// name the wire line it was decoded from.
    pub fn last_index(&self) -> Option<usize> {
        self.lines.len().checked_sub(1)
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> Option<&RawLine> {
        self.lines.get(self.selected)
    }

    /// Whether the selection is tracking the tail. Only the module's own
    /// tests ask directly; everything else observes it through where a pushed
    /// line leaves the selection.
    #[cfg(test)]
    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Open the view on `line`, or on the tail when there is no particular
    /// line to open on. Focusing a line pins the view; falling back to the
    /// tail resumes following it.
    pub fn enter(&mut self, line: Option<usize>) {
        self.preview.reset();
        match line {
            Some(index) if !self.lines.is_empty() => {
                self.selected = index.min(self.lines.len() - 1);
                self.follow = false;
            }
            _ => {
                self.selected = self.lines.len().saturating_sub(1);
                self.follow = true;
            }
        }
    }

    /// Move the selection to the next wire line.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.lines.len() {
            self.selected += 1;
            self.preview.reset();
        }
        // Re-enable follow only when the selection reaches the tail.
        self.follow = !self.lines.is_empty() && self.selected + 1 >= self.lines.len();
    }

    /// Move the selection to the previous wire line.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.preview.reset();
        }
        // Moving off the tail pins the view.
        self.follow = false;
    }

    pub fn select_first(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        self.selected = 0;
        self.preview.reset();
        self.follow = false;
    }

    pub fn select_last(&mut self) {
        if self.lines.is_empty() {
            return;
        }
        self.selected = self.lines.len() - 1;
        self.preview.reset();
        self.follow = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_server::Direction;

    fn line(text: &str) -> RawLine {
        RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: text.into(),
        }
    }

    #[test]
    fn the_selection_tracks_the_tail_until_the_operator_moves_it() {
        let mut raw = RawView::default();
        for i in 0..5 {
            raw.push(line(&format!("line {i}")));
        }
        assert_eq!(raw.selected_index(), 4, "starts pinned to the tail");
        assert!(raw.is_following());

        raw.select_prev();
        assert_eq!(raw.selected_index(), 3);
        assert!(!raw.is_following());

        // A new line while a specific line is selected keeps that same line
        // in view rather than yanking to the new tail.
        raw.push(line("new"));
        assert_eq!(raw.selected_index(), 3);

        raw.select_last();
        assert_eq!(raw.selected_index(), 5);
        assert!(raw.is_following());
        raw.select_first();
        assert_eq!(raw.selected_index(), 0);
        assert!(!raw.is_following());
    }

    /// Stepping back down to the last line means the operator has caught up
    /// with the stream, so the view resumes tracking it.
    #[test]
    fn reaching_the_tail_again_resumes_following() {
        let mut raw = RawView::default();
        raw.push(line("one"));
        raw.push(line("two"));
        raw.select_prev();
        assert!(!raw.is_following());

        raw.select_next();

        assert_eq!(raw.selected_index(), 1);
        assert!(raw.is_following());
    }

    #[test]
    fn entering_on_a_line_pins_the_view_and_entering_on_nothing_follows() {
        let mut raw = RawView::default();
        for i in 0..3 {
            raw.push(line(&format!("line {i}")));
        }

        raw.enter(Some(1));
        assert_eq!(raw.selected_index(), 1);
        assert!(!raw.is_following());

        raw.enter(None);
        assert_eq!(raw.selected_index(), 2);
        assert!(raw.is_following());
    }

    /// A line index recorded before a lazy attach dropped the history would
    /// otherwise point past the end of it.
    #[test]
    fn entering_on_a_line_that_is_no_longer_there_lands_on_the_last_one() {
        let mut raw = RawView::default();
        raw.push(line("only"));

        raw.enter(Some(7));

        assert_eq!(raw.selected_index(), 0);
    }

    #[test]
    fn entering_an_empty_view_selects_nothing_and_follows() {
        let mut raw = RawView::default();

        raw.enter(Some(3));

        assert_eq!(raw.selected_index(), 0);
        assert!(raw.is_following());
        assert!(raw.selected().is_none());
    }

    #[test]
    fn navigating_an_empty_view_does_nothing() {
        let mut raw = RawView::default();

        raw.select_first();
        raw.select_last();
        raw.select_prev();

        assert_eq!(raw.selected_index(), 0);
        assert!(raw.is_empty());
    }
}
