//! File tree and prepared-content presentation.

use crate::palette;
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
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Span::styled(
            format!(" {scope} "),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    if files.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "no files mentioned by this entry",
                Style::default().fg(palette::MUTED_TEXT),
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
                    .fg(palette::ACCENT)
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
                    Style::default().fg(palette::MUTED_TEXT),
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
                .fg(palette::TEXT)
                .bg(palette::SELECTION_BACKGROUND)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette::TEXT)
        };
        let marker_style = if index == selected {
            Style::default()
                .fg(palette::SELECTION_MARKER)
                .bg(palette::SELECTION_BACKGROUND)
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

pub fn render_content(
    frame: &mut Frame,
    title: Option<&str>,
    content: FileContent<'_>,
    area: Rect,
) {
    let (title, text) = match content {
        FileContent::None => (
            " file preview ".into(),
            Text::from(Line::from(Span::styled(
                "no file selected",
                Style::default().fg(palette::MUTED_TEXT),
            ))),
        ),
        FileContent::Empty => (
            format!(" {} ", title.unwrap_or_default()),
            Text::from(Line::from(Span::styled(
                "(empty file)",
                Style::default().fg(palette::MUTED_TEXT),
            ))),
        ),
        FileContent::Ready(content) => (
            format!(" {} ", title.unwrap_or_default()),
            Text::from(content.replace('\t', "    ")),
        ),
        FileContent::Failed(error) => (
            format!(" {} ", title.unwrap_or_default()),
            Text::from(Line::from(Span::styled(
                format!("could not read file: {error}"),
                Style::default().fg(palette::ERROR),
            ))),
        ),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(Span::styled(title, Style::default().fg(palette::ACCENT)));
    frame.render_widget(
        Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        backend::TestBackend,
        layout::{Constraint, Layout},
        Terminal,
    };
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
}
