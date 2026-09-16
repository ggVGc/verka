//! Short-lived status messages shown below the main event area.

use super::palette;
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub(crate) fn message_area_height(app: &App) -> u16 {
    if app.notices.is_empty() {
        0
    } else {
        (app.notices.len() as u16).saturating_add(2)
    }
}

pub(crate) fn render_messages(frame: &mut Frame, app: &App, area: Rect) {
    let lines = app.notices.iter().map(|message| {
        Line::from(vec![
            Span::styled("● ", Style::default().fg(palette::ACCENT)),
            Span::styled(message.text.clone(), Style::default().fg(palette::TEXT)),
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

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn panel_grows_by_one_row_for_each_message() {
        let mut app = testing::app("s1");
        assert_eq!(message_area_height(&app), 0);

        app.show_action_message("first");
        assert_eq!(message_area_height(&app), 3);
        app.show_action_message("second");
        assert_eq!(message_area_height(&app), 4);

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("● first"));
        assert!(screen.contains("● second"));
    }
}
