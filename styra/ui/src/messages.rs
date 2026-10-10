use crate::log::LogLevel;
use crate::theme;
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

pub struct MessageView<'a> {
    pub text: &'a str,
    pub level: LogLevel,
}

pub fn render(frame: &mut Frame, messages: &[MessageView<'_>], area: Rect) {
    let lines = messages.iter().map(|message| {
        let color = match message.level {
            LogLevel::Info => theme::TEXT,
            LogLevel::Warn => theme::WARNING,
            LogLevel::Error => theme::ERROR,
        };
        Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(message.text, Style::default().fg(color)),
        ])
    });
    let panel = Paragraph::new(lines.collect::<Vec<_>>()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::INACTIVE))
            .title(" Status "),
    );
    frame.render_widget(panel, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, style::Color, Terminal};

    #[test]
    fn message_text_and_marker_use_their_severity_color() {
        let messages = [
            MessageView {
                text: "failure",
                level: LogLevel::Error,
            },
            MessageView {
                text: "warning",
                level: LogLevel::Warn,
            },
            MessageView {
                text: "success",
                level: LogLevel::Info,
            },
        ];
        let mut terminal = Terminal::new(TestBackend::new(24, 5)).unwrap();
        terminal
            .draw(|frame| render(frame, &messages, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        for (row, color) in [(1, Color::Red), (2, Color::Yellow), (3, theme::TEXT)] {
            for column in [1, 3, 9] {
                assert_eq!(buffer[(column, row)].fg, color);
            }
        }
    }
}
