//! The event list's `/` search: what has been typed, and who holds the keys.
//!
//! The search only marks what is already on screen — it moves neither the
//! selection nor the viewport — so it is a display choice like the filters
//! beside it rather than a way to navigate. Highlighting starts once enough
//! has been typed to be looking for something; see
//! [`styra_ui::search::MIN_TERM`].

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

    /// Hand the keys back to the list, leaving the term marked.
    pub fn accept(&mut self) {
        self.typing = false;
    }

    /// Leave the list as it was, unmarked.
    pub fn cancel(&mut self) {
        self.query.clear();
        self.typing = false;
    }

    /// This search as the event list reads it.
    pub fn view(&self) -> styra_ui::search::SearchView<'_> {
        styra_ui::search::SearchView {
            query: self.query(),
            typing: self.typing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_term_marks_nothing_until_it_is_long_enough() {
        let mut search = Search::default();
        search.open();
        for character in "re".chars() {
            search.push(character);
        }
        assert_eq!(search.view().term(), None);

        search.push('t');
        assert_eq!(search.view().term(), Some("ret"));
    }

    #[test]
    fn accepting_keeps_the_search_and_cancelling_drops_it() {
        let mut search = Search::default();
        search.open();
        for character in "retry".chars() {
            search.push(character);
        }

        search.accept();
        assert!(!search.typing());
        assert_eq!(search.view().term(), Some("retry"));

        search.cancel();
        assert_eq!(search.query(), None);
        assert_eq!(search.view().term(), None);
    }

    #[test]
    fn backspacing_the_last_character_closes_the_prompt() {
        let mut search = Search::default();
        search.open();
        search.push('r');
        search.backspace();

        assert!(!search.typing());
        assert_eq!(search.query(), None);
    }

    #[test]
    fn opening_again_asks_a_fresh_question() {
        let mut search = Search::default();
        search.open();
        for character in "retry".chars() {
            search.push(character);
        }
        search.accept();
        search.open();

        assert_eq!(search.query(), Some(""));
        assert_eq!(search.view().term(), None);
    }
}
