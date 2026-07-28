//! The one-line footer pointing to the full keyboard shortcut reference.

use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub(crate) fn render_footer(frame: &mut Frame, _app: &App, area: Rect) {
    let footer = Paragraph::new(Line::from(Span::styled(
        " ? keybinds",
        Style::default().fg(Color::Gray),
    )));
    frame.render_widget(footer, area);
}

pub(crate) fn tag_color(tag: &str) -> Color {
    match tag {
        "agent" => Color::Rgb(211, 158, 96),
        "user" => Color::Rgb(115, 190, 137),
        "shell" => Color::Rgb(184, 124, 0),
        "tool" => Color::Magenta,
        "plan" | "files" => Color::Blue,
        "error" | "malformed" => Color::Red,
        _ => Color::DarkGray,
    }
}

/// A very light tint for conversational prose. These stay close to the
/// default foreground so messages are distinguishable without competing with
/// the stronger colors reserved for status and errors.
pub(crate) fn message_text_color(tag: &str) -> Color {
    match tag {
        "agent" => Color::Rgb(238, 219, 193),
        "user" => Color::Rgb(207, 233, 214),
        _ => Color::White,
    }
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
    fn footer_only_advertises_keybind_reference() {
        let app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        let screen = rendered(&app);
        assert!(screen.contains("? keybinds"));
        assert!(!screen.contains("j/k next/prev"));
    }
}
