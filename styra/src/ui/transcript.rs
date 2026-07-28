//! A quick way to read the current session as a plain-text transcript,
//! rendered fresh from the decoded events each frame with genta's
//! `render_events`. Unlike the raw/log views, it reads as a document from the
//! start rather than anchoring to the tail.

use super::title_line;
use crate::app::{App, Focus};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Follows the event list's minor and conversation-only filters; since this
/// recomputes from `app.entries` fresh every frame rather than caching
/// anything, changing a filter re-renders it with no extra wiring needed.
pub(crate) fn render_transcript_view(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(
            &app.launch_label(),
            &app.status,
            Some("transcript"),
        ));
    if app.conversation_only {
        block = block.title_bottom(Line::from(Span::styled(
            " conversation only ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
    }

    if app.entries.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  nothing to render yet",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let events = app
        .entries
        .iter()
        .enumerate()
        .filter(|(idx, _)| app.is_visible(*idx))
        .map(|(_, entry)| entry.event.clone())
        .collect::<Vec<_>>();
    let text = styra_server::render::render_events(&events, false, app.show_minor);
    let lines: Vec<Line<'static>> = text
        .lines()
        .map(|line| {
            Line::from(Span::styled(
                line.to_owned(),
                Style::default().fg(Color::White),
            ))
        })
        .collect();

    let viewport = area.height.saturating_sub(2) as usize;
    let max_start = lines.len().saturating_sub(viewport) as u16;
    let start = app.transcript_scroll.min(max_start);
    let paragraph = Paragraph::new(lines).block(block).scroll((start, 0));
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use styra_server::event::AgentEvent;

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
    fn transcript_view_renders_the_current_session() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::UserMessage {
            text: "implement retry backoff".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "Added backoff, tests pass.".into(),
        });
        app.toggle_transcript();
        let screen = rendered(&app);
        assert!(screen.contains("transcript"));
        assert!(screen.contains("implement retry backoff"));
        assert!(screen.contains("Added backoff"));
    }

    #[test]
    fn transcript_view_follows_the_minor_toggle_and_rerenders_when_flipped() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-1".into(),
            model: None,
            effort: None,
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "hello world".into(),
        });
        app.toggle_transcript();

        assert!(!app.show_minor);
        assert!(!rendered(&app).contains("t-1"));

        // Toggling minor visibility while the transcript is already open must
        // re-render it on the very next frame, not require reopening the view.
        app.toggle_minor();
        assert!(rendered(&app).contains("t-1"));
    }

    #[test]
    fn transcript_view_follows_conversation_only_filter_and_shows_indicator() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::UserMessage {
            text: "keep this prompt".into(),
        });
        app.push_event(AgentEvent::Thinking {
            text: "hide this reasoning".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "keep this reply".into(),
        });
        app.toggle_conversation_only();
        app.toggle_transcript();

        let screen = rendered(&app);
        assert!(screen.contains("conversation only"));
        assert!(screen.contains("keep this prompt"));
        assert!(screen.contains("keep this reply"));
        assert!(!screen.contains("hide this reasoning"));
    }

    #[test]
    fn transcript_view_shows_a_placeholder_before_anything_happens() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.toggle_transcript();
        assert!(rendered(&app).contains("nothing to render yet"));
    }
}
