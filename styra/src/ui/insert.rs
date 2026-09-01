//! The path prompt, floating over the message box it writes into.
//!
//! Two boxes for the two questions, so that neither has to be read for the
//! other's answer: one is a text field with a cursor in it, the other a short
//! list of single-key answers about a path that is already decided.

use super::palette;
use crate::app::App;
use crate::insert::Insert;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub(crate) fn render_insert(frame: &mut Frame, app: &App, area: Rect) {
    match &app.insert {
        None => {}
        Some(Insert::Typing(text)) => render_typing(frame, text, area),
        Some(Insert::Grant(host)) => render_grant(frame, &host.display().to_string(), area),
    }
}

/// Centre a box of `height` rows in `area`, no wider than the area allows.
fn floating(area: Rect, height: u16) -> Rect {
    let width = area.width.saturating_sub(4).min(72);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn render_typing(frame: &mut Frame, text: &str, area: Rect) {
    let prompt = floating(area, 3);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" path · Tab complete · Enter insert · Esc cancel ");
    let inner = block.inner(prompt);
    frame.render_widget(Clear, prompt);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_owned(),
            Style::default().fg(palette::TEXT),
        )))
        .block(block),
        prompt,
    );
    // The real cursor, not a drawn bar: this box has the keyboard, and the
    // message box underneath has already claimed the cursor for its own end of
    // text. Placed last, so this is where it lands.
    if inner.width > 0 {
        frame.set_cursor_position(Position {
            x: inner.x + (text.chars().count() as u16).min(inner.width - 1),
            y: inner.y,
        });
    }
}

/// The second question. It names the path in full, because the answer grants a
/// host directory to an isolated agent and the thing being granted is the whole
/// point of asking.
fn render_grant(frame: &mut Frame, host: &str, area: Rect) {
    let prompt = floating(area, 5);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::WARNING))
        .title(Span::styled(
            " outside the sandbox ",
            Style::default().fg(palette::WARNING),
        ))
        .title_bottom(Line::from(Span::styled(
            " for this interaction ",
            Style::default().fg(palette::WARNING),
        )));
    let key = |ch: &'static str| {
        Span::styled(
            ch,
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    };
    let muted = Style::default().fg(palette::MUTED_TEXT);
    let lines = vec![
        Line::from(Span::styled(
            host.to_owned(),
            Style::default().fg(palette::TEXT),
        )),
        Line::default(),
        Line::from(vec![
            key("r"),
            Span::styled(" readable  ", muted),
            key("w"),
            Span::styled(" writable  ", muted),
            key("n"),
            Span::styled(" insert without mounting  ", muted),
            key("Esc"),
            Span::styled(" cancel", muted),
        ]),
    ];
    frame.render_widget(Clear, prompt);
    frame.render_widget(Paragraph::new(lines).block(block), prompt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
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

    fn app() -> App {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_input();
        app
    }

    #[test]
    fn the_path_prompt_floats_over_the_message_it_writes_into() {
        let mut app = app();
        app.insert = Some(Insert::Typing("/srv/data/not".into()));
        let screen = rendered(&app);
        assert!(screen.contains("/srv/data/not"), "{screen}");
        assert!(screen.contains("Tab complete"), "{screen}");
    }

    /// The grant question names the path and every answer to it: what is being
    /// handed to the agent is the whole of what is being asked.
    #[test]
    fn the_grant_question_names_the_path_and_its_answers() {
        let mut app = app();
        app.insert = Some(Insert::Grant(PathBuf::from("/srv/data")));
        let screen = rendered(&app);
        assert!(screen.contains("outside the sandbox"), "{screen}");
        assert!(screen.contains("/srv/data"), "{screen}");
        assert!(screen.contains("readable"), "{screen}");
        assert!(screen.contains("writable"), "{screen}");
        assert!(screen.contains("for this interaction"), "{screen}");
    }
}
