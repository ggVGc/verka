//! Styling for code detail blocks — commands, tool output, and diffs.
//!
//! The expanded list entry and the preview panes render the same blocks, so
//! both go through [`code_block_lines`]: a command highlighted in the list is
//! highlighted the same way under `p`, and a suspicious shell result is marked
//! in both places.

use super::{markdown::syntax_highlighted_code_lines, palette};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// The left edge drawn down a code block, marking where it starts and ends
/// without needing to know the width it will be rendered at.
const GUTTER: &str = "│ ";

/// Renders one code block as styled lines, each prefixed with `indent` and a
/// gutter that sets the block off from the surrounding text.
///
/// `suspicious_shell` marks a command that reported success while printing an
/// error diagnostic; those output lines are colored as errors so the
/// contradiction is visible rather than buried in ordinary output.
pub fn code_block_lines(
    text: &str,
    language: Option<&str>,
    text_color: Color,
    suspicious_shell: bool,
    indent: &str,
) -> Vec<Line<'static>> {
    body_lines(text, language, text_color, suspicious_shell)
        .into_iter()
        .map(|line| with_gutter(line, indent))
        .collect()
}

/// Prepends the indent and gutter to one already-styled code line.
///
/// The gutter is the same on every row rather than capped with corners: an
/// inline block can be truncated to a line cap mid-way, and a block that had
/// lost its closing corner would read as unfinished code.
fn with_gutter(line: Line<'static>, indent: &str) -> Line<'static> {
    let line_style = line.style;
    let mut spans = vec![Span::styled(
        format!("{indent}{GUTTER}"),
        Style::default().fg(palette::INACTIVE),
    )];
    spans.extend(line.spans);
    Line::from(spans).style(line_style)
}

/// The block's content, without the gutter.
fn body_lines(
    text: &str,
    language: Option<&str>,
    text_color: Color,
    suspicious_shell: bool,
) -> Vec<Line<'static>> {
    // Agent-message fences become `DetailBlock::Code` before they reach the
    // UI. Feed recognized languages back through the shared TextMate renderer
    // so they receive the same theme as fenced Markdown elsewhere.
    if !suspicious_shell {
        if let Some(lines) = syntax_highlighted_code_lines(text, language, "") {
            return lines;
        }
    }

    text.lines()
        .map(|line| {
            if suspicious_shell && is_error_diagnostic(line) {
                return plain(line, palette::ERROR);
            }
            if language == Some("bash") {
                return Line::from(bash_spans(&line.replace('\t', "    ")));
            }
            let color = if line.starts_with('+') && !line.starts_with("+++") {
                palette::SUCCESS
            } else if line.starts_with('-') && !line.starts_with("---") {
                palette::ERROR
            } else if line.starts_with("@@") {
                palette::ACCENT
            } else {
                text_color
            };
            plain(line, color)
        })
        .collect()
}

fn plain(line: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        line.replace('\t', "    "),
        Style::default().fg(color),
    ))
}

/// Output that contradicts a reported success. Kept deliberately narrow: these
/// are diagnostics a shell or compiler emits in a fixed form, not any line that
/// happens to mention an error.
pub fn is_error_diagnostic(line: &str) -> bool {
    let line = line.trim_start().to_ascii_lowercase();
    line.starts_with("error:")
        || line.starts_with("error[")
        || line.starts_with("fatal:")
        || line.contains(": no such file or directory")
        || line.contains(": permission denied")
        || line.contains(": read-only file system")
        || line.ends_with(": command not found")
}

/// Small shell highlighter for command previews. Genta identifies the code as
/// Bash; Styra owns the terminal palette.
fn bash_spans(line: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with('#') {
            spans.push(Span::styled(
                rest.to_owned(),
                Style::default().fg(palette::ADDITIONAL_INFO),
            ));
            break;
        }
        let first = rest.chars().next().unwrap();
        let (len, color) = if first == '\'' || first == '"' {
            let end = rest[1..]
                .find(first)
                .map(|offset| offset + 2)
                .unwrap_or(rest.len());
            (end, palette::SUCCESS)
        } else if first.is_whitespace() {
            (
                rest.find(|ch: char| !ch.is_whitespace())
                    .unwrap_or(rest.len()),
                palette::TEXT,
            )
        } else {
            let end = rest
                .find(|ch: char| ch.is_whitespace() || "|&;<>".contains(ch))
                .unwrap_or(rest.len());
            if end == 0 {
                (first.len_utf8(), palette::SPECIAL)
            } else {
                let token = &rest[..end];
                let color = if token.starts_with('-') {
                    palette::ACCENT
                } else if token.contains('$') {
                    palette::WARNING
                } else {
                    palette::TEXT
                };
                (end, color)
            }
        };
        spans.push(Span::styled(
            rest[..len].to_owned(),
            Style::default().fg(color),
        ));
        rest = &rest[len..];
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colors of one line's content, past the gutter.
    fn colors(line: &Line<'_>) -> Vec<Option<Color>> {
        line.spans[1..].iter().map(|span| span.style.fg).collect()
    }

    /// One line's first content span, past the gutter.
    fn content<'a>(line: &'a Line<'_>) -> &'a Span<'a> {
        &line.spans[1]
    }

    #[test]
    fn every_row_of_a_block_carries_the_gutter() {
        let lines = code_block_lines("one\ntwo", None, palette::TEXT, false, "  ");

        for line in &lines {
            assert_eq!(line.spans[0].content.as_ref(), "  │ ");
            assert_eq!(line.spans[0].style.fg, Some(palette::INACTIVE));
        }
    }

    #[test]
    fn a_highlighted_block_carries_the_gutter_too() {
        let lines = code_block_lines("fn main() {}", Some("rust"), palette::TEXT, false, "  ");

        assert_eq!(lines[0].spans[0].content.as_ref(), "  │ ");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette::INACTIVE));
    }

    #[test]
    fn bash_blocks_use_the_markdown_syntax_theme() {
        let lines = code_block_lines(
            "grep -n 'needle' $FILE",
            Some("bash"),
            palette::TEXT,
            false,
            "  ",
        );

        let colors = colors(&lines[0]);
        assert!(
            colors.contains(&Some(palette::ADDITIONAL_INFO)),
            "{colors:?}"
        ); // command / option
        assert!(colors.len() > 2, "{colors:?}"); // tokenized, not one plain span
    }

    #[test]
    fn rust_blocks_use_the_markdown_syntax_theme() {
        let lines = code_block_lines("fn main() {}", Some("rust"), palette::TEXT, false, "  ");

        let keyword = lines[0]
            .spans
            .iter()
            .find(|span| span.content == "fn")
            .expect("Rust keyword span");
        assert_eq!(keyword.style.fg, Some(palette::MARKDOWN_CODE_KEYWORD));
    }

    #[test]
    fn diff_lines_are_colored_by_their_marker() {
        let lines = code_block_lines(
            "@@ -1 +1 @@\n-gone\n+added\n--- a/f\n+++ b/f",
            None,
            palette::TEXT,
            false,
            "",
        );

        let fg: Vec<Option<Color>> = lines.iter().map(|line| content(line).style.fg).collect();
        assert_eq!(
            fg,
            vec![
                Some(palette::ACCENT),
                Some(palette::ERROR),
                Some(palette::SUCCESS),
                // File headers are not additions or removals.
                Some(palette::TEXT),
                Some(palette::TEXT),
            ]
        );
    }

    #[test]
    fn a_suspicious_success_colors_its_diagnostic_even_in_a_bash_block() {
        let lines = code_block_lines(
            "ls missing\nls: no such file or directory",
            Some("bash"),
            palette::TEXT,
            true,
            "",
        );

        assert_eq!(content(&lines[1]).style.fg, Some(palette::ERROR));
    }

    #[test]
    fn tabs_become_spaces_so_columns_do_not_collapse() {
        let lines = code_block_lines("a\tb", None, palette::TEXT, false, "");

        assert_eq!(content(&lines[0]).content.as_ref(), "a    b");
    }
}
