//! Styra-wire and provider-native raw record presentation.

use crate::chrome::PanelChrome;
use styra_protocol::RawLine;

pub enum RawSource<'a> {
    Styra(&'a [RawLine]),
    Received(&'a [RawLine]),
    Provider(&'a [String]),
}

pub struct RawView<'a> {
    pub chrome: PanelChrome,
    pub source: RawSource<'a>,
    pub selected: usize,
    pub requested_preview_scroll: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawFeedback {
    pub preview_limit: u16,
    pub effective_preview_scroll: u16,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/raw.rs"]
mod rendering;
