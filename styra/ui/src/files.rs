//! File tree and prepared-content presentation.

use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::BTreeSet;
use std::path::Path;

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
    marked_line: Option<u32>,
    area: Rect,
) {
    let (title, text, scroll) = match content {
        FileContent::None => (
            " file preview ".into(),
            Text::from(Line::from(Span::styled(
                "no file selected",
                Style::default().fg(palette::MUTED_TEXT),
            ))),
            0,
        ),
        FileContent::Empty => (
            format!(" {} ", title.unwrap_or_default()),
            Text::from(Line::from(Span::styled(
                "(empty file)",
                Style::default().fg(palette::MUTED_TEXT),
            ))),
            0,
        ),
        FileContent::Ready(content) => {
            let content = content.replace('\t', "    ");
            let lines =
                crate::markdown::syntax_highlighted_code_lines(&content, file_language(title), "")
                    .unwrap_or_else(|| {
                        content
                            .lines()
                            .map(|line| Line::from(line.to_owned()))
                            .collect()
                    });
            if let Some(marked) = marked_line {
                let marked_index = usize::try_from(marked.saturating_sub(1)).unwrap_or(usize::MAX);
                let lines: Vec<_> = lines
                    .into_iter()
                    .enumerate()
                    .map(|(index, line)| {
                        let selected = index == marked_index;
                        let style = if selected {
                            Style::default().bg(palette::SELECTION_BACKGROUND)
                        } else {
                            Style::default()
                        };
                        let mut spans = vec![Span::styled(
                            format!("{}{:>5} │ ", if selected { "▶" } else { " " }, index + 1),
                            style.fg(if selected {
                                palette::SELECTION_MARKER
                            } else {
                                palette::MUTED_TEXT
                            }),
                        )];
                        spans.extend(line.spans);
                        Line::from(spans).style(line.style.patch(style))
                    })
                    .collect();
                let viewport = usize::from(area.height.saturating_sub(2));
                let scroll = marked_index
                    .saturating_sub(viewport / 2)
                    .min(usize::from(u16::MAX)) as u16;
                (
                    format!(" {} ", title.unwrap_or_default()),
                    Text::from(lines),
                    scroll,
                )
            } else {
                (
                    format!(" {} ", title.unwrap_or_default()),
                    Text::from(lines),
                    0,
                )
            }
        }
        FileContent::Failed(error) => (
            format!(" {} ", title.unwrap_or_default()),
            Text::from(Line::from(Span::styled(
                format!("could not read file: {error}"),
                Style::default().fg(palette::ERROR),
            ))),
            0,
        ),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(Span::styled(title, Style::default().fg(palette::ACCENT)));
    frame.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

/// The TextMate grammar name for a previewed filename. Link targets can carry
/// `:line[:column]`, so peel those numeric suffixes before asking `Path` for
/// its extension.
fn file_language(title: Option<&str>) -> Option<&'static str> {
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
    match Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "jsx" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "json" => Some("json"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" => Some("cpp"),
        "go" => Some("go"),
        "java" => Some("java"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "sql" => Some("sql"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "md" | "mdx" => Some("markdown"),
        "xml" => Some("xml"),
        _ => None,
    }
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
                    None,
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
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal
            .draw(|frame| {
                render_content(
                    frame,
                    Some("src/main.rs:2"),
                    FileContent::Ready("first\nsecond\nthird"),
                    Some(2),
                    frame.area(),
                );
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(output.contains("▶    2 │ second"), "{output}");
    }

    #[test]
    fn file_extensions_select_the_syntax_grammar_even_with_a_location_suffix() {
        assert_eq!(file_language(Some("src/main.rs:12")), Some("rust"));
        assert_eq!(file_language(Some("web/app.tsx:8:3")), Some("typescript"));
        assert_eq!(file_language(Some("notes.txt")), None);
    }
}
