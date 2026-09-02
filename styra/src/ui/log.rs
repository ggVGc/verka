//! The log view: Styra's own diagnostic/stderr log for the current session.

use super::{palette, render_placeholder, view_block};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use styra_server::LogLevel;

pub(crate) fn render_log(frame: &mut Frame, app: &App, area: Rect) {
    let block = view_block(app, Some("log"));

    if app.log.is_empty() {
        render_placeholder(frame, block, area, "  no log entries yet");
        return;
    }

    let lines: Vec<Line<'static>> = app.log.iter().map(log_line).collect();
    let viewport = area.height.saturating_sub(2) as usize;
    let max_start = lines.len().saturating_sub(viewport);
    let start = max_start.saturating_sub(app.log_scroll_back as usize) as u16;
    let paragraph = Paragraph::new(lines).block(block).scroll((start, 0));
    frame.render_widget(paragraph, area);
}

pub(crate) fn log_line(entry: &styra_server::LogEntry) -> Line<'static> {
    let (label, color) = match entry.level {
        LogLevel::Info => ("info ", palette::MUTED_TEXT),
        LogLevel::Warn => ("warn ", palette::WARNING),
        LogLevel::Error => ("error", palette::ERROR),
    };
    Line::from(vec![
        Span::styled(
            format!("{label} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(entry.message.clone(), Style::default().fg(palette::TEXT)),
    ])
}

#[cfg(test)]
mod tests {
    use super::super::testing::rendered;
    use super::*;
    use crate::app::View;

    #[test]
    fn log_view_shows_entries_with_levels() {
        use styra_server::LogEntry;
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_log(LogEntry::info("launching codex"));
        app.push_log(LogEntry::error("could not run the agent: bwrap missing"));
        app.toggle_view(View::Log);
        let screen = rendered(&app);
        assert!(screen.contains("log"));
        assert!(screen.contains("info"));
        assert!(screen.contains("error"));
        assert!(screen.contains("bwrap missing"));
    }
}
