use super::*;

fn marked(line: &Line<'_>) -> Vec<String> {
    line.spans
        .iter()
        .filter(|span| span.style.bg == Some(theme::ACCENT))
        .map(|span| span.content.to_string())
        .collect()
}

#[test]
fn a_term_marks_the_whole_word_it_was_found_in() {
    let line = Line::from("the retry_backoff in src/retry.rs is fine");
    let marked = marked(&highlight_line(line, "retry"));

    assert_eq!(marked, vec!["retry_backoff", "src/retry.rs"]);
}

#[test]
fn matching_ignores_case_and_keeps_the_rest_of_the_line() {
    let line = highlight_line(Line::from("Retry once"), "ret");
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();

    assert_eq!(text, "Retry once");
    assert_eq!(marked(&line), vec!["Retry"]);
}

#[test]
fn a_word_cut_across_spans_is_marked_as_one() {
    let bold = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
    let line = Line::from(vec![
        Span::raw("src/re"),
        Span::styled("try", bold),
        Span::raw(".rs done"),
    ]);
    let line = highlight_line(line, "retry");

    assert_eq!(marked(&line).concat(), "src/retry.rs");
    // The mark is drawn over the styling, not instead of it.
    let emphasis = line
        .spans
        .iter()
        .find(|span| span.content == "try")
        .expect("the emphasized part of the word");
    assert!(emphasis
        .style
        .add_modifier
        .contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn nothing_is_marked_until_the_term_is_long_enough() {
    for (query, expected) in [("re", None), ("  re ", None), (" retry ", Some("retry"))] {
        assert_eq!(term(query), expected, "{query:?}");
        let view = SearchView {
            query: Some(query),
            typing: true,
        };
        assert_eq!(view.term(), expected, "{query:?}");
    }
    assert!(marked(&highlight_line(Line::from("retry"), "retry")).len() == 1);

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
fn a_line_with_no_match_is_left_exactly_as_it_was() {
    let line = Line::from(vec![Span::raw("nothing"), Span::raw(" here")]);
    let highlighted = highlight_line(line.clone(), "retry");

    assert_eq!(highlighted.spans.len(), line.spans.len());
    assert!(marked(&highlighted).is_empty());
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
fn deleting_the_only_word_keeps_the_prompt_open() {
    let mut search = Search::default();
    search.open();
    for character in "src/retry".chars() {
        search.edit(Edit::Push(character));
    }
    search.edit(Edit::DeleteWord);
    assert_eq!(search.query(), Some("src/"));
    search.edit(Edit::DeleteWord);
    assert!(search.typing());
    assert_eq!(search.query(), Some(""));
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
