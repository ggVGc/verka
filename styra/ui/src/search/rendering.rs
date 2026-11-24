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

use super::*;
use crate::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The `/` search, shown along the bottom of the view it is marking.
///
/// It says how much is still missing while the term is too short, so a search
/// that highlights nothing yet does not look like a search that found nothing.
pub fn prompt(search: &SearchView<'_>) -> Option<Line<'static>> {
    let query = search.query?;
    let mut text = format!(" /{query}");
    if search.typing {
        text.push('▌');
    }
    let missing = MIN_TERM.saturating_sub(query.trim().chars().count());
    if missing > 0 {
        text.push_str(&format!(
            " · {missing} more character{}",
            if missing == 1 { "" } else { "s" }
        ));
    }
    text.push(' ');
    Some(Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )))
}

/// The mark drawn over a matching word.
///
/// Cyan rather than the yellow of a selection ([`theme::SELECTION_MARKER`]):
/// a match is not where the cursor is, and the two are routinely on screen at
/// once — the selected row can hold matches of its own.
fn match_style() -> Style {
    Style::new().fg(theme::CODE_BACKGROUND).bg(theme::ACCENT)
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

#[cfg(test)]
mod tests;
