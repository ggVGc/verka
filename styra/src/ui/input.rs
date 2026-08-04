//! The message input box: wraps typed text and queued messages to the panel
//! width and keeps the terminal cursor positioned within them.

use crate::app::{App, Focus};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

pub(crate) fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Input;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let title = if app.can_send() {
        if app.queued_message_count() == 0 {
            " message ".to_owned()
        } else {
            format!(" message · {} queued ", app.queued_message_count())
        }
    } else {
        " message (resumes on send) ".to_owned()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(title, Style::default().fg(Color::Gray)));
    let inner = block.inner(area);
    let display = input_display(app, inner.width);
    let visible_rows = inner.height;
    let scroll = (display.lines.len() as u16).saturating_sub(visible_rows);
    let paragraph = Paragraph::new(display.lines)
        .block(block)
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);

    if focused {
        frame.set_cursor_position(Position {
            x: inner.x + display.cursor_col,
            y: inner.y + display.cursor_row.saturating_sub(scroll),
        });
    }
}

struct InputDisplay {
    lines: Vec<Line<'static>>,
    cursor_col: u16,
    cursor_row: u16,
}

fn input_display(app: &App, width: u16) -> InputDisplay {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for message in app.queued_messages() {
        lines.extend(wrapped_input_lines(
            &format!("queued: {message}"),
            width,
            Style::default().fg(Color::DarkGray),
        ));
    }
    let preceding_rows = lines.len();

    if app.input.is_empty() {
        lines.push(Line::from(Span::styled(
            "type a message, Enter to send",
            Style::default().fg(Color::Gray),
        )));
        return InputDisplay {
            lines,
            cursor_col: 0,
            cursor_row: preceding_rows as u16,
        };
    }

    let mut input_lines = wrapped_input_lines(&app.input, width, Style::default().fg(Color::White));
    let mut cursor_col = input_lines
        .last()
        .map(|line| line.width())
        .unwrap_or_default();
    // At the right edge, a terminal cursor advances to the next visual row.
    // Represent that row explicitly so the cursor never lands on the border.
    if cursor_col == width {
        input_lines.push(Line::default());
        cursor_col = 0;
    }
    let cursor_row = preceding_rows + input_lines.len().saturating_sub(1);
    lines.extend(input_lines);

    InputDisplay {
        lines,
        cursor_col: cursor_col as u16,
        cursor_row: cursor_row as u16,
    }
}

fn wrapped_input_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for logical_line in text.split('\n') {
        let mut current = String::new();
        let mut current_width = 0;
        for ch in logical_line.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if current_width > 0 && current_width + ch_width > width {
                lines.push(Line::from(Span::styled(current, style)));
                current = String::new();
                current_width = 0;
            }
            current.push(ch);
            current_width += ch_width;
        }
        lines.push(Line::from(Span::styled(current, style)));
    }
    lines
}

/// Input box height grows with wrapped content to a useful maximum; beyond
/// that, rendering scrolls to keep the cursor and newest text visible.
pub(crate) fn input_area_height(app: &App, width: u16) -> u16 {
    let lines = input_display(app, width).lines.len().max(1);
    (lines as u16 + 2).clamp(3, 8)
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
    fn message_box_is_only_shown_while_input_is_active() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        assert_eq!(app.focus, Focus::List);
        assert!(!rendered(&app).contains("type a message, Enter to send"));

        app.enter_input();
        assert!(rendered(&app).contains("type a message, Enter to send"));
    }

    #[test]
    fn input_wraps_at_the_panel_width_and_keeps_the_cursor_on_screen() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_input();
        app.set_input("abcdefghijk".into());

        let display = input_display(&app, 5);
        assert_eq!(display.lines.len(), 3);
        assert_eq!(display.cursor_col, 1);
        assert_eq!(display.cursor_row, 2);

        app.set_input("abcde".into());
        let display = input_display(&app, 5);
        assert_eq!(display.lines.len(), 2);
        assert_eq!(display.cursor_col, 0);
        assert_eq!(display.cursor_row, 1);
    }

    #[test]
    fn long_input_scrolls_to_keep_the_newest_text_visible() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_input();
        app.set_input(format!("{}TAIL", "x".repeat(200)));

        let mut terminal = Terminal::new(TestBackend::new(20, 12)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rendered = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("TAIL"), "{rendered}");
    }
}
