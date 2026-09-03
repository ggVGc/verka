//! The last turn's typed answer, and the selection within it.
//!
//! Held apart from [`App`](crate::app::App) because the three fields go
//! together or not at all: an answer, the reason there was none, and a row
//! index that only means anything against the answer it was taken on. Leaving
//! a stale selection behind when the answer changes is the failure this type
//! exists to make impossible, so the selection is private and moves only
//! through this type's own methods.
//!
//! An answer whose value is absent is not the same as no answer: it is a reply
//! that missed its contract, which is shown rather than discarded so the
//! operator can re-read it under another shape. [`crate::ui::answer`] renders
//! it.

use styra_server::{Answer, AnswerValue, FileLocation};

/// The fetched answer and where the operator is in it.
#[derive(Default)]
pub struct AnswerView {
    /// `None` before anything has been asked for.
    answer: Option<Answer>,
    /// Why the last answer could not be fetched at all — no typed turn, no
    /// reply yet, no session. Distinct from an answer that arrived and failed
    /// to parse, which is an `Answer`.
    error: Option<String>,
    /// Which row is selected, for the shapes that are navigated rather than
    /// read. Always within the current answer, because it is reset with it.
    selected: usize,
}

impl AnswerView {
    /// Record a fetched answer, or why there was none to fetch. Either way the
    /// selection returns to the first row: it belonged to the answer being
    /// replaced.
    pub fn set(&mut self, answer: Result<Answer, String>) {
        self.selected = 0;
        match answer {
            Ok(answer) => {
                self.answer = Some(answer);
                self.error = None;
            }
            Err(error) => {
                self.answer = None;
                self.error = Some(error);
            }
        }
    }

    pub fn answer(&self) -> Option<&Answer> {
        self.answer.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    fn value(&self) -> Option<&AnswerValue> {
        self.answer.as_ref().and_then(|answer| answer.value.as_ref())
    }

    /// How many selectable rows the current answer has; 0 for shapes that are
    /// read rather than navigated.
    pub fn rows(&self) -> usize {
        match self.value() {
            Some(AnswerValue::Lines(lines)) => lines.len(),
            Some(AnswerValue::Files(files)) => files.len(),
            _ => 0,
        }
    }

    pub fn select_next(&mut self) {
        let last = self.rows().saturating_sub(1);
        self.selected = self.selected.saturating_add(1).min(last);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.rows().saturating_sub(1);
    }

    /// The file location the selection is on, if the answer is showing files
    /// at all. What `e` opens, and what the footer names.
    pub fn selected_file(&self) -> Option<&FileLocation> {
        match self.value() {
            Some(AnswerValue::Files(files)) => files.get(self.selected),
            _ => None,
        }
    }

    /// The selected row as text, for the `y` shortcut. A navigable answer
    /// copies the row the operator is on; one that is read rather than
    /// navigated copies the whole value, which is what reaching for `y` on a
    /// JSON or prose answer means.
    pub fn copy_text(&self) -> Option<String> {
        let answer = self.answer.as_ref()?;
        match answer.value.as_ref() {
            Some(AnswerValue::Lines(lines)) => lines.get(self.selected).cloned(),
            Some(AnswerValue::Files(_)) => self.selected_file().map(FileLocation::located),
            Some(AnswerValue::Text(text)) => Some(text.clone()),
            Some(AnswerValue::Json(json)) => serde_json::to_string_pretty(json).ok(),
            // Nothing parsed, so the reply itself is the only thing there is
            // to copy — and the thing worth looking at.
            None => Some(answer.source.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(value: Option<AnswerValue>) -> Answer {
        Answer {
            contract: styra_server::Contract::Lines,
            value,
            error: None,
            source: "the reply as it came".into(),
        }
    }

    fn lines(count: usize) -> Answer {
        answer(Some(AnswerValue::Lines(
            (0..count).map(|row| format!("line {row}")).collect(),
        )))
    }

    /// The selection is an index into an answer that is being replaced, so
    /// carrying it over would point at a row of the previous one.
    #[test]
    fn a_new_answer_takes_the_selection_back_to_its_first_row() {
        let mut view = AnswerView::default();
        view.set(Ok(lines(5)));
        view.select_next();
        view.select_next();
        assert_eq!(view.selected_index(), 2);

        view.set(Ok(lines(2)));

        assert_eq!(view.selected_index(), 0);
    }

    #[test]
    fn a_failed_fetch_clears_the_previous_answer_and_says_why() {
        let mut view = AnswerView::default();
        view.set(Ok(lines(3)));

        view.set(Err("no typed turn yet".into()));

        assert!(view.answer().is_none());
        assert_eq!(view.error(), Some("no typed turn yet"));
        assert_eq!(view.rows(), 0);
    }

    #[test]
    fn a_fetched_answer_clears_the_previous_error() {
        let mut view = AnswerView::default();
        view.set(Err("no reply yet".into()));

        view.set(Ok(lines(1)));

        assert!(view.error().is_none());
        assert!(view.answer().is_some());
    }

    #[test]
    fn the_selection_cannot_leave_the_rows_that_exist() {
        let mut view = AnswerView::default();
        view.set(Ok(lines(2)));

        for _ in 0..5 {
            view.select_next();
        }
        assert_eq!(view.selected_index(), 1, "held at the last row");

        for _ in 0..5 {
            view.select_prev();
        }
        assert_eq!(view.selected_index(), 0, "held at the first row");

        view.select_last();
        assert_eq!(view.selected_index(), 1);
        view.select_first();
        assert_eq!(view.selected_index(), 0);
    }

    /// Prose and JSON are read rather than navigated, so there is no row for
    /// the selection to be on and `j` must not invent one.
    #[test]
    fn a_shape_that_is_read_rather_than_navigated_has_no_rows() {
        let mut view = AnswerView::default();
        view.set(Ok(answer(Some(AnswerValue::Text("a paragraph".into())))));

        view.select_next();

        assert_eq!(view.rows(), 0);
        assert_eq!(view.selected_index(), 0);
        assert_eq!(view.copy_text(), Some("a paragraph".into()));
    }

    /// A reply that missed its contract is kept, not discarded: copying it
    /// gives the operator the raw reply to judge for themselves.
    #[test]
    fn an_answer_that_missed_its_contract_copies_the_reply_itself() {
        let mut view = AnswerView::default();
        view.set(Ok(answer(None)));

        assert!(view.answer().is_some(), "shown rather than discarded");
        assert_eq!(view.copy_text(), Some("the reply as it came".into()));
    }

    #[test]
    fn a_navigable_answer_copies_the_row_the_selection_is_on() {
        let mut view = AnswerView::default();
        view.set(Ok(lines(3)));
        view.select_next();

        assert_eq!(view.copy_text(), Some("line 1".into()));
    }
}
