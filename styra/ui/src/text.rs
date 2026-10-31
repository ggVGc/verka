//! Terminal-column measurements and styled text layout shared by UI panes.
//!
//! Cuts happen at grapheme boundaries, so combining marks and joined emoji
//! stay together. Widths follow the same Unicode rules as Ratatui.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// The longest prefix that fits, without splitting a displayed grapheme.
pub fn prefix(text: &str, columns: usize) -> &str {
    let mut used = 0;
    for (index, grapheme) in text.grapheme_indices(true) {
        used += width(grapheme);
        if used > columns {
            return &text[..index];
        }
    }
    text
}

/// The longest suffix that fits, without splitting a displayed grapheme.
pub fn suffix(text: &str, columns: usize) -> &str {
    let mut used = 0;
    for (index, grapheme) in text.grapheme_indices(true).rev() {
        used += width(grapheme);
        if used > columns {
            return &text[index + grapheme.len()..];
        }
    }
    text
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/text.rs"]
mod rendering;
