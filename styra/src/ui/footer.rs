//! The one-line footer with the keyboard shortcut reference and workspace.

use super::palette;
use crate::app::App;
use crate::keymap::HELP;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub(crate) fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let working_directory = app
        .working_directory
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let directory_width = working_directory.width().min(area.width as usize) as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(directory_width)])
        .split(area);

    let keybinds = Paragraph::new(Line::from(Span::styled(
        format!(" {HELP} keybinds"),
        Style::default().fg(palette::MUTED_TEXT),
    )));
    let directory = Paragraph::new(Line::from(Span::styled(
        working_directory,
        Style::default().fg(palette::ADDITIONAL_INFO),
    )))
    .right_aligned();
    frame.render_widget(keybinds, chunks[0]);
    frame.render_widget(directory, chunks[1]);
}

pub(crate) fn tag_color(tag: &str) -> Color {
    match tag {
        "agent" => palette::AGENT_TAG,
        "user" => palette::USER_TAG,
        "shell" => palette::SHELL_TAG,
        "tool" => palette::SPECIAL,
        "plan" | "files" => palette::INFO,
        "error" | "malformed" => palette::ERROR,
        _ => palette::ADDITIONAL_INFO,
    }
}

/// A very light tint for conversational prose. These stay close to the
/// default foreground so messages are distinguishable without competing with
/// the stronger colors reserved for status and errors.
pub(crate) fn message_text_color(tag: &str) -> Color {
    match tag {
        "agent" => palette::AGENT_TEXT,
        "user" => palette::USER_TEXT,
        _ => palette::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app))
            .unwrap();
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
    fn footer_shows_keybinds_and_working_directory() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.set_workspace_root("/tmp/styra/workspace".into());
        let screen = rendered(&app);
        assert!(screen.contains("? keybinds"));
        assert!(screen.contains("/tmp/styra/workspace"));
        assert!(!screen.contains("j/k next/prev"));
    }

    #[test]
    fn working_directory_is_aligned_to_the_bottom_right() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.set_workspace_root("/workspace".into());
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer();

        let bottom_row: String = (0..40)
            .map(|x| buffer.cell((x, 9)).unwrap().symbol())
            .collect();
        assert!(bottom_row.ends_with("/workspace"));
    }
}
