//! The message box itself: one centered, modal input box over whatever is
//! already on screen.
//!
//! Both places that compose a message use this — the main interaction view and
//! the live-interactions picker — so a message is typed into the same box, at
//! the same size, with the same wrapping and cursor, wherever it was opened
//! from. Only what is behind it, and what a sent message goes to, differ.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::input::wrapped_input_lines;
use super::palette;

/// Everything the box draws: its titles, whatever stands above the buffer
/// (the main view's queued messages), and the buffer being typed.
pub(crate) struct ModalInput<'a> {
    /// Left-hand title, naming the box and how to send from it.
    pub title: String,
    /// Right-hand title for a standing qualifier on the message — the main
    /// view's answer contract. Drawn in the accent color to set it apart from
    /// the box's own name.
    pub note: Option<String>,
    /// Text above the buffer: already-composed messages still waiting. Wrapped
    /// with the buffer, and dimmed to set it apart from what is being typed.
    pub preceding: Vec<String>,
    /// The buffer being typed.
    pub text: &'a str,
    /// What an empty buffer says instead, so the box explains itself.
    pub placeholder: &'a str,
    /// Whether the terminal cursor belongs in this box. Only the innermost
    /// modal owns the cursor, so the main view hands it to its own file prompt.
    pub cursor: bool,
}

/// The box's centered area over `frame_area`: wide enough for prose, and as
/// tall as the wrapped content needs up to a cap, beyond which it scrolls.
fn area(input: &ModalInput<'_>, frame_area: Rect) -> Rect {
    let width = frame_area.width.saturating_sub(4).min(80);
    let height = height(input, width.saturating_sub(2)).min(frame_area.height);
    Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Height the box wants for `width` columns of content, borders included.
pub(crate) fn height(input: &ModalInput<'_>, width: u16) -> u16 {
    let lines = display(input, width).lines.len().max(1);
    (lines as u16 + 2).clamp(3, 8)
}

/// Draw the box over the whole frame: wash the finished screen beneath it down
/// to dark gray (and ask the terminal to dim it) so every color visibly
/// recedes, then clear and draw the box itself at normal brightness.
pub(crate) fn render(frame: &mut Frame, input: &ModalInput<'_>) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(palette::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        frame.area(),
    );
    let area = area(input, frame.area());
    frame.render_widget(Clear, area);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Span::styled(
            input.title.clone(),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    if let Some(note) = &input.note {
        block = block.title(Span::styled(
            note.clone(),
            Style::default().fg(palette::ACCENT),
        ));
    }
    let inner = block.inner(area);
    let display = display(input, inner.width);
    let scroll = (display.lines.len() as u16).saturating_sub(inner.height);
    frame.render_widget(
        Paragraph::new(display.lines)
            .block(block)
            .scroll((scroll, 0)),
        area,
    );

    if input.cursor {
        frame.set_cursor_position(Position {
            x: inner.x + display.cursor_col,
            y: inner.y + display.cursor_row.saturating_sub(scroll),
        });
    }
}

pub(crate) struct InputDisplay {
    pub lines: Vec<Line<'static>>,
    pub cursor_col: u16,
    pub cursor_row: u16,
}

/// Wrap the box's content to `width` and place the cursor at the end of the
/// buffer, which is where typing continues.
pub(crate) fn display(input: &ModalInput<'_>, width: u16) -> InputDisplay {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for text in &input.preceding {
        lines.extend(wrapped_input_lines(
            text,
            width,
            Style::default().fg(palette::ADDITIONAL_INFO),
        ));
    }
    let preceding_rows = lines.len();

    if input.text.is_empty() {
        lines.push(Line::from(Span::styled(
            input.placeholder.to_owned(),
            Style::default().fg(palette::MUTED_TEXT),
        )));
        return InputDisplay {
            lines,
            cursor_col: 0,
            cursor_row: preceding_rows as u16,
        };
    }

    let mut input_lines =
        wrapped_input_lines(input.text, width, Style::default().fg(palette::TEXT));
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
