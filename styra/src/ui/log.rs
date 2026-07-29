//! The log view: Styra's own diagnostic/stderr log for the current session.

use super::{title_line, workspace_title};
use crate::app::{App, Focus};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use styra_server::LogLevel;

pub(crate) fn render_log(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(&app.launch_label(), &app.status, Some("log")));
    if let Some(title) = workspace_title(app) {
        block = block.title(title);
    }

    if app.log.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no log entries yet",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
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
        LogLevel::Info => ("info ", Color::Gray),
        LogLevel::Warn => ("warn ", Color::Yellow),
        LogLevel::Error => ("error", Color::Red),
    };
    Line::from(vec![
        Span::styled(
            format!("{label} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(entry.message.clone(), Style::default().fg(Color::White)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| super::super::render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn log_view_shows_entries_with_levels() {
        use styra_server::LogEntry;
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_log(LogEntry::info("launching codex"));
        app.push_log(LogEntry::error("could not run the agent: bwrap missing"));
        app.toggle_log();
        let screen = rendered(&app);
        assert!(screen.contains("log"));
        assert!(screen.contains("info"));
        assert!(screen.contains("error"));
        assert!(screen.contains("bwrap missing"));
    }
}
