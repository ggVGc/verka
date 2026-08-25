//! Full-screen keyboard shortcut reference.

use crate::keymap::{ReferenceRow, CLOSE_REFERENCE, REFERENCE};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub(crate) fn render_keybinds(frame: &mut Frame, area: Rect) {
    let heading = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let key = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(Color::Gray);

    let section = |name: &'static str| Line::from(Span::styled(name, heading));
    let bindings = |keys: &'static str, action: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {keys:<20}"), key),
            Span::raw(action),
        ])
    };

    let mut lines = REFERENCE
        .iter()
        .map(|row| match row {
            ReferenceRow::Section(name) => section(name),
            ReferenceRow::Binding { keys, action } => bindings(keys, action),
            ReferenceRow::Blank => Line::default(),
        })
        .collect::<Vec<_>>();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(" {CLOSE_REFERENCE} to close "),
        muted,
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" styra · keybinds ");
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn reference_groups_the_available_keybinds() {
        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal
            .draw(|frame| render_keybinds(frame, frame.area()))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        for expected in [
            "Global",
            "Events and previews",
            "Raw, log, and transcript",
            "Driva (launch policy",
            "choose Driva templates",
            "Message editor",
            "Launch and selection screens",
            "current sessions",
            "Ctrl+L",
            "z R / z M",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}: {screen}");
        }
    }
}
