//! Styling for unified diffs: each added or removed line's sign in green or
//! red, and the code after it highlighted in the language of the file it
//! belongs to.
//!
//! The language comes from the file's path, which a diff names in its own
//! headers (`diff --git`, `+++`) — or does not, as with the snippets
//! reconstructed from Claude's Edit calls, whose path the caller passes in.
//! Each hunk's code is highlighted as one block rather than line by line, so
//! a string or comment spanning lines keeps its colors; removed and added
//! lines are interleaved in that block, which the grammars tolerate well.
//!
//! Code in a language the highlighter does not know keeps the plain
//! rendering: the whole line green or red.
//!
//! Each code line is numbered from its hunk header (`@@ -12,3 +12,4 @@`): a
//! removal with the line it had in the old file, anything else with its line
//! in the new one. A hunk whose header gives no position — Claude's snippets,
//! where the host could not place them — leaves the column blank.

mod words;
pub use words::highlight_words;

use crate::event_list::{blended_style, faded_style};
use crate::language::from_path as language_of;
use crate::markdown::syntax_highlighted_code_lines;
use crate::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render `text` as a diff without decorative indentation or a gutter.
/// `path` names the file for a diff that carries no headers saying
/// so; a header in the diff takes over from it.
///
/// `compact` leaves out what a reader skimming a change does not need:
/// unchanged context and the per-file metadata, keeping the path each file
/// header names and the hunk boundaries.
pub fn diff_block_lines(text: &str, path: Option<&str>, compact: bool) -> Vec<Line<'static>> {
    diff_body_lines(text, path, compact)
}

/// How far, in tenths, context and other metadata are faded toward the
/// background, so the changes stand out from them.
pub const CONTEXT_FADE: u16 = 4;

/// [`diff_block_lines`], with context and other metadata faded
/// `context_fade` tenths of the way toward the background rather than
/// [`CONTEXT_FADE`].
pub fn diff_block_lines_fading(
    text: &str,
    path: Option<&str>,
    compact: bool,
    context_fade: u16,
) -> Vec<Line<'static>> {
    diff_body_lines_fading(text, path, compact, context_fade)
}

/// The diff's rows, without the gutter.
pub(crate) fn diff_body_lines(text: &str, path: Option<&str>, compact: bool) -> Vec<Line<'static>> {
    diff_body_lines_fading(text, path, compact, CONTEXT_FADE)
}

fn diff_body_lines_fading(
    text: &str,
    path: Option<&str>,
    compact: bool,
    context_fade: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut bright_rows = Vec::new();
    let mut language = path.and_then(language_of);
    let mut hunk: Vec<(&str, Option<u32>)> = Vec::new();
    let mut in_hunk = false;
    let width = number_width(text);
    // The next old and new line numbers, while inside a hunk that gave them.
    let mut next: Option<(u32, u32)> = None;
    let flush =
        |lines: &mut Vec<Line<'static>>,
         hunk: &mut Vec<(&str, Option<u32>)>,
         language: &Option<String>| { flush(lines, hunk, language.as_deref(), width) };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush(&mut lines, &mut hunk, &language);
            in_hunk = false;
            next = None;
            let named = rest
                .rsplit_once(" b/")
                .map(|(_, path)| path)
                .unwrap_or(rest);
            language = language_of(named).or(language);
            bright_rows.push(lines.len());
            lines.push(header(if compact { named } else { line }, theme::ACCENT));
        } else if line.starts_with("@@") {
            flush(&mut lines, &mut hunk, &language);
            in_hunk = true;
            next = hunk_range(line).map(|(old, _, new, _)| (old, new));
            bright_rows.push(lines.len());
            lines.push(hunk_separator(line));
        } else if !in_hunk || is_file_marker(line) {
            // Everything between a file header and its first hunk describes
            // the file rather than changing it. A diff with neither header —
            // a bare run of `+`/`-` lines — is all code.
            if let Some(named) = line.strip_prefix("+++ ") {
                let named = named.strip_prefix("b/").unwrap_or(named);
                language = language_of(named).or(language);
            }
            if is_code(line) && !is_file_marker(line) {
                hunk.push((line, None));
            } else if !compact {
                flush(&mut lines, &mut hunk, &language);
                if line.starts_with("--- ") || line.starts_with("+++ ") {
                    bright_rows.push(lines.len());
                }
                lines.push(header(line, theme::TEXT));
            }
        } else if !is_code(line) {
            // `\ No newline at end of file`, which is no line of either file.
            if !compact {
                hunk.push((line, None));
            }
        } else {
            let number = next.as_mut().map(|(old, new)| {
                let number = if line.starts_with('-') { *old } else { *new };
                if !line.starts_with('+') {
                    *old += 1;
                }
                if !line.starts_with('-') {
                    *new += 1;
                }
                number
            });
            // Context still counts toward the numbers even where it is not
            // shown.
            if !compact || is_change(line) {
                hunk.push((line, number));
            }
        }
    }
    flush(&mut lines, &mut hunk, &language);
    // Keep additions, file names and hunk cuts at full brightness; subdue
    // context and other metadata.
    for (index, line) in lines.iter_mut().enumerate() {
        // Not even dimmed, as a named colour would be, at no fade at all.
        if context_fade > 0 && line.style.bg.is_none() && bright_rows.binary_search(&index).is_err()
        {
            for span in &mut line.spans {
                span.style = faded_style(span.style, context_fade);
            }
        }
    }
    lines
}

/// A hunk header's old start and length and new start and length. A length
/// left out is one, as git writes it.
fn hunk_range(line: &str) -> Option<(u32, u32, u32, u32)> {
    let mut parts = line.strip_prefix("@@ ")?.split_whitespace();
    let side = |part: Option<&str>, sign: char| -> Option<(u32, u32)> {
        let range = part?.strip_prefix(sign)?;
        match range.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((range.parse().ok()?, 1)),
        }
    };
    let (old, old_len) = side(parts.next(), '-')?;
    let (new, new_len) = side(parts.next(), '+')?;
    Some((old, old_len, new, new_len))
}

/// Digits in the largest line number any hunk reaches, so the column lines
/// up; zero when no hunk gives a position and there is no column at all.
fn number_width(text: &str) -> usize {
    text.lines()
        .filter_map(hunk_range)
        .map(|(old, old_len, new, new_len)| (old + old_len).max(new + new_len))
        .max()
        .map_or(0, |last| last.to_string().len())
}

/// The `---`/`+++` lines naming a file's two sides. A removed line whose code
/// begins `-- ` looks the same, but only inside a hunk, and there a file
/// marker always names a side the way git writes it.
fn is_file_marker(line: &str) -> bool {
    ["--- a/", "--- /dev/null", "+++ b/", "+++ /dev/null"]
        .iter()
        .any(|marker| line.starts_with(marker))
}

fn is_change(line: &str) -> bool {
    line.starts_with('+') || line.starts_with('-')
}

/// A line of code in the diff: a change, or unchanged context — which some
/// producers trim to an empty line when the code itself was blank.
fn is_code(line: &str) -> bool {
    is_change(line) || line.starts_with(' ') || line.is_empty()
}

fn header(line: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        line.replace('\t', "    "),
        Style::default().fg(color),
    ))
}

/// Mark the gap in the file with the hunk's context, or a dashed rule where
/// it has none, in bold so the cut stands out from the code either side of
/// it. The diff view runs dashes on after it to the edge of the panel.
/// Coordinates are still used for numbering, but are not shown in the cut.
fn hunk_separator(line: &str) -> Line<'static> {
    let rest = line.trim_start_matches('@');
    let (inside, context) = rest.split_once("@@").unwrap_or((rest, ""));
    let context = if context.trim().is_empty() && hunk_range(line).is_none() {
        inside.trim()
    } else {
        context.trim()
    };
    let label = if context.is_empty() {
        "┄┄┄"
    } else {
        context
    };
    let mut line = header(label, theme::DIFF_HUNK_CUT);
    for span in &mut line.spans {
        span.style = span.style.add_modifier(Modifier::BOLD);
    }
    line
}

/// Render the collected run of code lines and empty it.
fn flush(
    lines: &mut Vec<Line<'static>>,
    hunk: &mut Vec<(&str, Option<u32>)>,
    language: Option<&str>,
    width: usize,
) {
    if hunk.is_empty() {
        return;
    }
    let number = |number: Option<u32>| -> Option<Span<'static>> {
        (width > 0).then(|| {
            let text = match number {
                Some(number) => format!("{number:>width$} "),
                None => " ".repeat(width + 1),
            };
            Span::styled(text, Style::default().fg(theme::INACTIVE))
        })
    };
    let code: Vec<String> = hunk
        .iter()
        .map(|(line, _)| line.get(1..).unwrap_or_default().replace('\t', "    "))
        .collect();
    let highlighted = language
        .and_then(|language| syntax_highlighted_code_lines(&code.join("\n"), Some(language), ""))
        // The highlighter renders one line per line of code; anything else
        // cannot be matched back up with the signs.
        .filter(|highlighted| highlighted.len() == hunk.len());
    match highlighted {
        Some(highlighted) => {
            for (&(line, at), code) in hunk.iter().zip(highlighted) {
                let mut spans: Vec<Span<'static>> = number(at).into_iter().collect();
                spans.push(sign(line));
                spans.extend(
                    code.spans
                        .into_iter()
                        .filter(|span| !span.content.is_empty()),
                );
                lines.push(tinted(Line::from(spans), line));
            }
        }
        None => {
            for (&(line, at), code) in hunk.iter().zip(code) {
                let color = match line.chars().next() {
                    Some('+') => theme::SUCCESS,
                    Some('-') => theme::ERROR,
                    _ => theme::TEXT,
                };
                let mut spans: Vec<Span<'static>> = number(at).into_iter().collect();
                spans.push(sign(line));
                spans.push(Span::styled(code, Style::default().fg(color)));
                lines.push(tinted(Line::from(spans), line));
            }
        }
    }
    hunk.clear();
}

/// An added or removed row with its background hint. Context is left on
/// whatever it is drawn over. Removed text is darkened and tinted red
/// while retaining its syntax colors.
///
/// The hint is on every span as well as on the row: the event list and the
/// preview re-wrap rows span by span, which keeps the spans' styles but not
/// the row's. [`fill_changed_rows`] then carries it to the panel's edge.
fn tinted(row: Line<'static>, line: &str) -> Line<'static> {
    let background = match line.chars().next() {
        Some('+') => theme::DIFF_ADDED_BACKGROUND,
        Some('-') => theme::DIFF_REMOVED_BACKGROUND,
        _ => return row,
    };
    let spans = row
        .spans
        .into_iter()
        .map(|span| {
            let style = if line.starts_with('-') {
                blended_style(
                    faded_style(span.style, 3),
                    theme::DIFF_REMOVED_FOREGROUND_TINT,
                    4,
                )
            } else {
                span.style
            }
            .bg(background);
            span.style(style)
        })
        .collect::<Vec<_>>();
    Line::from(spans).style(Style::default().bg(background))
}

/// Pad each changed diff row out to `width`, so its background hint is a band
/// across the panel rather than stopping where the code does — `Paragraph`
/// colors only the cells it writes. For rows already wrapped to `width`.
pub fn fill_changed_rows(lines: &mut [Line<'static>], width: usize) {
    for line in lines {
        let Some(background) = line
            .style
            .bg
            .or_else(|| {
                line.spans
                    .iter()
                    .filter_map(|span| span.style.bg)
                    .find(|bg| {
                        [theme::DIFF_ADDED_BACKGROUND, theme::DIFF_REMOVED_BACKGROUND].contains(bg)
                    })
            })
            .filter(|bg| {
                [theme::DIFF_ADDED_BACKGROUND, theme::DIFF_REMOVED_BACKGROUND].contains(bg)
            })
        else {
            continue;
        };
        let pad = width.saturating_sub(line.width());
        if pad > 0 {
            line.spans.push(Span::styled(
                " ".repeat(pad),
                Style::default().bg(background),
            ));
        }
    }
}

/// A code line's first column: green for an addition, red for a removal, and
/// a blank for context.
fn sign(line: &str) -> Span<'static> {
    match line.chars().next() {
        Some('+') => Span::styled(
            "+",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ),
        Some('-') => Span::styled(
            "-",
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ),
        _ => Span::raw(" "),
    }
}

#[cfg(test)]
mod tests;
