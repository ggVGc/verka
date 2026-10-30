//! Filtered plain-text transcript presentation.

use crate::chrome::PanelChrome;

pub struct TranscriptView<'a> {
    pub chrome: PanelChrome,
    /// Owned by the adapter because this is the same derived text used for
    /// clipboard and export behavior; the renderer only borrows it.
    pub text: &'a str,
    pub has_entries: bool,
    /// See [`all_events_title`].
    pub all_events: bool,
    pub show_minor: bool,
    /// See [`crate::chrome::uncommitted_title`].
    pub uncommitted_changes: bool,
    pub requested_scroll: u16,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/transcript.rs"]
mod rendering;
