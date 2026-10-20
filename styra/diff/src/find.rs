//! The line selection's `/`: text found in the patch's code, marked wherever
//! it is drawn, as Vim's `hlsearch` marks it, and stepped through a line at a
//! time with `n` and `N`.
//!
//! Only code is searched — the added, removed and unchanged lines of hunks —
//! not file headers or hunk cuts, so a term is found where the line cursor
//! can stand on it and open the file at. The term is matched as typed, and
//! without regard to case unless it has a capital in it, as Vim's
//! `smartcase` has it.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use styra_ui::theme;

use crate::split::{aligned, SplitRow};
use crate::view::{rows, Row};

/// For each line of `text`, its code as drawn — without the sign, tabs as
/// four spaces — if it is a line of code in a hunk.
pub(crate) fn code_lines(text: &str) -> Vec<Option<String>> {
    let mut in_hunk = false;
    text.lines()
        .map(|line| {
            if line.starts_with("diff --git ") {
                in_hunk = false;
            } else if line.starts_with("@@") {
                in_hunk = true;
                return None;
            }
            let marker = ["--- a/", "--- /dev/null", "+++ b/", "+++ /dev/null"]
                .iter()
                .any(|marker| line.starts_with(marker));
            let code = line.is_empty() || line.starts_with(['+', '-', ' ']);
            (in_hunk && code && !marker)
                .then(|| line.get(1..).unwrap_or_default().replace('\t', "    "))
        })
        .collect()
}

/// The byte ranges of `code` where `term` is found, one after another.
pub(crate) fn matches(code: &str, term: &str) -> Vec<(usize, usize)> {
    let fold = !term.chars().any(char::is_uppercase);
    let same = |a: char, b: char| a == b || fold && a.to_lowercase().eq(b.to_lowercase());
    let mut found = Vec::new();
    if term.is_empty() {
        return found;
    }
    let mut from = 0;
    while from < code.len() {
        let mut rest = code[from..].char_indices();
        let mut end = None;
        let mut wanted = term.chars();
        loop {
            match (wanted.next(), rest.next()) {
                (None, next) => {
                    end = Some(from + next.map_or(code.len() - from, |(at, _)| at));
                    break;
                }
                (Some(want), Some((_, have))) if same(want, have) => {}
                _ => break,
            }
        }
        let step = code[from..].chars().next().map_or(1, char::len_utf8);
        match end {
            Some(end) => {
                found.push((from, end));
                from = end;
            }
            None => from += step,
        }
    }
    found
}

/// Mark `term` in the code of each of `lines`, `text` drawn a line for each
/// of its own. The marks are cut into the spans they fall across, so each
/// part keeps the style it was drawn with beneath the mark.
pub(crate) fn highlight(text: &str, lines: &mut [Line<'static>], term: &str) {
    for (line, code) in lines.iter_mut().zip(code_lines(text)) {
        let Some(code) = code else { continue };
        let found = matches(&code, term);
        if found.is_empty() {
            continue;
        }
        let drawn: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        // The code is drawn last, after the line number and the sign.
        if !drawn.ends_with(code.as_str()) {
            continue;
        }
        let offset = drawn.len() - code.len();
        let found: Vec<_> = found
            .into_iter()
            .map(|(from, to)| (from + offset, to + offset))
            .collect();
        let mut spans = Vec::with_capacity(line.spans.len() + found.len() * 2);
        let mut at = 0;
        for span in std::mem::take(&mut line.spans) {
            let content = span.content.into_owned();
            let start = at;
            at += content.len();
            let mut cursor = 0;
            for &(from, to) in found.iter().filter(|(from, to)| *to > start && *from < at) {
                let from = from.saturating_sub(start).max(cursor);
                let to = (to - start).min(content.len());
                if to <= from {
                    continue;
                }
                if from > cursor {
                    spans.push(Span::styled(content[cursor..from].to_owned(), span.style));
                }
                spans.push(Span::styled(
                    content[from..to].to_owned(),
                    span.style.patch(style()),
                ));
                cursor = to;
            }
            if cursor < content.len() {
                spans.push(Span::styled(content[cursor..].to_owned(), span.style));
            }
        }
        line.spans = spans;
    }
}

/// The mark over found text: the colours Styra's other `/` searches mark
/// what they find with.
pub(crate) fn style() -> Style {
    Style::new().fg(theme::CODE_BACKGROUND).bg(theme::ACCENT)
}

/// The mark over found text on the line cursor's line, which `n` and `N`
/// step from, set apart from the rest as Vim's `CurSearch` sets it apart:
/// inverted, its colour on the code's dark background.
pub(crate) fn current_style() -> Style {
    Style::new()
        .fg(theme::LIGHT_ACCENT)
        .bg(theme::CODE_BACKGROUND)
        .add_modifier(Modifier::BOLD)
}

/// Each row drawn with `term` in it, in order, with the line on it to put the
/// line cursor on: side by side, the newer side's when both halves have it.
pub(crate) fn match_rows(
    text: &str,
    term: &str,
    hidden: bool,
    split: bool,
    per_file: bool,
) -> Vec<(usize, usize)> {
    let found: Vec<bool> = code_lines(text)
        .into_iter()
        .map(|code| code.is_some_and(|code| !matches(&code, term).is_empty()))
        .collect();
    let found = |line: usize| found.get(line).copied().unwrap_or(false);
    let drawn: Vec<(usize, Vec<usize>)> = if split {
        aligned(text, per_file)
            .into_iter()
            .enumerate()
            .map(|(row, shown)| {
                let lines = match shown {
                    SplitRow::Shared(Row::Line(line)) => vec![line],
                    SplitRow::Shared(_) => Vec::new(),
                    SplitRow::Pair(old, new) => [new, old]
                        .into_iter()
                        .flatten()
                        .map(|cell| cell.index)
                        .collect(),
                };
                (row, lines)
            })
            .collect()
    } else {
        rows(text, hidden, per_file)
            .into_iter()
            .enumerate()
            .map(|(row, shown)| match shown {
                Row::Line(line) => (row, vec![line]),
                Row::RuleAbove | Row::RuleBelow => (row, Vec::new()),
            })
            .collect()
    };
    drawn
        .into_iter()
        .filter_map(|(row, lines)| Some((row, lines.into_iter().find(|&line| found(line))?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(line: &Line<'_>) -> Vec<String> {
        line.spans
            .iter()
            .filter(|span| span.style.bg == Some(theme::ACCENT))
            .map(|span| span.content.to_string())
            .collect()
    }

    #[test]
    fn a_term_is_found_case_insensitively_unless_it_has_a_capital() {
        assert_eq!(matches("Retry retry", "retry"), vec![(0, 5), (6, 11)]);
        assert_eq!(matches("Retry retry", "Retry"), vec![(0, 5)]);
        assert_eq!(matches("aaaa", "aa"), vec![(0, 2), (2, 4)]);
        assert_eq!(matches("é é", "é"), vec![(0, 2), (3, 5)]);
        assert!(matches("abc", "").is_empty());
        assert!(matches("ab", "abc").is_empty());
    }

    #[test]
    fn only_code_is_searched() {
        let text = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n\
                    @@ -1,2 +1,2 @@ fn f\n f\tx\n-fn\n+f\n";
        assert_eq!(
            code_lines(text),
            vec![
                None,
                None,
                None,
                None,
                Some("f    x".into()),
                Some("fn".into()),
                Some("f".into()),
            ]
        );
    }

    #[test]
    fn the_matched_text_alone_is_marked_and_the_number_left_alone() {
        let text = "diff --git a/f b/f\n@@ -1 +1 @@\n-one one\n+none\n";
        let mut lines = vec![
            Line::from("f"),
            Line::from("┄┄┄"),
            Line::from(vec![Span::raw("1 "), Span::raw("-"), Span::raw("one one")]),
            Line::from(vec![
                Span::raw("1 "),
                Span::raw("+"),
                Span::raw("no"),
                Span::raw("ne"),
            ]),
        ];
        highlight(text, &mut lines, "one");
        assert_eq!(marked(&lines[2]), vec!["one", "one"]);
        assert_eq!(marked(&lines[3]), vec!["o", "ne"], "across spans");
        highlight(text, &mut lines, "1");
        assert_eq!(marked(&lines[2]), vec!["one", "one"], "not the number");
    }
}
