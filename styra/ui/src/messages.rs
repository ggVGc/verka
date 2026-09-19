use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn height(messages: usize) -> u16 {
    if messages == 0 {
        0
    } else {
        (messages as u16).saturating_add(2)
    }
}

pub fn render(frame: &mut Frame, messages: &[String], area: Rect) {
    let lines = messages.iter().map(|message| {
        Line::from(vec![
            Span::styled("● ", Style::default().fg(palette::ACCENT)),
            Span::styled(message.clone(), Style::default().fg(palette::TEXT)),
        ])
    });
    let panel = Paragraph::new(lines.collect::<Vec<_>>()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::INACTIVE))
            .title(" Status "),
    );
    frame.render_widget(panel, area);
}
