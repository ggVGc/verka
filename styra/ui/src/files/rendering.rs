use super::*;
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::BTreeSet;

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
mod tests;
