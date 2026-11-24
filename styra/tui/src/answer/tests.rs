use super::*;

fn answer(value: Option<AnswerValue>) -> Answer {
    Answer {
        contract: styra_protocol::Contract::Lines,
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
