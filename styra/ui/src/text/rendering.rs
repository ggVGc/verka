use super::*;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

/// Clip to columns without an ellipsis, preserving line and span styles.
pub fn clip(mut line: Line<'static>, columns: usize) -> Line<'static> {
    let mut used = 0;
    let mut kept = Vec::new();
    for mut span in line.spans {
        let span_width = width(&span.content);
        if used + span_width <= columns {
            used += span_width;
            kept.push(span);
        } else {
            let content = prefix(&span.content, columns - used);
            if !content.is_empty() {
                span.content = content.to_owned().into();
                kept.push(span);
            }
            break;
        }
    }
    line.spans = kept;
    line
}

/// Clip with an ellipsis. By default it inherits the style at the cut.
pub fn truncate(
    line: Line<'static>,
    columns: usize,
    ellipsis_style: Option<Style>,
) -> Line<'static> {
    if line.width() <= columns {
        return line;
    }
    if columns == 0 {
        return clip(line, 0);
    }
    let budget = columns - width("…");
    let mut used = 0;
    let cut_style = line
        .spans
        .iter()
        .find_map(|span| {
            used += span.width();
            (used > budget).then_some(span.style)
        })
        .unwrap_or_default();
    let mut kept = clip(line, budget);
    kept.spans
        .push(Span::styled("…", ellipsis_style.unwrap_or(cut_style)));
    kept
}

/// Cut out of the middle with an ellipsis, keeping both the start and the
/// end in view. The ellipsis inherits the style at the cut.
pub fn truncate_middle(line: Line<'static>, columns: usize) -> Line<'static> {
    if line.width() <= columns {
        return line;
    }
    if columns == 0 {
        return clip(line, 0);
    }
    let budget = columns - width("…");
    let tail_budget = budget / 2;
    // The end, taken span by span from the back.
    let mut tail = Vec::new();
    let mut used = 0;
    for span in line.spans.iter().rev() {
        let span_width = span.width();
        if used + span_width <= tail_budget {
            used += span_width;
            tail.push(span.clone());
        } else {
            let content = suffix(&span.content, tail_budget - used);
            used += width(content);
            if !content.is_empty() {
                tail.push(Span::styled(content.to_owned(), span.style));
            }
            break;
        }
    }
    tail.reverse();
    let mut kept = clip(line, budget - used);
    let cut_style = kept
        .spans
        .last()
        .or(tail.first())
        .map(|span| span.style)
        .unwrap_or_default();
    kept.spans.push(Span::styled("…", cut_style));
    kept.spans.extend(tail);
    kept
}

/// Word-wrap with a hanging indent, preserving styles. Zero width means
/// unbounded, as used by callers laying out a pane before its size is known.
pub fn wrap_line(line: Line<'static>, columns: usize, indent: usize) -> Vec<Line<'static>> {
    wrap(line, columns, indent, true)
}

/// Hard-wrap editable text, preserving spaces and explicit empty lines.
pub fn wrap_input(text: &str, columns: usize, style: Style) -> Vec<Line<'static>> {
    text.split('\n')
        .flat_map(|logical| {
            wrap(
                Line::from(Span::styled(logical.to_owned(), style)),
                columns,
                0,
                false,
            )
        })
        .collect()
}

fn wrap(mut line: Line<'static>, columns: usize, indent: usize, words: bool) -> Vec<Line<'static>> {
    if columns == 0 {
        return vec![line];
    }
    let spans = std::mem::take(&mut line.spans);
    let widest = spans
        .iter()
        .flat_map(|span| span.content.graphemes(true))
        .map(width)
        .max()
        .unwrap_or(1)
        .min(columns);
    // Leave enough room after a hanging indent for a whole grapheme.
    let indent = indent.min(columns.saturating_sub(widest.max(1)));
    let mut lines = Vec::new();
    let mut current = line.clone();
    let mut used = 0;
    let next = |current: &mut Line<'static>, used: &mut usize, lines: &mut Vec<Line<'static>>| {
        lines.push(std::mem::replace(current, line.clone()));
        if indent > 0 {
            current.spans.push(Span::raw(" ".repeat(indent)));
        }
        *used = indent;
    };
    for span in spans {
        // Spaces are separate tokens so word wrapping can drop an overflowing
        // boundary space. Hard wrapping keeps every space in editable text.
        let tokens: Vec<&str> = if words {
            span.content
                .split_inclusive(' ')
                .flat_map(|token| match token.strip_suffix(' ') {
                    Some("") => vec![" "],
                    Some(word) => vec![word, " "],
                    None => vec![token],
                })
                .collect()
        } else {
            vec![span.content.as_ref()]
        };
        for token in tokens {
            let token_width = width(token);
            if words && token == " " && used + 1 > columns {
                if !current.spans.is_empty() {
                    next(&mut current, &mut used, &mut lines);
                }
                continue;
            }
            if words
                && token_width <= columns - indent
                && used + token_width > columns
                && !current.spans.is_empty()
            {
                next(&mut current, &mut used, &mut lines);
            }
            let mut remaining = token;
            while !remaining.is_empty() {
                let chunk = prefix(remaining, columns.saturating_sub(used));
                if !chunk.is_empty() {
                    current
                        .spans
                        .push(Span::styled(chunk.to_owned(), span.style));
                    used += width(chunk);
                    remaining = &remaining[chunk.len()..];
                } else if used > indent {
                    next(&mut current, &mut used, &mut lines);
                } else {
                    // A two-column grapheme cannot fit in a one-column pane.
                    // Show a replacement rather than overflowing or looping.
                    let grapheme = remaining.graphemes(true).next().unwrap();
                    current.spans.push(Span::styled("�", span.style));
                    used += 1;
                    remaining = &remaining[grapheme.len()..];
                }
                if !remaining.is_empty() && used >= columns {
                    next(&mut current, &mut used, &mut lines);
                }
            }
        }
    }
    if !current.spans.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests;
