//! The panel that shows one entry's full content, and the three choices that
//! decide what it shows and how.
//!
//! Held apart from [`App`](crate::app::App) because the three move together:
//! changing the presentation or the target changes what is on screen, so the
//! scroll offset taken against the old content no longer means anything and
//! has to go back to the top. That reset was previously the caller's to
//! remember at each of the two places that could change either, with the
//! offset a public field any of them could have set instead.
//!
//! Which entry the panel is *pointed at* is not here: that is a question about
//! the timeline, and only [`App`](crate::app::App) has both.
//! [`crate::ui::preview`] renders it.

use styra_server::event::PresentationMode;

use crate::app::Scroll;

/// Which entry the preview panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewTarget {
    /// The entry the list selection is on.
    #[default]
    Selection,
    /// The newest shell command and its result, regardless of where the
    /// selection currently is — so the preview keeps showing what the agent
    /// is running while the operator reads elsewhere in the list.
    Command,
}

/// The panel's display choices, which belong to the operator rather than to
/// the Interaction on screen and so outlive it; see
/// [`OperatorState`](crate::app::OperatorState).
///
/// The scroll offset is deliberately not among them: it was taken against
/// another screen's content.
#[derive(Clone, Copy, Default)]
pub struct Choices {
    open: bool,
    mode: PresentationMode,
    target: PreviewTarget,
}

/// Whether the panel is open, what it is pointed at, and how far into it the
/// operator has read.
#[derive(Default)]
pub struct Preview {
    /// When true, a side panel shows the full expanded content of the
    /// previewed entry, independent of whether it is folded in the list.
    pub open: bool,
    /// How far the previewed entry is scrolled.
    pub scroll: Scroll,
    mode: PresentationMode,
    target: PreviewTarget,
}

impl Preview {
    pub fn mode(&self) -> PresentationMode {
        self.mode
    }

    pub fn target(&self) -> PreviewTarget {
        self.target
    }

    /// Whether the panel is pointed at the newest command rather than at the
    /// list selection.
    pub fn follows_command(&self) -> bool {
        self.target == PreviewTarget::Command
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Open the panel, for the layouts that come with it already showing.
    pub fn show(&mut self) {
        self.open = true;
    }

    /// Switch between the concise presentation and the complete decoded one.
    /// The content changes, so the offset taken against the old one is
    /// meaningless and the panel returns to the top.
    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            PresentationMode::Pretty => PresentationMode::Raw,
            PresentationMode::Raw => PresentationMode::Pretty,
        };
        self.scroll.reset();
    }

    /// Switch between following the list selection and following the newest
    /// command. A different entry, so again from the top.
    pub fn toggle_target(&mut self) {
        self.target = match self.target {
            PreviewTarget::Selection => PreviewTarget::Command,
            PreviewTarget::Command => PreviewTarget::Selection,
        };
        self.scroll.reset();
    }

    /// The choices that outlive this screen.
    pub fn choices(&self) -> Choices {
        Choices {
            open: self.open,
            mode: self.mode,
            target: self.target,
        }
    }

    /// Adopt the presentation choices made on a previous screen. The offset is
    /// not adopted with them: it was taken against that screen's content. See
    /// [`crate::app::OperatorState`].
    pub fn adopt(&mut self, choices: Choices) {
        self.open = choices.open;
        self.mode = choices.mode;
        self.target = choices.target;
        self.scroll.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offset counts lines of whatever is currently rendered. Changing
    /// what that is without resetting it would leave the panel scrolled to an
    /// arbitrary point in content the operator has not seen.
    #[test]
    fn changing_what_is_shown_returns_to_the_top() {
        let mut preview = Preview::default();
        preview.scroll.note_limit(100);
        preview.scroll.page_down();
        assert!(preview.scroll.offset > 0);

        preview.toggle_mode();
        assert_eq!(preview.scroll.offset, 0);

        preview.scroll.page_down();
        assert!(preview.scroll.offset > 0);

        preview.toggle_target();
        assert_eq!(preview.scroll.offset, 0);
    }

    #[test]
    fn the_presentation_and_the_target_toggle_independently() {
        let mut preview = Preview::default();
        assert_eq!(preview.mode(), PresentationMode::Pretty);
        assert_eq!(preview.target(), PreviewTarget::Selection);

        preview.toggle_mode();

        assert_eq!(preview.mode(), PresentationMode::Raw);
        assert_eq!(
            preview.target(),
            PreviewTarget::Selection,
            "the target is a separate choice"
        );

        preview.toggle_target();
        preview.toggle_mode();

        assert_eq!(preview.mode(), PresentationMode::Pretty);
        assert!(preview.follows_command());
    }

    /// Opening and closing the panel does not change what it would show, so an
    /// operator who closes it and opens it again gets what they had.
    #[test]
    fn opening_the_panel_leaves_the_presentation_choices_alone() {
        let mut preview = Preview::default();
        preview.toggle_mode();
        preview.toggle_target();

        preview.toggle();
        preview.toggle();

        assert!(!preview.open);
        assert_eq!(preview.mode(), PresentationMode::Raw);
        assert!(preview.follows_command());
    }

    #[test]
    fn adopting_a_previous_screens_choices_starts_its_content_from_the_top() {
        let mut preview = Preview::default();
        preview.scroll.note_limit(100);
        preview.scroll.page_down();

        let mut chosen = Preview::default();
        chosen.show();
        chosen.toggle_mode();
        chosen.toggle_target();
        preview.adopt(chosen.choices());

        assert!(preview.open);
        assert_eq!(preview.mode(), PresentationMode::Raw);
        assert!(preview.follows_command());
        assert_eq!(preview.scroll.offset, 0, "a different screen's content");
    }
}
