//! Markdown-to-Ratatui styling for agent messages.
//!
//! Multi-line detail blocks are rendered by `tui-markdown`, which parses a
//! whole buffer at once and so can render tables, code fences, and other
//! multi-line constructs correctly. Single-line summaries stay on the
//! lighter-weight `pulldown-cmark`-based inline renderer below, since
//! `tui-markdown` has no single-line-only mode.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::StyleSheet;

/// Renders a detail block's full markdown buffer as styled lines, each
/// prefixed with `indent`.
pub(crate) fn markdown_block_lines(
    text: &str,
    base_style: Style,
    indent: &str,
) -> Vec<Line<'static>> {
    let normalized = force_hard_line_breaks(text);
    let rendered = tui_markdown::from_str_with_options(
        &normalized,
        &tui_markdown::Options::new(StyraStyleSheet),
    );
    rendered
        .lines
        .into_iter()
        .map(|line| {
            // tui-markdown puts some styling (heading color, blockquote color,
            // table borders) on the Line itself rather than on every Span, so
            // that base style has to be carried over explicitly.
            let line_style = line.style;
            let mut spans = vec![Span::styled(indent.to_owned(), base_style)];
            spans.extend(line.spans.into_iter().enumerate().map(|(i, span)| {
                let mut content = span.content.into_owned();
                // tui-markdown has no hook to customize the unordered-list
                // marker, so the "- " it hardcodes is swapped for a bullet
                // glyph here to match Styra's established look.
                if i == 0 {
                    if let Some(bulleted) = bulletize(&content) {
                        content = bulleted;
                    }
                }
                Span::styled(content, span.style)
            }));
            Line::from(spans).style(line_style)
        })
        .collect()
}

fn bulletize(content: &str) -> Option<String> {
    let indent = content.strip_suffix("- ")?;
    indent
        .chars()
        .all(|c| c == ' ')
        .then(|| format!("{indent}\u{2022} "))
}

/// Agent messages commonly separate paragraphs with a single `\n` rather
/// than a blank line, but CommonMark treats a single newline inside a
/// paragraph as a soft break that collapses to a space. Force it into a hard
/// break instead, so plain prose keeps one rendered line per source line.
/// Code fences and table rows are left untouched since their line structure
/// is already meaningful.
fn force_hard_line_breaks(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut fenced = vec![false; lines.len()];
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced[i] = true;
            in_fence = !in_fence;
        } else {
            fenced[i] = in_fence;
        }
    }

    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if i + 1 < lines.len() {
            let trimmed = line.trim();
            let next_is_blank = lines[i + 1].trim().is_empty();
            let is_table_row = trimmed.starts_with('|');
            if !fenced[i] && !is_table_row && !trimmed.is_empty() && !next_is_blank {
                out.push_str("  ");
            }
            out.push('\n');
        }
    }
    out
}

/// Styra's palette overrides for `tui-markdown`'s default style sheet.
///
/// Headings keep the library's default colors but drop the leading `#`
/// marker, since pretty mode strips markdown syntax rather than showing it
/// styled.
#[derive(Clone, Copy, Debug, Default)]
struct StyraStyleSheet;

impl StyleSheet for StyraStyleSheet {
    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code(&self) -> Style {
        Style::new().fg(Color::Yellow)
    }
}

/// Renders inline Markdown used in compact, single-line event summaries.
pub(crate) fn parse_inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let spans = render_spans(text, base_style);
    if spans.is_empty() {
        vec![Span::styled(String::new(), base_style)]
    } else {
        spans
    }
}

fn render_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    let mut spans = Vec::new();
    let mut styles = vec![base_style];

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Strong => styles.push(current_style(&styles).add_modifier(Modifier::BOLD)),
                Tag::Emphasis => styles.push(current_style(&styles).add_modifier(Modifier::ITALIC)),
                Tag::Strikethrough => {
                    styles.push(current_style(&styles).add_modifier(Modifier::CROSSED_OUT))
                }
                _ => {}
            },
            Event::End(TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough) => {
                if styles.len() > 1 {
                    styles.pop();
                }
            }
            Event::End(_) => {}
            Event::Text(text) => {
                spans.push(Span::styled(text.into_string(), current_style(&styles)))
            }
            Event::Code(code) => spans.push(Span::styled(
                code.into_string(),
                current_style(&styles).fg(Color::Yellow),
            )),
            Event::SoftBreak | Event::HardBreak => {
                spans.push(Span::styled(" ", current_style(&styles)))
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                spans.push(Span::styled(html.into_string(), current_style(&styles)))
            }
            _ => {}
        }
    }
    spans
}

fn current_style(styles: &[Style]) -> Style {
    styles.last().copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn rendered_line(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn supports_common_inline_markdown_and_literal_unmatched_markers() {
        let base = Style::default();
        let spans = parse_inline_spans("*italic* and ~~gone~~ and `open", base);

        assert_eq!(rendered(&spans), "italic and gone and `open");
        assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
        assert!(spans[2].style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn empty_inline_input_still_produces_a_span() {
        assert_eq!(parse_inline_spans("", Style::default()).len(), 1);
    }

    #[test]
    fn block_lines_strip_the_heading_marker_and_style_the_heading() {
        let base = Style::default().fg(Color::White);
        let lines = markdown_block_lines("# Title", base, "  ");

        assert_eq!(rendered_line(&lines[0]), "  Title");
        assert_eq!(lines[0].style.bg, Some(Color::Cyan));
        assert!(lines[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn block_lines_render_a_table_with_borders() {
        let base = Style::default();
        let lines = markdown_block_lines("| A | B |\n|---|---|\n| 1 | 2 |", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert!(rendered.iter().any(|line| line.contains('┌')));
        assert!(rendered.iter().any(|line| line.contains('│')));
        assert!(rendered
            .iter()
            .any(|line| line.contains('A') && line.contains('B')));
    }

    #[test]
    fn block_lines_keep_ordered_list_numbering() {
        let base = Style::default();
        let lines = markdown_block_lines("1. first\n2. second", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert!(rendered.iter().any(|line| line.starts_with("1. first")));
        assert!(rendered.iter().any(|line| line.starts_with("2. second")));
    }

    #[test]
    fn block_lines_use_a_bullet_glyph_for_unordered_items() {
        let base = Style::default();
        let lines = markdown_block_lines("- one\n- two", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert!(rendered.iter().any(|line| line.starts_with("\u{2022} one")));
        assert!(rendered.iter().any(|line| line.starts_with("\u{2022} two")));
        assert!(!rendered.iter().any(|line| line.contains("- ")));
    }

    #[test]
    fn block_lines_keep_one_line_per_source_line_without_blank_separators() {
        // Agent messages routinely separate lines with a single `\n`, not a
        // blank line. CommonMark's soft-break-to-space rule would otherwise
        // silently merge them into one rendered line.
        let base = Style::default();
        let lines = markdown_block_lines("hello\nworld", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert_eq!(rendered, vec!["hello".to_owned(), "world".to_owned()]);
    }

    #[test]
    fn block_lines_leave_fenced_code_content_untouched() {
        let base = Style::default();
        let lines = markdown_block_lines("```\nfn f() {}\n```", base, "");

        let rendered: Vec<String> = lines.iter().map(rendered_line).collect();
        assert!(rendered.iter().any(|line| line == "fn f() {}"));
    }
}
