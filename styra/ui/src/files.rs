//! File tree and prepared-content presentation.

use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::BTreeSet;

/// Display-ready path structure. Display strings are owned because converting
/// platform paths may be lossy; file contents remain separate and borrowed.
pub struct FileView {
    pub reported: String,
    pub root: String,
    pub relative: String,
    pub components: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileContent<'a> {
    None,
    Ready(&'a str),
    Empty,
    Failed(&'a str),
}

pub fn render_tree(
    frame: &mut Frame,
    files: &[FileView],
    selected: usize,
    scope: &str,
    area: Rect,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Span::styled(
            format!(" {scope} "),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    if files.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no files mentioned by this entry",
                Style::default().fg(theme::MUTED_TEXT),
            )))
            .block(block),
            area,
        );
        return;
    }
    let mut lines = Vec::new();
    let mut selected_line = 0usize;
    let mut last_root: Option<&str> = None;
    let mut shown_dirs = BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        if last_root != Some(file.root.as_str()) {
            if last_root.is_some() {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                file.root.clone(),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )));
            last_root = Some(&file.root);
        }
        let mut prefix = String::new();
        for (depth, component) in file
            .components
            .iter()
            .take(file.components.len().saturating_sub(1))
            .enumerate()
        {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            if shown_dirs.insert((file.root.clone(), prefix.clone())) {
                lines.push(Line::from(Span::styled(
                    format!("{}▾ {component}/", "  ".repeat(depth + 1)),
                    Style::default().fg(theme::MUTED_TEXT),
                )));
            }
        }
        let name = file
            .components
            .last()
            .map(String::as_str)
            .unwrap_or(&file.relative);
        let style = if index == selected {
            Style::default()
                .fg(theme::TEXT)
                .bg(theme::SELECTION_BACKGROUND)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT)
        };
        let marker_style = if index == selected {
            Style::default()
                .fg(theme::SELECTION_MARKER)
                .bg(theme::SELECTION_BACKGROUND)
        } else {
            style
        };
        lines.push(Line::from(vec![
            Span::styled("  ".repeat(file.components.len()), style),
            Span::styled(if index == selected { "›" } else { " " }, marker_style),
            Span::styled(format!(" {name}"), style),
        ]));
        if index == selected {
            selected_line = lines.len().saturating_sub(1);
        }
    }
    let scroll = selected_line
        .saturating_sub(usize::from(area.height.saturating_sub(2)))
        .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

/// Draw a file's content, scrolled `requested_scroll` lines down as far as it
/// goes. Where a marked line should start out is the caller's to choose.
pub fn render_content(
    frame: &mut Frame,
    title: Option<&str>,
    content: FileContent<'_>,
    marked_line: Option<u32>,
    requested_scroll: u16,
    area: Rect,
) -> crate::preview::PreviewFeedback {
    let (title, text) = match content {
        FileContent::None => (
            " file preview ".into(),
            Text::from(Line::from(Span::styled(
                "no file selected",
                Style::default().fg(theme::MUTED_TEXT),
            ))),
        ),
        FileContent::Empty => (
            format!(" {} ", title.unwrap_or_default()),
            Text::from(Line::from(Span::styled(
                "(empty file)",
                Style::default().fg(theme::MUTED_TEXT),
            ))),
        ),
        FileContent::Ready(content) => {
            let content = content.replace('\t', "    ");
            let language = file_language(title);
            let mut lines =
                crate::markdown::syntax_highlighted_code_lines(&content, language.as_deref(), "")
                    .unwrap_or_else(|| {
                        content
                            .lines()
                            .map(|line| Line::from(line.to_owned()))
                            .collect()
                    });
            if let Some(marked) = marked_line {
                let marked_index = usize::try_from(marked.saturating_sub(1)).unwrap_or(usize::MAX);
                lines = lines
                    .into_iter()
                    .enumerate()
                    .map(|(index, line)| {
                        let selected = index == marked_index;
                        let style = if selected {
                            Style::default().bg(theme::SELECTION_BACKGROUND)
                        } else {
                            Style::default()
                        };
                        let mut spans = vec![Span::styled(
                            format!("{}{:>5} │ ", if selected { "▶" } else { " " }, index + 1),
                            style.fg(if selected {
                                theme::SELECTION_MARKER
                            } else {
                                theme::MUTED_TEXT
                            }),
                        )];
                        spans.extend(line.spans);
                        Line::from(spans).style(line.style.patch(style))
                    })
                    .collect();
            }
            (
                format!(" {} ", title.unwrap_or_default()),
                Text::from(lines),
            )
        }
        FileContent::Failed(error) => (
            format!(" {} ", title.unwrap_or_default()),
            Text::from(Line::from(Span::styled(
                format!("could not read file: {error}"),
                Style::default().fg(theme::ERROR),
            ))),
        ),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::INACTIVE))
        .title(Span::styled(title, Style::default().fg(theme::ACCENT)));
    let inner = block.inner(area);
    let limit = crate::preview::preview_scroll_limit(&text.lines, inner.width, inner.height);
    let effective = requested_scroll.min(limit);
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((effective, 0)),
        area,
    );
    crate::preview::PreviewFeedback {
        limit,
        effective_scroll: effective,
        viewport: inner.height,
    }
}

/// Link targets can carry `:line[:column]`; strip those numeric suffixes
/// before using the shared filename-to-grammar lookup.
fn file_language(title: Option<&str>) -> Option<String> {
    let mut path = title?;
    for _ in 0..2 {
        let Some((before, suffix)) = path.rsplit_once(':') else {
            break;
        };
        if suffix.parse::<u32>().is_err() {
            break;
        }
        path = before;
    }
    crate::language::from_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        backend::TestBackend,
        buffer::Buffer,
        layout::{Constraint, Layout},
        Terminal,
    };

    fn draw_content(
        height: u16,
        title: &str,
        content: &str,
        target: Option<u32>,
        scroll: u16,
    ) -> (Buffer, crate::preview::PreviewFeedback) {
        let mut terminal = Terminal::new(TestBackend::new(40, height)).unwrap();
        let mut feedback = None;
        terminal
            .draw(|frame| {
                feedback = Some(render_content(
                    frame,
                    Some(title),
                    FileContent::Ready(content),
                    target,
                    scroll,
                    frame.area(),
                ));
            })
            .unwrap();
        (terminal.backend().buffer().clone(), feedback.unwrap())
    }

    fn screen(files: &[FileView], content: FileContent<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                let panes =
                    Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(frame.area());
                render_tree(frame, files, 0, "files · focused", panes[0]);
                render_content(
                    frame,
                    files.first().map(|file| file.reported.as_str()),
                    content,
                    None,
                    0,
                    panes[1],
                );
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn tree_renders_prepared_content_without_io() {
        let files = vec![FileView {
            reported: "src/main.rs".into(),
            root: "/workspace".into(),
            relative: "src/main.rs".into(),
            components: vec!["src".into(), "main.rs".into()],
        }];
        let output = screen(&files, FileContent::Ready("fn main() {}\n"));
        for expected in ["/workspace", "▾ src/", "› main.rs", "fn main() {}"] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
    }
    #[test]
    fn content_states_are_explicit() {
        assert!(screen(&[], FileContent::None).contains("no file selected"));
        assert!(screen(&[], FileContent::Empty).contains("(empty file)"));
        assert!(screen(&[], FileContent::Failed("denied")).contains("could not read file: denied"));
    }

    #[test]
    fn a_target_line_is_numbered_and_marked() {
        let (buffer, _) = draw_content(8, "src/main.rs:2", "first\nsecond\nthird", Some(2), 0);
        let output = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(output.contains("▶    2 │ second"), "{output}");
    }

    #[test]
    fn file_extensions_select_the_syntax_grammar_even_with_a_location_suffix() {
        assert_eq!(
            file_language(Some("src/main.rs:12")).as_deref(),
            Some("rust")
        );
        assert_eq!(
            file_language(Some("web/app.tsx:8:3")).as_deref(),
            Some("typescript")
        );
        assert_eq!(file_language(Some("lib/styra.ex:4")).as_deref(), Some("ex"));
        assert_eq!(file_language(Some("lib/styra.EX:4")).as_deref(), Some("ex"));
        assert_eq!(
            file_language(Some("src/main.RS:12")).as_deref(),
            Some("rust")
        );
        assert_eq!(file_language(Some("Makefile")).as_deref(), Some("Makefile"));
    }

    #[test]
    fn content_scrolls_as_asked_up_to_its_last_page() {
        let content = (1..=30).map(|n| format!("line {n}\n")).collect::<String>();
        let draw = |scroll| {
            let (buffer, feedback) = draw_content(12, "notes.txt", &content, None, scroll);
            let rows: Vec<String> = buffer
                .content()
                .chunks(40)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect())
                .collect();
            (feedback, rows)
        };

        let (feedback, rows) = draw(5);
        assert_eq!(feedback.effective_scroll, 5);
        assert_eq!(feedback.viewport, 10);
        assert_eq!(feedback.limit, 20);
        assert!(rows[1].contains("line 6"), "{rows:?}");

        let (feedback, rows) = draw(u16::MAX);
        assert_eq!(feedback.effective_scroll, 20);
        assert!(rows[10].contains("line 30"), "{rows:?}");
    }

    #[test]
    fn file_types_without_a_name_here_are_still_highlighted() {
        let content = "defmodule Styra do\n  def hello, do: :world\nend\n";
        let (buffer, _) = draw_content(6, "lib/styra.ex", content, None, 0);
        let colors: BTreeSet<_> = buffer
            .content()
            .iter()
            .filter(|cell| !cell.symbol().trim().is_empty() && !"│─┌┐└┘".contains(cell.symbol()))
            .map(|cell| format!("{:?}", cell.fg))
            .collect();
        assert!(colors.len() > 2, "{colors:?}");
    }
}
