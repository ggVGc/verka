use super::*;
use ratatui::style::Color;

#[test]
fn severity_colors_the_message_as_well_as_the_label() {
    for (level, color) in [
        (LogLevel::Error, Color::Red),
        (LogLevel::Warn, Color::Yellow),
    ] {
        let rendered = line(&LogEntryView {
            level,
            message: "details",
        });
        assert!(rendered
            .spans
            .iter()
            .all(|span| span.style.fg == Some(color)));
    }
}
