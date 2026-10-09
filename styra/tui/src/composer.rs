//! The message being typed, and the history of the ones already sent.
//!
//! Held apart from [`App`](crate::app::App) because none of it depends on the
//! session: the buffer, the readline-style edits over it, and walking back
//! through earlier prompts are the same whether an agent is running or not.
//!
//! The server keeps a copy per Session (see [`ComposerState`]), so the boxes
//! and the history are there again from any client and after a restart; this
//! is the working copy, and it tracks what it last handed the server so the
//! event loop knows when there is something to store.
//!
//! A message may be built from several boxes — one for the instruction, one
//! for a pasted log, one for a constraint — which are typed separately and
//! sent together, each as its own paragraph. With more than one box the
//! operator can step back from typing and choose between them.

use styra_protocol::ComposerState;

/// The operator's message buffer and their prompt history.
pub struct Composer {
    /// The boxes the message is built from, in order. Never empty: a message
    /// with nothing in it is still one empty box.
    parts: Vec<String>,
    /// Which box is being typed into, or highlighted while choosing.
    focused: usize,
    /// Whether the operator is choosing between boxes rather than typing.
    choosing: bool,
    /// Messages already submitted this session, oldest first.
    history: Vec<String>,
    /// How far back through `history` the operator has walked, if at all.
    cursor: Option<usize>,
    /// The half-typed box set aside while walking back, so `Down` can return
    /// to it.
    draft: String,
    /// What the server was last known to hold for this Session.
    saved: ComposerState,
}

impl Default for Composer {
    fn default() -> Self {
        Self::restore(ComposerState::default())
    }
}

impl Composer {
    /// Pick up where the operator left a Session's message box.
    pub fn restore(state: ComposerState) -> Self {
        let mut parts = state.parts;
        if parts.is_empty() {
            parts.push(String::new());
        }
        let focused = state.focused.min(parts.len() - 1);
        let mut composer = Self {
            parts,
            focused,
            choosing: false,
            history: state.history,
            cursor: None,
            draft: String::new(),
            saved: ComposerState::default(),
        };
        composer.saved = composer.state();
        composer
    }

    /// The box as the server stores it.
    pub fn state(&self) -> ComposerState {
        ComposerState {
            parts: self.parts.clone(),
            focused: self.focused,
            history: self.history.clone(),
        }
    }

    /// What there is to store, if anything changed since the last
    /// [`Composer::mark_saved`].
    pub fn unsaved(&self) -> Option<ComposerState> {
        let saved = &self.saved;
        let same = self.parts == saved.parts
            && self.focused == saved.focused
            && self.history == saved.history;
        (!same).then(|| self.state())
    }

    pub fn mark_saved(&mut self, state: ComposerState) {
        self.saved = state;
    }

    /// The box being typed into.
    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.parts[self.focused]
    }

    fn text_mut(&mut self) -> &mut String {
        &mut self.parts[self.focused]
    }

    pub fn parts(&self) -> &[String] {
        &self.parts
    }

    pub fn focused(&self) -> usize {
        self.focused
    }

    pub fn choosing(&self) -> bool {
        self.choosing
    }

    /// The message the boxes make together: each non-blank box trimmed, as
    /// its own paragraph.
    pub fn message(&self) -> String {
        self.parts
            .iter()
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Replace the whole message with `text` in a single box.
    pub fn set(&mut self, text: String) {
        self.parts = vec![text];
        self.focused = 0;
        self.choosing = false;
        self.reset_history();
    }

    pub fn char(&mut self, ch: char) {
        self.reset_history();
        self.text_mut().push(ch);
    }

    pub fn backspace(&mut self) {
        self.reset_history();
        self.text_mut().pop();
    }

    /// Delete the word immediately before the end of the buffer (`Ctrl-W`),
    /// readline-style: trailing whitespace first, then non-whitespace back
    /// to the previous word boundary (or the start of the buffer).
    pub fn delete_word(&mut self) {
        self.reset_history();
        let text = self.text_mut();
        let trimmed = text.trim_end_matches(char::is_whitespace).len();
        text.truncate(trimmed);
        let word_start = text
            .rfind(char::is_whitespace)
            .map(|idx| idx + 1)
            .unwrap_or(0);
        text.truncate(word_start);
    }

    /// Append `text` as its own word, adding the separating space the operator
    /// would otherwise have had to remember to type first. Used by the path
    /// prompt, which produces one token rather than free text.
    pub fn insert(&mut self, text: &str) {
        self.reset_history();
        let buffer = self.text_mut();
        if !buffer.is_empty() && !buffer.ends_with(char::is_whitespace) {
            buffer.push(' ');
        }
        buffer.push_str(text);
    }

    pub fn newline(&mut self) {
        self.reset_history();
        self.text_mut().push('\n');
    }

    // --- Boxes ---------------------------------------------------------------

    /// Open a new, empty box straight after the current one and type into it.
    pub fn add_part(&mut self) {
        self.reset_history();
        self.focused += 1;
        self.parts.insert(self.focused, String::new());
        self.choosing = false;
    }

    /// Step back from typing to choose between boxes. Returns whether there
    /// was more than one box to choose between; with one there is nothing to
    /// choose, and the caller treats the key as leaving the box instead.
    pub fn start_choosing(&mut self) -> bool {
        self.choosing = self.parts.len() > 1;
        if self.choosing {
            self.reset_history();
        }
        self.choosing
    }

    /// Go back to typing, into whichever box is highlighted.
    pub fn stop_choosing(&mut self) {
        self.choosing = false;
    }

    /// Highlight the next box, staying on the last.
    pub fn choose_next(&mut self) {
        self.focused = (self.focused + 1).min(self.parts.len() - 1);
    }

    /// Highlight the previous box, staying on the first.
    pub fn choose_previous(&mut self) {
        self.focused = self.focused.saturating_sub(1);
    }

    /// Throw away the highlighted box. Down to a single box there is nothing
    /// left to choose between, so typing resumes in it.
    pub fn remove_chosen(&mut self) {
        if self.parts.len() <= 1 {
            return;
        }
        self.parts.remove(self.focused);
        self.focused = self.focused.min(self.parts.len() - 1);
        if self.parts.len() == 1 {
            self.choosing = false;
        }
    }

    // --- History -------------------------------------------------------------

    /// Recall older submitted prompts into the current box, preserving what it
    /// held so `Down` can return to it after walking back to the newest
    /// history entry.
    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.cursor {
            Some(index) => index.saturating_sub(1),
            None => {
                self.draft = self.parts[self.focused].clone();
                self.history.len() - 1
            }
        };
        self.cursor = Some(next);
        self.parts[self.focused].clone_from(&self.history[next]);
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.cursor else {
            return;
        };
        if index + 1 < self.history.len() {
            let next = index + 1;
            self.cursor = Some(next);
            self.parts[self.focused].clone_from(&self.history[next]);
        } else {
            self.cursor = None;
            self.parts[self.focused] = std::mem::take(&mut self.draft);
        }
    }

    fn reset_history(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }

    /// Take the message for sending (see [`Composer::message`]), leaving a
    /// single empty box. Returns `None` when every box holds only whitespace.
    pub fn take(&mut self) -> Option<String> {
        let message = self.message();
        self.set(String::new());
        if message.is_empty() {
            return None;
        }
        self.history.push(message.clone());
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A restored box is already saved; editing it is what makes it unsaved,
    /// and a stored state that names no box still opens one.
    #[test]
    fn restoring_tracks_what_the_server_holds() {
        let composer = Composer::default();
        assert_eq!(composer.unsaved(), None);

        let mut composer = Composer::restore(ComposerState {
            parts: vec!["a".into(), "b".into()],
            focused: 7,
            history: vec!["sent".into()],
        });
        assert_eq!(composer.focused(), 1);
        assert_eq!(composer.unsaved(), None);
        composer.char('!');
        let state = composer.unsaved().unwrap();
        assert_eq!(state.parts, ["a", "b!"]);
        composer.mark_saved(state);
        assert_eq!(composer.unsaved(), None);
        // Sending changes the history as well as the boxes.
        assert_eq!(composer.take(), Some("a\n\nb!".into()));
        assert_eq!(composer.unsaved().unwrap().history, ["sent", "a\n\nb!"]);

        let empty = Composer::restore(ComposerState::default());
        assert_eq!(empty.parts(), [""]);
    }

    #[test]
    fn typing_edits_the_buffer_and_sending_empties_it() {
        let mut composer = Composer::default();
        composer.char('h');
        composer.char('i');
        composer.newline();
        composer.char('!');
        composer.backspace();
        assert_eq!(composer.text(), "hi\n");
        assert_eq!(composer.take(), Some("hi".into()));
        assert!(composer.text().is_empty());
        assert_eq!(composer.take(), None);
    }

    #[test]
    fn delete_word_removes_the_trailing_word_readline_style() {
        let mut composer = Composer::default();
        composer.set("fix the flaky test".into());
        composer.delete_word();
        assert_eq!(composer.text(), "fix the flaky ");
        composer.delete_word();
        assert_eq!(composer.text(), "fix the ");
        composer.set("one".into());
        composer.delete_word();
        assert!(composer.text().is_empty());
        // And on an empty buffer it is a no-op rather than an underflow.
        composer.delete_word();
        assert!(composer.text().is_empty());
    }

    /// An inserted path is a word: it never runs into the word before it, and
    /// it never doubles a space the operator already typed.
    #[test]
    fn inserting_separates_what_it_appends() {
        let mut composer = Composer::default();
        composer.insert("/srv/data/notes.txt");
        assert_eq!(composer.text(), "/srv/data/notes.txt");
        composer.set("summarize".into());
        composer.insert("/srv/data/notes.txt");
        assert_eq!(composer.text(), "summarize /srv/data/notes.txt");
        composer.set("summarize ".into());
        composer.insert("/srv/data/notes.txt");
        assert_eq!(composer.text(), "summarize /srv/data/notes.txt");
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
        assert_eq!(composer.text(), "second");
        composer.history_previous();
        assert_eq!(composer.text(), "first");
        // Already at the oldest: staying there beats wrapping around.
        composer.history_previous();
        assert_eq!(composer.text(), "first");

        composer.history_next();
        assert_eq!(composer.text(), "second");
        composer.history_next();
        assert_eq!(composer.text(), "draft");
        // Past the newest there is nothing to return to a second time.
        composer.history_next();
        assert_eq!(composer.text(), "draft");
    }

    /// Typing anything abandons the walk, so the next `Up` starts over from the
    /// newest entry with the new text as the draft.
    #[test]
    fn typing_ends_a_history_walk() {
        let mut composer = Composer::default();
        composer.set("first".into());
        composer.take();

        composer.history_previous();
        assert_eq!(composer.text(), "first");
        composer.char('!');
        composer.history_previous();
        assert_eq!(composer.text(), "first");
        composer.history_next();
        assert_eq!(composer.text(), "first!");
    }
    /// Each box is typed separately; sending joins the non-blank ones as
    /// paragraphs, in order, and leaves one empty box behind.
    #[test]
    fn boxes_are_sent_together_as_paragraphs() {
        let mut composer = Composer::default();
        composer.set("fix the test ".into());
        composer.add_part();
        assert_eq!(composer.focused(), 1);
        assert!(composer.text().is_empty());
        composer.add_part();
        composer.insert("it fails on CI");
        assert_eq!(composer.message(), "fix the test\n\nit fails on CI");
        assert_eq!(
            composer.take(),
            Some("fix the test\n\nit fails on CI".into())
        );
        assert_eq!(composer.parts().len(), 1);
        assert!(composer.text().is_empty());
    }

    /// A new box goes straight after the current one, not at the end.
    #[test]
    fn a_new_box_follows_the_current_one() {
        let mut composer = Composer::default();
        composer.set("first".into());
        composer.add_part();
        composer.insert("third");
        assert!(composer.start_choosing());
        composer.choose_previous();
        composer.stop_choosing();
        composer.add_part();
        composer.insert("second");
        assert_eq!(composer.parts(), ["first", "second", "third"]);
    }

    /// Choosing needs something to choose between, and stays within the boxes.
    #[test]
    fn choosing_moves_between_boxes_and_removes_them() {
        let mut composer = Composer::default();
        assert!(!composer.start_choosing());
        assert!(!composer.choosing());

        composer.set("a".into());
        composer.add_part();
        composer.insert("b");
        assert!(composer.start_choosing());
        composer.choose_next();
        assert_eq!(composer.focused(), 1);
        composer.choose_previous();
        composer.choose_previous();
        assert_eq!(composer.focused(), 0);

        composer.remove_chosen();
        assert_eq!(composer.parts(), ["b"]);
        // With one box left there is nothing to choose between.
        assert!(!composer.choosing());
        composer.remove_chosen();
        assert_eq!(composer.parts(), ["b"]);
    }

    /// History recalls into the box being typed in, leaving the others be.
    #[test]
    fn history_recalls_into_the_current_box() {
        let mut composer = Composer::default();
        composer.set("earlier".into());
        composer.take();
        composer.set("context".into());
        composer.add_part();
        composer.history_previous();
        assert_eq!(composer.parts(), ["context", "earlier"]);
        composer.history_next();
        assert_eq!(composer.parts(), ["context", ""]);
    }
}
