//! Short-lived status messages shown below the main event area.

use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub(crate) fn message_area_height(app: &App) -> u16 {
    if app.action_messages.is_empty() {
        0
    } else {
        (app.action_messages.len() as u16).saturating_add(2)
    }
}

pub(crate) fn render_messages(frame: &mut Frame, app: &App, area: Rect) {
    let lines = app.action_messages.iter().map(|message| {
        Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Cyan)),
            Span::styled(message.text.clone(), Style::default().fg(Color::White)),
        ])
    });
    let panel = Paragraph::new(lines.collect::<Vec<_>>()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Status "),
    );
    frame.render_widget(panel, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn panel_grows_by_one_row_for_each_message() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
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
