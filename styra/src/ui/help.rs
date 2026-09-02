//! Full-screen keyboard shortcut reference.

use super::palette;
use crate::keymap::{ReferenceRow, CLOSE_REFERENCE, REFERENCE};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// The reference is longer than a short terminal, so `scroll` says how far
/// down it the operator has moved. [`reference_height`] is what bounds that.
pub(crate) fn render_keybinds(frame: &mut Frame, area: Rect, scroll: u16) {
    let heading = Style::default()
        .fg(palette::ACCENT)
        .add_modifier(Modifier::BOLD);
    let key = Style::default()
        .fg(palette::WARNING)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(palette::MUTED_TEXT);

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
        format!(" j/k to scroll · {CLOSE_REFERENCE} to close "),
        muted,
    )));

    let visible = area.height.saturating_sub(2);
    let scroll = scroll.min(reference_height().saturating_sub(visible));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" styra · keybinds ");
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

/// How many lines the reference occupies: every row, plus the blank line and
/// the closing hint appended after them.
pub(crate) fn reference_height() -> u16 {
    REFERENCE.len() as u16 + 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen_at(scroll: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal
            .draw(|frame| render_keybinds(frame, frame.area(), scroll))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    /// The reference is longer than the screen it is drawn on, so every
    /// section has to be reachable by scrolling — otherwise the ones at the
    /// end are documented nowhere the operator can see.
    #[test]
    fn scrolling_reaches_the_sections_past_the_fold() {
        let top = screen_at(0);
        assert!(top.contains("Global"), "{top}");
        assert!(!top.contains("Launch and selection screens"), "{top}");

        let bottom = screen_at(reference_height());
        assert!(bottom.contains("Launch and selection screens"), "{bottom}");
        assert!(bottom.contains("Typed answer"), "{bottom}");
    }

    /// Scrolling past the end stops at it rather than emptying the panel.
    #[test]
    fn the_scroll_stops_at_the_last_line() {
        assert_eq!(screen_at(reference_height()), screen_at(u16::MAX));
    }

    /// Everything the reference documents has to be legible somewhere in it.
    /// Checked across the whole scroll range, since it no longer fits on one
    /// screen — what matters is that it is reachable, not that it is on top.
    #[test]
    fn reference_groups_the_available_keybinds() {
        let screen = format!("{}{}", screen_at(0), screen_at(reference_height()));

        for expected in [
            "Global",
            "Events and previews",
            "Raw, log, quota, and transcript",
            "Driva (launch policy",
            "choose Driva templates",
            "Message editor",
            "Launch and selection screens",
            "live interactions",
            "Ctrl+L",
            "z R / z M",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}: {screen}");
        }
    }
}
