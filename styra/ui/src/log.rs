use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

pub enum LogLevel {
    Info,
    Warn,
    Error,
}

pub struct LogEntryView<'a> {
    pub level: LogLevel,
    pub message: &'a str,
}

pub fn render(
    frame: &mut Frame,
    block: Block<'static>,
    entries: &[LogEntryView<'_>],
    scroll_back: usize,
    area: Rect,
) {
    if entries.is_empty() {
        frame.render_widget(Paragraph::new("  no log entries yet").block(block), area);
        return;
    }
    let lines = entries.iter().map(line).collect::<Vec<_>>();
    let viewport = area.height.saturating_sub(2) as usize;
    let start = lines
        .len()
        .saturating_sub(viewport)
        .saturating_sub(scroll_back) as u16;
    frame.render_widget(Paragraph::new(lines).block(block).scroll((start, 0)), area);
}

fn line(entry: &LogEntryView<'_>) -> Line<'static> {
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
        Span::styled(entry.message.to_owned(), Style::default().fg(palette::TEXT)),
    ])
}
