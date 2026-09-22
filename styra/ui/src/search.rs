//! Highlighting the words a `/` search matches.
//!
//! The search marks whole words rather than the matched letters alone: what
//! the operator is looking for in a log is a path, an identifier, or a message
//! — `retry` should point at `retry_backoff` and `src/retry.rs` entire, so the
//! thing found can be read off the screen without hunting for where the match
//! ends.

use crate::palette;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// How much has to be typed before anything is highlighted.
///
/// One or two letters match most of a log at once, which highlights so much
/// that nothing stands out — and the list would flash on the way to every
/// term. Three is enough to be looking for something.
pub const MIN_TERM: usize = 3;

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

/// The mark drawn over a matching word.
///
/// Cyan rather than the yellow of a selection ([`palette::SELECTION_MARKER`]):
/// a match is not where the cursor is, and the two are routinely on screen at
/// once — the selected row can hold matches of its own.
fn match_style() -> Style {
    Style::new()
        .fg(palette::CODE_BACKGROUND)
        .bg(palette::ACCENT)
}

/// Mark every word of `lines` containing `term`, case-insensitively.
pub fn highlight_lines(lines: Vec<Line<'static>>, term: Option<&str>) -> Vec<Line<'static>> {
    let Some(term) = term else {
        return lines;
    };
    lines
        .into_iter()
        .map(|line| highlight_line(line, term))
        .collect()
}

/// As [`highlight_lines`], for one rendered line.
///
/// Matching is done against the line's whole text rather than span by span,
/// because styling cuts a word up wherever it likes: a Markdown link, inline
/// code, or a wrapped table cell all arrive as several spans of one word. The
/// spans are then cut again at the marked boundaries, so each keeps the style
/// it was rendered with beneath the mark.
pub fn highlight_line(line: Line<'static>, term: &str) -> Line<'static> {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let words = matching_words(&text, term);
    if words.is_empty() {
        return line;
    }
    let mark = match_style();
    let style = line.style;
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    let mut at = 0usize;
    for span in line.spans {
        let content = span.content.into_owned();
        let start = at;
        at += content.len();
        let mut cursor = 0usize;
        for (word_from, word_to) in words.iter().filter(|(from, to)| *to > start && *from < at) {
            let from = word_from.saturating_sub(start).max(cursor);
            let to = (word_to - start).min(content.len());
            if to <= from {
                continue;
            }
            if from > cursor {
                spans.push(Span::styled(content[cursor..from].to_owned(), span.style));
            }
            spans.push(Span::styled(
                content[from..to].to_owned(),
                span.style.patch(mark),
            ));
            cursor = to;
        }
        if cursor < content.len() {
            spans.push(Span::styled(content[cursor..].to_owned(), span.style));
        }
    }
    Line::from(spans).style(style)
}

/// The byte ranges of the whitespace-separated words of `text` that contain
/// `term`.
///
/// A word is everything between two runs of whitespace, punctuation included,
/// so `src/retry.rs:20,` is one word: a path and the line number a reply cites
/// it at are what the operator went looking for, not three findings.
fn matching_words(text: &str, term: &str) -> Vec<(usize, usize)> {
    let term = term.to_lowercase();
    let mut words = Vec::new();
    let mut word: Option<usize> = None;
    let matches = |from: usize, to: usize, words: &mut Vec<(usize, usize)>| {
        if text[from..to].to_lowercase().contains(&term) {
            words.push((from, to));
        }
    };
    for (index, character) in text.char_indices() {
        match (character.is_whitespace(), word) {
            (true, Some(from)) => {
                matches(from, index, &mut words);
                word = None;
            }
            (false, None) => word = Some(index),
            _ => {}
        }
    }
    if let Some(from) = word {
        matches(from, text.len(), &mut words);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(line: &Line<'_>) -> Vec<String> {
        line.spans
            .iter()
            .filter(|span| span.style.bg == Some(palette::ACCENT))
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
        assert_eq!(term("re"), None);
        assert_eq!(term("  re "), None);
        assert_eq!(term(" retry "), Some("retry"));

        let view = SearchView {
            query: Some("re"),
            typing: true,
        };
        assert_eq!(view.term(), None);
        assert!(marked(&highlight_line(Line::from("retry"), "retry")).len() == 1);
    }

    #[test]
    fn a_line_with_no_match_is_left_exactly_as_it_was() {
        let line = Line::from(vec![Span::raw("nothing"), Span::raw(" here")]);
        let highlighted = highlight_line(line.clone(), "retry");

        assert_eq!(highlighted.spans.len(), line.spans.len());
        assert!(marked(&highlighted).is_empty());
    }
}
