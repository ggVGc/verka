//! A minimal inline markdown renderer for agent messages: `#` headings,
//! `- `/`* ` bullets, and inline `` `code` `` / `**bold**`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Renders one line of an agent message's markdown as styled spans.
pub(crate) fn markdown_line_spans(line: &str, base_style: Style) -> Vec<Span<'static>> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    if let Some(rest) = ["### ", "## ", "# "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))
    {
        let mut spans = vec![Span::styled(indent.to_owned(), base_style)];
        spans.extend(parse_inline_spans(
            rest,
            base_style.fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
        return spans;
    }
    if let Some(rest) = ["- ", "* "]
        .iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))
    {
        let mut spans = vec![Span::styled(format!("{indent}• "), base_style)];
        spans.extend(parse_inline_spans(rest, base_style));
        return spans;
    }
    let mut spans = vec![Span::styled(indent.to_owned(), base_style)];
    spans.extend(parse_inline_spans(trimmed, base_style));
    spans
}

enum InlineMarker {
    Code,
    Bold,
}

/// Splits `text` on `` `code` `` and `**bold**` markers, styling each run;
/// unmatched openers are left as literal text rather than swallowed.
pub(crate) fn parse_inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    loop {
        let code = rest.find('`').map(|i| (i, InlineMarker::Code));
        let bold = rest.find("**").map(|i| (i, InlineMarker::Bold));
        let next = match (code, bold) {
            (Some(c), Some(b)) if c.0 <= b.0 => Some(c),
            (Some(_), Some(b)) => Some(b),
            (Some(c), None) => Some(c),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some((start, marker)) = next else {
            if !rest.is_empty() {
                spans.push(Span::styled(rest.to_owned(), base_style));
            }
            break;
        };
        let marker_str = match marker {
            InlineMarker::Code => "`",
            InlineMarker::Bold => "**",
        };
        let inner_start = start + marker_str.len();
        let Some(close_off) = rest[inner_start..].find(marker_str) else {
            spans.push(Span::styled(rest.to_owned(), base_style));
            break;
        };
        if start > 0 {
            spans.push(Span::styled(rest[..start].to_owned(), base_style));
        }
        let inner = &rest[inner_start..inner_start + close_off];
        let inner_style = match marker {
            InlineMarker::Code => base_style.fg(Color::Yellow),
            InlineMarker::Bold => base_style.add_modifier(Modifier::BOLD),
        };
        spans.push(Span::styled(inner.to_owned(), inner_style));
        rest = &rest[inner_start + close_off + marker_str.len()..];
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }
    spans
}
