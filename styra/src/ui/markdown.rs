//! Markdown-to-Ratatui styling for agent messages.
//!
//! Markdown syntax is parsed by `pulldown-cmark`; this module only maps the
//! parser's semantic events onto Styra's terminal palette.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Renders one line of an agent message's markdown as styled spans.
pub(crate) fn markdown_line_spans(line: &str, base_style: Style) -> Vec<Span<'static>> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let mut spans = vec![Span::styled(indent.to_owned(), base_style)];
    spans.extend(render_spans(trimmed, base_style, true));
    spans
}

/// Renders inline Markdown used in compact, single-line event summaries.
pub(crate) fn parse_inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let spans = render_spans(text, base_style, false);
    if spans.is_empty() {
        vec![Span::styled(String::new(), base_style)]
    } else {
        spans
    }
}

fn render_spans(text: &str, base_style: Style, render_blocks: bool) -> Vec<Span<'static>> {
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    let mut spans = Vec::new();
    let mut styles = vec![base_style];
    let mut list_depth = 0usize;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { .. } if render_blocks => styles.push(
                    current_style(&styles)
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Tag::Strong => styles.push(current_style(&styles).add_modifier(Modifier::BOLD)),
                Tag::Emphasis => styles.push(current_style(&styles).add_modifier(Modifier::ITALIC)),
                Tag::Strikethrough => {
                    styles.push(current_style(&styles).add_modifier(Modifier::CROSSED_OUT))
                }
                Tag::List(_) if render_blocks => list_depth += 1,
                Tag::Item if render_blocks => spans.push(Span::styled(
                    format!("{}• ", "  ".repeat(list_depth.saturating_sub(1))),
                    current_style(&styles),
                )),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) | TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => {
                    if styles.len() > 1 {
                        styles.pop();
                    }
                }
                TagEnd::List(_) if render_blocks => list_depth = list_depth.saturating_sub(1),
                _ => {}
            },
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

    #[test]
    fn renders_heading_list_and_inline_styles() {
        let base = Style::default().fg(Color::White);

        let heading = markdown_line_spans("  # Hello **world**", base);
        assert_eq!(rendered(&heading), "  Hello world");
        assert_eq!(heading[1].style.fg, Some(Color::Cyan));
        assert!(heading[2].style.add_modifier.contains(Modifier::BOLD));

        let list = markdown_line_spans("- use `cargo test`", base);
        assert_eq!(rendered(&list), "• use cargo test");
        assert_eq!(list.last().unwrap().style.fg, Some(Color::Yellow));
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
}
