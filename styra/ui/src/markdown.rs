//! Markdown-to-Ratatui styling for agent messages.
//!
//! Everything is rendered by `tui-markdown`, which parses a whole buffer at
//! once and so can render tables, code fences, and other multi-line
//! constructs correctly. A single-line summary is the same rendering with its
//! lines laid end to end, so a row and its expanded body never disagree.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkDisplay {
    Compact,
    Full,
}
/// Which entry of a rendered block, if any, carries the selection.
///
/// Entries are numbered in reading order across the whole block, starting at
/// zero, so a caller that keeps a `usize` and bounds it by
/// [`BlockRender::entries`] can walk a response's citations without knowing
/// anything about how they were laid out.
pub type EntryIndex = usize;

/// How many Markdown links a block contains, in reading order.
///
/// This is the same numbering used by [`markdown_block_render`], allowing a
/// caller to keep one selection while the links live in separate messages.
pub fn markdown_link_count(text: &str) -> usize {
    link_destinations(text).count()
}

/// Destination of the `index`th entry, in the same reading order as
/// [`markdown_link_count`] and [`markdown_block_render`]: a link's
/// destination, or the text of a code span that names a file.
pub fn markdown_link_destination(text: &str, index: EntryIndex) -> Option<String> {
    link_destinations(text).nth(index)
}

fn link_destinations(text: &str) -> impl Iterator<Item = String> + '_ {
    let mut depth = 0usize;
    Parser::new_ext(text, Options::all()).filter_map(move |event| match event {
        Event::Start(Tag::Link { dest_url, .. }) => {
            depth += 1;
            Some(dest_url.into_string())
        }
        Event::End(TagEnd::Link) => {
            depth = depth.saturating_sub(1);
            None
        }
        // A code span inside a link is its label; the link is the entry.
        Event::Code(code) if depth == 0 && is_file_reference(&code) => Some(code.into_string()),
        _ => None,
    })
}

/// Whether an inline code span reads as a file reference — `src/app.rs`,
/// `app.rs:120`, `lib/mod.rs:7:3` — rather than code.
///
/// Agents cite files in backticks as often as they write links, and a
/// citation should be selectable however it was written. What rules out code
/// is its punctuation: a call, a path in `::`, a URL, or anything with a space
/// is not a filename. A dotted name with no slash must end in a short,
/// letter-led extension, so `Cargo.toml` is a file while `1.2.3` and
/// `config.default_model` are not. A short field access such as `self.spec`
/// still passes; selecting it only opens a file that does not exist.
pub fn is_file_reference(code: &str) -> bool {
    let mut path = code;
    // At most `:line:column`, the same suffix link destinations may carry.
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((before, digits))
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
            {
                path = before;
            }
            _ => break,
        }
    }
    if path.is_empty()
        || !path.chars().any(|ch| ch.is_ascii_alphabetic())
        || !path.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '~' | '+' | '@')
        })
    {
        return false;
    }
    if path.contains('/') {
        return true;
    }
    // A dotfile (`.gitignore`) is all extension, so its length says nothing.
    path.rsplit_once('.').is_some_and(|(stem, extension)| {
        (stem.is_empty() || (1..=5).contains(&extension.len()))
            && !extension.is_empty()
            && extension.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    })
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/markdown.rs"]
mod rendering;
