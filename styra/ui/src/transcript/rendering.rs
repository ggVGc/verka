use super::*;
use crate::chrome::{all_events_title, panel_block, uncommitted_title};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub fn render(frame: &mut Frame, view: &TranscriptView<'_>, area: Rect) -> u16 {
    let mut block = panel_block(&view.chrome);
    if view.all_events {
        block = block.title_bottom(all_events_title(view.show_minor));
    }
    if view.uncommitted_changes {
        block = uncommitted_title(block);
    }
    if !view.has_entries {
        frame.render_widget(Paragraph::new("  nothing to render yet").block(block), area);
        return 0;
    }
    let lines = view
        .text
        .lines()
        .map(|line| {
            Line::from(Span::styled(
                line.to_owned(),
                Style::default().fg(theme::TEXT),
            ))
        })
        .collect::<Vec<_>>();
    let viewport = area.height.saturating_sub(2) as usize;
    let limit = lines.len().saturating_sub(viewport) as u16;
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((view.requested_scroll.min(limit), 0)),
        area,
    );
    limit
}

#[cfg(test)]
mod tests;
