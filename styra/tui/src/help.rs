//! The full-screen keyboard reference, and how far into it the operator has
//! read.
//!
//! Held apart from [`App`](crate::app::App) for the same reason as
//! [`Preview`](crate::preview::Preview): two fields with a rule between them.
//! Closing the reference returns it to the top, so reopening it starts where
//! it reads from rather than wherever it was last left — a section the
//! operator scrolled to once is not what they want next time they press `?`.
//!
//! That rule lived in the event loop, which closed the overlay and zeroed the
//! offset as two statements, and reset the offset again by hand for `g`.

use styra_ui::scroll::Scroll;

/// Whether the reference is showing, and where in it.
#[derive(Default)]
pub struct Help {
    open: bool,
    /// The reference is taller than a short terminal, so without this the
    /// sections at the end are unreachable rather than merely below the fold.
    scroll: Scroll,
}

impl Help {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    /// Close the reference, so the next `?` starts from the top.
    pub fn close(&mut self) {
        self.open = false;
        self.scroll.reset();
    }

    /// The offset to render at, held to what the reference can actually
    /// scroll to at this height.
    pub fn offset(&self) -> u16 {
        self.scroll.clamped()
    }

    /// Record the furthest the renderer can scroll, which only it knows.
    pub fn note_limit(&mut self, limit: u16) {
        self.scroll.note_limit(limit);
    }

    pub fn apply_feedback(&mut self, limit: u16, effective_offset: u16) {
        self.scroll.offset = effective_offset;
        self.scroll.note_limit(limit);
    }

    pub fn line_down(&mut self) {
        self.scroll.line_down();
    }

    pub fn line_up(&mut self) {
        self.scroll.line_up();
    }

    pub fn page_down(&mut self) {
        self.scroll.page_down();
    }

    pub fn page_up(&mut self) {
        self.scroll.page_up();
    }

    /// Jump back to the start of the reference.
    pub fn scroll_to_top(&mut self) {
        self.scroll.reset();
    }
}

#[cfg(test)]
mod tests;
