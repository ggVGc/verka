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
