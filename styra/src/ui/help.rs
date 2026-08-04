//! Full-screen keyboard shortcut reference.

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

    let section = |name| Line::from(Span::styled(name, heading));
    let bindings = |keys: &'static str, action: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {keys:<20}"), key),
            Span::raw(action),
        ])
    };

    let lines = vec![
        section("Global"),
        bindings("?", "show/close this reference"),
        bindings("i / Esc", "focus message / return to list"),
        bindings("q", "quit"),
        bindings("s", "interrupt active turn"),
        bindings("S", "stop interaction"),
        bindings("n / N", "new session / stop and start new session"),
        bindings("L", "choose model for an idle agent turn"),
        bindings("!", "open session shell in a new terminal"),
        bindings(
            "a / A / V; E",
            "current sessions/interactions/Workspaces; notes",
        ),
        bindings(
            "r / l / t / d",
            "raw / log / transcript / driva; press again for events",
        ),
        bindings("f", "files mentioned by the focused entry"),
        Line::default(),
        section("Events and previews"),
        bindings("J/K or ↓/↑", "next/previous entry"),
        bindings("j/k", "next/previous line"),
        bindings("g/G", "first/last entry"),
        bindings("Space, Enter, o", "toggle selected entry"),
        bindings("O / C", "expand selected / show expanded conversation"),
        bindings("z R / z M", "expand all / collapse all"),
        bindings("m / p", "toggle minor events / preview panel"),
        bindings("c", "toggle conversation-only events"),
        bindings("P", "toggle full-screen preview"),
        bindings("v", "toggle pretty/diff preview"),
        bindings("PgUp/PgDn", "scroll preview"),
        Line::default(),
        section("Raw, log, and transcript"),
        bindings("j/k or ↓/↑", "move or scroll"),
        bindings("g/G", "first/top or last/bottom"),
        bindings("PgUp/PgDn", "scroll raw-line preview"),
        Line::default(),
        section("Files"),
        bindings("j/k or ↓/↑", "next/previous file"),
        bindings("J/K", "next/previous interaction-log entry"),
        bindings("e", "open selected file in editor"),
        bindings("p", "toggle interaction preview"),
        bindings("a", "toggle focused-entry/all-session files"),
        Line::default(),
        section("Message editor"),
        bindings("Enter", "send message"),
        bindings("Alt+Enter", "insert newline"),
        bindings("↑/↓", "older/newer message history"),
        bindings("Ctrl+W", "delete previous word"),
        bindings(
            "Ctrl+L",
            "choose model before first message or idle agent turn",
        ),
        Line::default(),
        section("Launch and selection screens"),
        bindings("j/k or ↓/↑", "move selection"),
        bindings("Tab, h/l, ←/→", "move launch column"),
        bindings("Enter", "select"),
        bindings("D", "select and save launch default"),
        bindings("Esc or q", "cancel"),
        Line::default(),
        Line::from(Span::styled(" ?, Esc, or q to close ", muted)),
    ];

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
        let mut terminal = Terminal::new(TestBackend::new(100, 48)).unwrap();
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
