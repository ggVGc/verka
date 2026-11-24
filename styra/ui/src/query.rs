//! Typing into a search or filter field, the same in every one: the event
//! list's `/`, the diff's file filter and its text to find, the interaction
//! list's filter, and the typed-at pickers. A field reads its keys with
//! [`Edit::from_key`] and applies what comes back to its query; the keys that
//! leave it — Enter, Esc, the arrows — stay each field's own.
//!
//! The one-line prompts read their keys the same way, so that a chord is a
//! command in every field rather than a typed letter in some; one that holds
//! prose rather than a name or a path says so with [`Words::Prose`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One change to a typed query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// A printable character, typed at the end.
    Push(char),
    /// Drop the last character.
    Backspace,
    /// Drop the last word (`Ctrl-W`); see [`delete_word`].
    DeleteWord,
}

impl Edit {
    /// The change `key` makes to a query, if it makes one. A character
    /// pressed with Control or Alt is a command rather than text, and makes
    /// none, apart from `Ctrl-W`.
    pub fn from_key(key: KeyEvent) -> Option<Self> {
        let chord = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Backspace => Some(Self::Backspace),
            KeyCode::Char('w') if key.modifiers == KeyModifiers::CONTROL => Some(Self::DeleteWord),
            KeyCode::Char(character) if !chord && !character.is_control() => {
                Some(Self::Push(character))
            }
            _ => None,
        }
    }

    /// Make this change to `query`.
    pub fn apply(self, query: &mut String) {
        self.apply_with(query, Words::Identifiers);
    }

    /// Make this change to `text`, with `words` deciding what `Ctrl-W` drops.
    pub fn apply_with(self, text: &mut String, words: Words) {
        match self {
            Self::Push(character) => text.push(character),
            Self::Backspace => {
                text.pop();
            }
            Self::DeleteWord => delete_word_in(text, words),
        }
    }
}

/// What `Ctrl-W` takes as one word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Words {
    /// Words end at punctuation as well as whitespace; see [`delete_word`].
    /// For queries, names and paths.
    Identifiers,
    /// Words end at whitespace only, as in readline. For a message or a shell
    /// command, where `--bin` or `don't` is one thing to retype.
    Prose,
}

/// Drop the last word of `query`, readline-style: the separators at the end
/// first, then back to the one before them.
///
/// A word here ends at punctuation as well as at whitespace, because what is
/// searched for is mostly identifiers and paths rather than prose: in
/// `claude:claude-opus-5/high` or `src/retry_backoff.rs` the parts worth
/// retyping are the ones `:`, `-`, `_`, `.` and `/` divide, so one press
/// peels off one of them instead of the whole query.
pub fn delete_word(query: &mut String) {
    delete_word_in(query, Words::Identifiers);
}

/// Drop the last word of `text`, readline-style, as `words` divides it.
pub fn delete_word_in(text: &mut String, words: Words) {
    let boundary = |character: char| match words {
        Words::Identifiers => !character.is_alphanumeric(),
        Words::Prose => character.is_whitespace(),
    };
    text.truncate(text.trim_end_matches(boundary).len());
    let word_start = text.rfind(boundary).map_or(0, |index| {
        index + text[index..].chars().next().map_or(1, char::len_utf8)
    });
    text.truncate(word_start);
}

#[cfg(test)]
mod tests;
