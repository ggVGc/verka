use crate::tag_picker::TagPicker;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::palette;

pub(crate) fn render(frame: &mut Frame, picker: &TagPicker) {
    let area = frame.area();
    let height = (picker.available.len() as u16 + 6).min(area.height.saturating_sub(2));
    let width = area.width.saturating_sub(8).min(56);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" Interaction tags · Space toggle · n new · Enter save · Esc cancel ");
    let inside = block.inner(popup);
    frame.render_widget(block, popup);
    let rows = picker
        .available
        .iter()
        .map(|tag| {
            let selected = picker.selected.contains(tag);
            ListItem::new(Line::from(vec![
                Span::styled(
                    if selected { " [x] " } else { " [ ] " },
                    Style::default().fg(palette::ACCENT),
                ),
                Span::raw(tag.clone()),
            ]))
        })
        .collect::<Vec<_>>();
    let input = picker
        .new_tag
        .as_ref()
        .map(|value| format!("new tag: {value} · Enter add & save"))
        .unwrap_or_else(|| "n adds a new tag".into());
    let list_area = Rect::new(
        inside.x,
        inside.y,
        inside.width,
        inside.height.saturating_sub(1),
    );
    let input_area = Rect::new(
        inside.x,
        inside.y + inside.height.saturating_sub(1),
        inside.width,
        1,
    );
    let list = List::new(rows).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select((!picker.available.is_empty()).then_some(picker.cursor));
    frame.render_stateful_widget(list, list_area, &mut state);
    frame.render_widget(
        Paragraph::new(input).style(Style::default().fg(palette::SUBORDINATE_TEXT)),
        input_area,
    );
}
