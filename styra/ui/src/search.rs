//! The `/` search: what has been typed, who holds the keys, and highlighting
//! the words it matches.
//!
//! The search only marks what is already on screen — it moves neither the
//! selection nor the viewport — so it is a display choice like the filters
//! beside it rather than a way to navigate. Highlighting starts once enough
//! has been typed to be looking for something; see [`MIN_TERM`].
//!
//! The search marks whole words rather than the matched letters alone: what
//! the operator is looking for in a log is a path, an identifier, or a message
//! — `retry` should point at `retry_backoff` and `src/retry.rs` entire, so the
//! thing found can be read off the screen without hunting for where the match
//! ends.

use crate::query::Edit;

/// How much has to be typed before anything is highlighted.
///
/// One or two letters match most of a log at once, which highlights so much
/// that nothing stands out — and the list would flash on the way to every
/// term. Three is enough to be looking for something.
pub const MIN_TERM: usize = 3;

/// The term being typed or standing, kept by whoever drives the view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    query: String,
    typing: bool,
}

impl Search {
    /// Start typing a new term. An earlier one is dropped: `/` asks what to
    /// look for, and the answer to that is not the last question's.
    pub fn open(&mut self) {
        self.query.clear();
        self.typing = true;
    }

    /// Whether the prompt has the keys, and so every printable one is part of
    /// the term rather than a command on the list underneath.
    pub fn typing(&self) -> bool {
        self.typing
    }

    /// What has been typed, while the prompt is open or a search stands.
    pub fn query(&self) -> Option<&str> {
        (self.typing || !self.query.is_empty()).then_some(self.query.as_str())
    }

    pub fn push(&mut self, character: char) {
        self.query.push(character);
    }

    /// Delete the last character, closing the prompt once nothing is left: a
    /// search backspaced away is a search abandoned.
    pub fn backspace(&mut self) {
        self.query.pop();
        if self.query.is_empty() {
            self.typing = false;
        }
    }

    /// Make a change typed into the prompt, as every search field takes it
    /// (see [`crate::query`]). Deleting a word leaves the prompt open even
    /// when it was the only one: it is taken back to be typed again, where
    /// backspacing a search away abandons it.
    pub fn edit(&mut self, edit: Edit) {
        match edit {
            Edit::Backspace => self.backspace(),
            edit => edit.apply(&mut self.query),
        }
    }

    /// Hand the keys back to the list, leaving the term marked.
    pub fn accept(&mut self) {
        self.typing = false;
    }

    /// Leave the list as it was, unmarked.
    pub fn cancel(&mut self) {
        self.query.clear();
        self.typing = false;
    }

    /// This search as the renderers read it.
    pub fn view(&self) -> SearchView<'_> {
        SearchView {
            query: self.query(),
            typing: self.typing,
        }
    }
}

/// The `/` search as the event list sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchView<'a> {
    /// What has been typed, while the prompt is open or a search stands.
    pub query: Option<&'a str>,
    /// Whether the prompt still holds the keys.
    pub typing: bool,
}

impl<'a> SearchView<'a> {
    /// The term to highlight, once enough of one has been typed.
    pub fn term(&self) -> Option<&'a str> {
        self.query.and_then(term)
    }
}

/// `query` as a term to highlight, or `None` while it is still too short to
/// be one. See [`MIN_TERM`].
pub fn term(query: &str) -> Option<&str> {
    let query = query.trim();
    (query.chars().count() >= MIN_TERM).then_some(query)
}

/// The byte ranges of the whitespace-separated words of `text` that contain
/// `term`.
///
/// A word is everything between two runs of whitespace, punctuation included,
/// so `src/retry.rs:20,` is one word: a path and the line number a reply cites
/// it at are what the operator went looking for, not three findings.
#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/search.rs"]
mod rendering;
