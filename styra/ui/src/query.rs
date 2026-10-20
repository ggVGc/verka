//! Typing into a search or filter field, the same in every one: the event
//! list's `/`, the diff's file filter and its text to find, the interaction
//! list's filter, and the typed-at pickers. A field reads its keys with
//! [`Edit::from_key`] and applies what comes back to its query; the keys that
//! leave it — Enter, Esc, the arrows — stay each field's own.

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
        match self {
            Self::Push(character) => query.push(character),
            Self::Backspace => {
                query.pop();
            }
            Self::DeleteWord => delete_word(query),
        }
    }
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
    let boundary = |character: char| !character.is_alphanumeric();
    query.truncate(query.trim_end_matches(boundary).len());
    let word_start = query.rfind(boundary).map_or(0, |index| {
        index + query[index..].chars().next().map_or(1, char::len_utf8)
    });
    query.truncate(word_start);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deleted(query: &str) -> String {
        let mut query = query.to_owned();
        delete_word(&mut query);
        query
    }

    #[test]
    fn a_word_ends_at_punctuation_and_whitespace() {
        assert_eq!(
            deleted("claude:claude-opus-5/high"),
            "claude:claude-opus-5/"
        );
        assert_eq!(deleted("claude:claude-opus-5/"), "claude:claude-opus-");
        assert_eq!(deleted("retry backoff  "), "retry ");
        assert_eq!(deleted("src/retry_backoff"), "src/retry_");
        assert_eq!(deleted("é·ñame"), "é·");
        assert_eq!(deleted("styra wörk- "), "styra ");
        assert_eq!(deleted("retry"), "");
        assert_eq!(deleted(""), "");
    }

    #[test]
    fn keys_are_text_unless_they_are_chords() {
        let key = |code, modifiers| Edit::from_key(KeyEvent::new(code, modifiers));
        assert_eq!(
            key(KeyCode::Char('w'), KeyModifiers::NONE),
            Some(Edit::Push('w'))
        );
        assert_eq!(
            key(KeyCode::Char('W'), KeyModifiers::SHIFT),
            Some(Edit::Push('W'))
        );
        assert_eq!(
            key(KeyCode::Char('w'), KeyModifiers::CONTROL),
            Some(Edit::DeleteWord)
        );
        assert_eq!(
            key(KeyCode::Backspace, KeyModifiers::NONE),
            Some(Edit::Backspace)
        );
        assert_eq!(key(KeyCode::Char('u'), KeyModifiers::CONTROL), None);
        assert_eq!(key(KeyCode::Char('w'), KeyModifiers::ALT), None);
        assert_eq!(key(KeyCode::Enter, KeyModifiers::NONE), None);
    }
}
