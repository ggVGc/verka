//! The message being typed, and the history of the ones already sent.
//!
//! Held apart from [`App`](crate::app::App) because none of it depends on the
//! session: the buffer, the readline-style edits over it, and walking back
//! through earlier prompts are the same whether an agent is running or not.

/// The operator's message buffer and their prompt history.
#[derive(Default)]
pub struct Composer {
    /// What is in the message box right now.
    pub text: String,
    /// Messages already submitted this session, oldest first.
    history: Vec<String>,
    /// How far back through `history` the operator has walked, if at all.
    cursor: Option<usize>,
    /// The half-typed message set aside while walking back, so `Down` can
    /// return to it.
    draft: String,
}

impl Composer {
    pub fn set(&mut self, text: String) {
        self.text = text;
        self.reset_history();
    }

    pub fn char(&mut self, ch: char) {
        self.reset_history();
        self.text.push(ch);
    }

    pub fn backspace(&mut self) {
        self.reset_history();
        self.text.pop();
    }

    /// Delete the word immediately before the end of the buffer (`Ctrl-W`),
    /// readline-style: trailing whitespace first, then non-whitespace back
    /// to the previous word boundary (or the start of the buffer).
    pub fn delete_word(&mut self) {
        self.reset_history();
        let trimmed = self.text.trim_end_matches(char::is_whitespace).len();
        self.text.truncate(trimmed);
        let word_start = self
            .text
            .rfind(char::is_whitespace)
            .map(|idx| idx + 1)
            .unwrap_or(0);
        self.text.truncate(word_start);
    }

    pub fn newline(&mut self) {
        self.reset_history();
        self.text.push('\n');
    }

    /// Recall older submitted prompts, preserving the current draft so `Down`
    /// can return to it after walking back to the newest history entry.
    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.cursor {
            Some(index) => index.saturating_sub(1),
            None => {
                self.draft.clone_from(&self.text);
                self.history.len() - 1
            }
        };
        self.cursor = Some(next);
        self.text.clone_from(&self.history[next]);
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.cursor else {
            return;
        };
        if index + 1 < self.history.len() {
            let next = index + 1;
            self.cursor = Some(next);
            self.text.clone_from(&self.history[next]);
        } else {
            self.cursor = None;
            self.text.clone_from(&self.draft);
            self.draft.clear();
        }
    }

    fn reset_history(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }

    /// Take the trimmed message for sending, clearing the buffer. Returns
    /// `None` when the buffer holds only whitespace.
    pub fn take(&mut self) -> Option<String> {
        let message = self.text.trim().to_owned();
        self.text.clear();
        if message.is_empty() {
            return None;
        }
        self.history.push(message.clone());
        self.reset_history();
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_edits_the_buffer_and_sending_empties_it() {
        let mut composer = Composer::default();
        composer.char('h');
        composer.char('i');
        composer.newline();
        composer.char('!');
        composer.backspace();
        assert_eq!(composer.text, "hi\n");
        assert_eq!(composer.take(), Some("hi".into()));
        assert!(composer.text.is_empty());
        assert_eq!(composer.take(), None);
    }

    #[test]
    fn delete_word_removes_the_trailing_word_readline_style() {
        let mut composer = Composer::default();
        composer.set("fix the flaky test".into());
        composer.delete_word();
        assert_eq!(composer.text, "fix the flaky ");
        composer.delete_word();
        assert_eq!(composer.text, "fix the ");
        composer.set("one".into());
        composer.delete_word();
        assert!(composer.text.is_empty());
        // And on an empty buffer it is a no-op rather than an underflow.
        composer.delete_word();
        assert!(composer.text.is_empty());
    }

    /// Walking back through history keeps the half-typed message, so `Down`
    /// returns to it rather than to an empty box.
    #[test]
    fn history_walks_back_and_returns_to_the_draft() {
        let mut composer = Composer::default();
        composer.set("first".into());
        composer.take();
        composer.set("second".into());
        composer.take();

        composer.set("draft".into());
        composer.history_previous();
        assert_eq!(composer.text, "second");
        composer.history_previous();
        assert_eq!(composer.text, "first");
        // Already at the oldest: staying there beats wrapping around.
        composer.history_previous();
        assert_eq!(composer.text, "first");

        composer.history_next();
        assert_eq!(composer.text, "second");
        composer.history_next();
        assert_eq!(composer.text, "draft");
        // Past the newest there is nothing to return to a second time.
        composer.history_next();
        assert_eq!(composer.text, "draft");
    }

    /// Typing anything abandons the walk, so the next `Up` starts over from the
    /// newest entry with the new text as the draft.
    #[test]
    fn typing_ends_a_history_walk() {
        let mut composer = Composer::default();
        composer.set("first".into());
        composer.take();

        composer.history_previous();
        assert_eq!(composer.text, "first");
        composer.char('!');
        composer.history_previous();
        assert_eq!(composer.text, "first");
        composer.history_next();
        assert_eq!(composer.text, "first!");
    }
}
