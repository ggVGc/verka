use super::*;
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

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
        LogLevel::Info => ("info ", theme::MUTED_TEXT),
        LogLevel::Warn => ("warn ", theme::WARNING),
        LogLevel::Error => ("error", theme::ERROR),
    };
    Line::from(vec![
        Span::styled(
            format!("{label} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            entry.message.to_owned(),
            Style::default().fg(match entry.level {
                LogLevel::Info => theme::TEXT,
                _ => color,
            }),
        ),
    ])
}

#[cfg(test)]
mod tests {
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
}
