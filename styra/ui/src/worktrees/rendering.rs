use super::*;
use crate::fuzzy_list::{render_fuzzy_list, FuzzyListView};
use crate::theme;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render_worktree_picker(frame: &mut Frame, view: &WorktreePickerView<'_>) {
    let detail_height = view.detail.len() as u16 + u16::from(view.notice.is_some()) + 2;
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(detail_height)])
        .split(frame.area());
    render_fuzzy_list(
        frame,
        &FuzzyListView {
            title: &format!(" styra · worktrees · {} ", view.scope),
            rows: view.rows,
            list: view.list,
            focused: true,
            empty_note: NO_ROWS,
            hint: " type to narrow · Enter open · ? keys ",
        },
        panes[0],
    );

    let mut lines: Vec<Line> = view
        .detail
        .iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(
                    format!(" {label:<10}"),
                    Style::default().fg(theme::MUTED_TEXT),
                ),
                Span::styled(value.clone(), Style::default().fg(theme::TEXT)),
            ])
        })
        .collect();
    if let Some(notice) = view.notice {
        lines.push(Line::from(Span::styled(
            format!(" {notice}"),
            Style::default()
                .fg(theme::WARNING)
                .add_modifier(Modifier::BOLD),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::INACTIVE)),
        ),
        panes[1],
    );
}

#[cfg(test)]
mod tests;
