//! How far a scrolling panel is scrolled, kept by whoever drives the panel.

/// How far a preview panel is scrolled, and the last maximum offset its
/// renderer calculated.
///
/// The limit has to come back from rendering because wrapping depends on the
/// terminal width, which only the renderer knows. Holding onto it stops
/// repeated PageDown presses at the bottom from accumulating an invisible
/// offset that PageUp would later have to unwind.
#[derive(Default)]
pub struct Scroll {
    /// Lines scrolled down from the top. Only ever shown through
    /// [`Scroll::clamped`], so it may sit past what currently fits.
    pub offset: u16,
    limit: u16,
    /// How many lines the renderer last showed at once; zero until it says.
    viewport: u16,
}

/// Lines moved by one PageUp/PageDown press.
const SCROLL_PAGE: u16 = 10;

impl Scroll {
    /// The offset to render at: what was asked for, held to what actually fits.
    pub fn clamped(&self) -> u16 {
        self.offset.min(self.limit)
    }

    /// Record the furthest the renderer can actually scroll at this width.
    pub fn note_limit(&mut self, limit: u16) {
        self.limit = limit;
    }

    /// Record how many lines the renderer shows at once, which is what a
    /// half-page step is half of.
    pub fn note_viewport(&mut self, viewport: u16) {
        self.viewport = viewport;
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }

    /// Jump past the true end; [`Self::clamped`] brings it back to the last
    /// page, so the exact rendered line count need not be known here.
    pub fn scroll_to_end(&mut self) {
        self.offset = u16::MAX;
    }

    pub fn page_down(&mut self) {
        self.offset = self.clamped().saturating_add(SCROLL_PAGE).min(self.limit);
    }

    pub fn page_up(&mut self) {
        self.offset = self.clamped().saturating_sub(SCROLL_PAGE);
    }

    /// Scroll by half of what is on screen, so the lines read last stay in
    /// view. Before the renderer has said how tall that is, by a page.
    pub fn half_page_down(&mut self) {
        let step = self.half_page();
        self.offset = self.clamped().saturating_add(step).min(self.limit);
    }

    pub fn half_page_up(&mut self) {
        let step = self.half_page();
        self.offset = self.clamped().saturating_sub(step);
    }

    fn half_page(&self) -> u16 {
        match self.viewport {
            0 => SCROLL_PAGE,
            viewport => (viewport / 2).max(1),
        }
    }

    pub fn line_down(&mut self) {
        self.offset = self.clamped().saturating_add(1).min(self.limit);
    }

    pub fn line_up(&mut self) {
        self.offset = self.clamped().saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Half a page is half of what the renderer last showed, and a page
    /// until it has said how much that is.
    #[test]
    fn half_a_page_is_half_the_viewport() {
        let mut scroll = Scroll::default();
        scroll.note_limit(100);
        scroll.half_page_down();
        assert_eq!(scroll.offset, SCROLL_PAGE, "no viewport reported yet");

        scroll.note_viewport(9);
        scroll.half_page_down();
        assert_eq!(scroll.offset, SCROLL_PAGE + 4);
        scroll.half_page_up();
        assert_eq!(scroll.offset, SCROLL_PAGE);

        scroll.note_viewport(1);
        scroll.half_page_down();
        assert_eq!(scroll.offset, SCROLL_PAGE + 1, "always at least a line");
    }
}
