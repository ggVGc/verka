//! The one-line status/help footer: a bar of keyboard hints that depends on
//! focus and, once a session is launched, the current view.

use crate::app::{Focus, View};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

pub(crate) fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    // The launch keys are only listed while they do something: once a session
    // is launched, its agent and model are settled and `L` is inert.
    if app.can_configure_launch() {
        let hints = match app.focus {
            Focus::Input => "Enter send (starts the agent) · ↑/↓ history · Ctrl+L choose default agent/model/effort · Alt+Enter newline · Esc back to list",
            Focus::List => "L choose default agent/model/effort · i message · A interactions · V Workspaces · q quit",
        };
        let footer = Paragraph::new(Line::from(Span::styled(
            format!(" {hints}"),
            Style::default().fg(Color::Gray),
        )));
        frame.render_widget(footer, area);
        return;
    }

    let hints = match (app.focus, app.view) {
        (Focus::Input, _) => "Enter send · ↑/↓ history · Alt+Enter newline · Ctrl+W delete word · Esc back to list",
        (Focus::List, View::Events) => {
            "j/k next/prev with detail/files · C collapse all · J/K next/prev line · PgUp/PgDn preview scroll · space fold · m minor · p preview · P full-screen · t transcript · r raw · l log · d driva · i message · s stop · A interactions · S reset · V Workspaces · q quit"
        }
        (Focus::List, View::Raw) => {
            "j/k next/prev line · g/G first/last · PgUp/PgDn preview scroll · r events · l log · t transcript · d driva · i message · s stop · A interactions · S reset · V Workspaces · q quit"
        }
        (Focus::List, View::Log) => {
            "j/k scroll · g/G top/bottom · l events · r raw · t transcript · d driva · i message · s stop · A interactions · S reset · V Workspaces · q quit"
        }
        (Focus::List, View::Transcript) => {
            "j/k scroll · g/G top/bottom · t events · r raw · l log · d driva · i message · s stop · A interactions · S reset · V Workspaces · q quit"
        }
        (Focus::List, View::Driva) => {
            "d events · r raw · l log · t transcript · i message · s stop · A interactions · S reset · V Workspaces · q quit"
        }
        (Focus::List, View::Preview) => {
            "j/k next/prev previewable · J/K next/prev line · PgUp/PgDn scroll · g/G first/last entry · P events · i message · s stop · A interactions · S reset · V Workspaces · q quit"
        }
    };
    let footer = Paragraph::new(Line::from(Span::styled(
        format!(" {hints}"),
        Style::default().fg(Color::Gray),
    )));
    frame.render_widget(footer, area);
}

pub(crate) fn tag_color(tag: &str) -> Color {
    match tag {
        "agent" => Color::Green,
        "user" => Color::Cyan,
        "command" => Color::Rgb(184, 124, 0),
        "tool" => Color::Magenta,
        "plan" | "files" => Color::Blue,
        "error" | "malformed" => Color::Red,
        _ => Color::DarkGray,
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
    fn footer_hints_depend_on_focus() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        // The full hint line is longer than the 80-column test terminal, so
        // check a marker near its start rather than one that may be clipped.
        assert!(rendered(&app).contains("j/k next/prev with detail"));
        app.enter_input();
        assert!(rendered(&app).contains("Enter send"));
    }

    #[test]
    fn footer_advertises_the_collapse_all_shortcut() {
        let app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        assert!(rendered(&app).contains("collapse all"));
    }
}
