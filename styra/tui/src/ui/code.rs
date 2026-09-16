//! Styling for code detail blocks — commands, tool output, and diffs.
//!
//! The expanded list entry and the preview panes render the same blocks, so
//! both go through [`code_block_lines`]: a command highlighted in the list is
//! highlighted the same way under `p`, and a suspicious shell result is marked
//! in both places.

use super::palette;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Renders one code block as styled lines, each prefixed with `indent`.
///
/// `suspicious_shell` marks a command that reported success while printing an
/// error diagnostic; those output lines are colored as errors so the
/// contradiction is visible rather than buried in ordinary output.
pub(crate) fn code_block_lines(
    text: &str,
    language: Option<&str>,
    text_color: Color,
    suspicious_shell: bool,
    indent: &str,
) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| {
            if suspicious_shell && is_error_diagnostic(line) {
                return indented(line, palette::ERROR, indent);
            }
            if language == Some("bash") {
                let mut spans = vec![Span::styled(
                    indent.to_owned(),
                    Style::default().fg(palette::TEXT),
                )];
                spans.extend(bash_spans(&line.replace('\t', "    ")));
                return Line::from(spans);
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
            indented(line, color, indent)
        })
        .collect()
}

fn indented(line: &str, color: Color, indent: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("{indent}{}", line.replace('\t', "    ")),
        Style::default().fg(color),
    ))
}

/// Output that contradicts a reported success. Kept deliberately narrow: these
/// are diagnostics a shell or compiler emits in a fixed form, not any line that
/// happens to mention an error.
pub(crate) fn is_error_diagnostic(line: &str) -> bool {
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

    fn colors(line: &Line<'_>) -> Vec<Option<Color>> {
        line.spans.iter().map(|span| span.style.fg).collect()
    }

    #[test]
    fn bash_commands_are_highlighted_token_by_token() {
        let lines = code_block_lines(
            "grep -n 'needle' $FILE",
            Some("bash"),
            palette::TEXT,
            false,
            "  ",
        );

        let colors = colors(&lines[0]);
        assert!(colors.contains(&Some(palette::ACCENT)), "{colors:?}"); // -n
        assert!(colors.contains(&Some(palette::SUCCESS)), "{colors:?}"); // 'needle'
        assert!(colors.contains(&Some(palette::WARNING)), "{colors:?}"); // $FILE
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

        let fg: Vec<Option<Color>> = lines.iter().map(|line| line.spans[0].style.fg).collect();
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

        assert_eq!(lines[1].spans[0].style.fg, Some(palette::ERROR));
    }

    #[test]
    fn tabs_become_spaces_so_columns_do_not_collapse() {
        let lines = code_block_lines("a\tb", None, palette::TEXT, false, "");

        assert_eq!(lines[0].spans[0].content.as_ref(), "a    b");
    }
}
