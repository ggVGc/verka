use super::*;
use crate::theme;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

/// Draw a narrowing list into `area`: a bordered box titled `title`, the
/// query along its bottom border once there is one, and the matching rows
/// with their matched letters marked.
pub fn render_fuzzy_list(frame: &mut Frame, view: &FuzzyListView, area: Rect) {
    let border_style = if view.focused {
        Style::default().fg(theme::ACCENT)
    } else {
        Style::default().fg(theme::INACTIVE)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            view.title.to_owned(),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    // The query rides the bottom border rather than taking a row of its own:
    // the box is already as tall as the rows it has, and a list that shrinks
    // by one the moment it is typed at hides the very row being hunted.
    if view.list.is_filtering() {
        block = block.title_bottom(Line::from(vec![
            Span::styled(" /", Style::default().fg(theme::MUTED_TEXT)),
            Span::styled(
                format!("{} ", view.list.query),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if !view.hint.is_empty() {
        block = block.title_bottom(
            Line::from(Span::styled(
                view.hint.to_owned(),
                Style::default().fg(theme::MUTED_TEXT),
            ))
            .right_aligned(),
        );
    }

    let matches = view.list.matches(view.rows);
    if matches.is_empty() {
        let note = if view.rows.is_empty() {
            view.empty_note.to_owned()
        } else {
            format!("no match for {}", view.list.query)
        };
        let note = ListItem::new(Line::from(Span::styled(
            format!("  {note}"),
            Style::default()
                .fg(theme::MUTED_TEXT)
                .add_modifier(Modifier::DIM),
        )));
        frame.render_widget(List::new(vec![note]).block(block), area);
        return;
    }

    let selected = view.list.selected.min(matches.len() - 1);
    let items: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(row, found)| {
            let mut spans = vec![Span::styled(
                if row == selected { "• " } else { "  " },
                Style::default().fg(if row == selected {
                    theme::SELECTION_MARKER
                } else {
                    theme::TEXT
                }),
            )];
            spans.extend(marked(&view.rows[found.index], &found.positions));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// `row` cut into spans so the characters at `positions` carry the match
/// mark. The letters that were typed are the reason the row is on screen, so
/// they are what the eye should land on.
pub fn marked(row: &str, positions: &[usize]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    for (index, character) in row.chars().enumerate() {
        if positions.contains(&index) {
            if !plain.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut plain),
                    Style::default().fg(theme::TEXT),
                ));
            }
            spans.push(Span::styled(
                character.to_string(),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            plain.push(character);
        }
    }
    if !plain.is_empty() {
        spans.push(Span::styled(plain, Style::default().fg(theme::TEXT)));
    }
    spans
}

#[cfg(test)]
mod tests;
