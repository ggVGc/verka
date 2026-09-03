//! The main interaction view's message box: what goes in the shared modal
//! input box when it is opened over a session — its titles, and the messages
//! already queued behind the one being typed.

use super::modal_input::{self, ModalInput};
use crate::app::{App, Focus};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

/// What the session's message box holds, for [`modal_input`] to draw.
pub(crate) fn modal(app: &App) -> ModalInput<'_> {
    let title = if app.can_send() {
        if app.outbox.queued_count() == 0 {
            " message ".to_owned()
        } else {
            format!(" message · {} queued ", app.outbox.queued_count())
        }
    } else {
        " message (resumes on send) ".to_owned()
    };
    ModalInput {
        title,
        // A contract changes what the agent is asked for, so it is shown on
        // the box the whole time it applies rather than only in the sent
        // message.
        note: app
            .outbox
            .contract()
            .map(|contract| format!(" asking for {} ", contract.as_str())),
        preceding: queued_lines(app),
        // The session view says this in an action message instead, under the
        // view rather than under the box.
        notice: None,
        text: &app.composer.text,
        placeholder: "type a message, Enter to send",
        cursor: app.focus == Focus::Input,
    }
}

pub(crate) fn render_input(frame: &mut Frame, app: &App) {
    modal_input::render(frame, &modal(app));
}

/// The messages already waiting, above the one being typed. Each keeps the
/// shape it was composed with, so the line says so — otherwise the operator
/// has no way to tell which of several waiting messages asked for what.
fn queued_lines(app: &App) -> Vec<String> {
    app.outbox
        .queued()
        .map(|message: &styra_server::QueuedMessage| {
            let prefix = match message.contract {
                Some(contract) => format!("queued ({}): ", contract.as_str()),
                None => "queued: ".to_owned(),
            };
            format!("{prefix}{}", message.text)
        })
        .collect()
}

/// Wrap `text` to `width` columns, breaking on explicit newlines and then on
/// the last column that fits.
pub(super) fn wrapped_input_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
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

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::super::testing::rendered;
    use super::super::{modal_input, palette};
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn message_box_is_only_shown_while_input_is_active() {
        let mut app = testing::app("s1");
        assert_eq!(app.focus, Focus::List);
        assert!(!rendered(&app).contains("type a message, Enter to send"));

        app.enter_input();
        assert!(rendered(&app).contains("type a message, Enter to send"));
    }

    #[test]
    fn input_wraps_at_the_panel_width_and_keeps_the_cursor_on_screen() {
        let mut app = testing::app("s1");
        app.enter_input();
        app.set_input("abcdefghijk".into());

        let display = modal_input::display(&modal(&app), 5);
        assert_eq!(display.lines.len(), 3);
        assert_eq!(display.cursor_col, 1);
        assert_eq!(display.cursor_row, 2);

        app.set_input("abcde".into());
        let display = modal_input::display(&modal(&app), 5);
        assert_eq!(display.lines.len(), 2);
        assert_eq!(display.cursor_col, 0);
        assert_eq!(display.cursor_row, 1);
    }

    #[test]
    fn queued_messages_use_the_additional_information_color() {
        let mut app = testing::app("s1");
        app.outbox
            .queue(styra_server::QueuedMessage::new("send this later"));

        let display = modal_input::display(&modal(&app), 40);
        let queued = display.lines[0]
            .spans
            .iter()
            .find(|span| span.content.contains("queued:"))
            .expect("queued message span");
        assert_eq!(queued.style.fg, Some(palette::ADDITIONAL_INFO));
    }

    #[test]
    fn long_input_scrolls_to_keep_the_newest_text_visible() {
        let mut app = testing::app("s1");
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
