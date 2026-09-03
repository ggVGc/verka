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
        .workspace
        .working_directory_or_current()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let worktrees = format!(
        " W worktrees: {} ",
        if app.workspace.worktrees_enabled {
            "ON"
        } else {
            "OFF"
        }
    );
    let worktrees_width = worktrees.width().min(area.width as usize) as u16;
    let directory_width = working_directory
        .width()
        .min(area.width.saturating_sub(worktrees_width) as usize) as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(worktrees_width),
            Constraint::Length(directory_width),
        ])
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
    let worktrees = Paragraph::new(Line::from(Span::styled(
        worktrees,
        Style::default().fg(if app.workspace.worktrees_enabled {
            palette::SUCCESS
        } else {
            palette::INACTIVE
        }),
    )))
    .right_aligned();
    frame.render_widget(keybinds, chunks[0]);
    frame.render_widget(worktrees, chunks[1]);
    frame.render_widget(directory, chunks[2]);
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
    use super::super::testing;
    use super::super::testing::rendered;

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn footer_shows_keybinds_and_working_directory() {
        let mut app = testing::app("s1");
        app.workspace.enter("/tmp/styra/workspace".into());
        let screen = rendered(&app);
        assert!(screen.contains("? keybinds"));
        assert!(screen.contains("/tmp/styra/workspace"));
        assert!(screen.contains("W worktrees: OFF"));
        assert!(!screen.contains("j/k next/prev"));
    }

    #[test]
    fn footer_makes_enabled_worktree_creation_visible() {
        let mut app = testing::app("s1");
        app.workspace.worktrees_enabled = true;

        assert!(rendered(&app).contains("W worktrees: ON"));
    }

    #[test]
    fn working_directory_is_aligned_to_the_bottom_right() {
        let mut app = testing::app("s1");
        app.workspace.enter("/workspace".into());
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
