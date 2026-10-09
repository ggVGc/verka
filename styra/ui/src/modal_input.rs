//! The message box itself: one centered, modal input box over whatever is
//! already on screen. A message built from several boxes shows them stacked
//! in it, each under a rule naming it.
//!
//! Kept separate from the surrounding screen renderer so input wrapping,
//! backdrop styling, and cursor placement remain one focused component.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::theme;

/// Everything the box draws: its titles, whatever stands above the buffer
/// (the main view's queued messages), and the buffer being typed.
pub struct ModalInput<'a> {
    /// Left-hand title, naming the box and how to send from it.
    pub title: String,
    /// Right-hand title for a standing qualifier on the message — the main
    /// view's answer contract. Drawn in the accent color to set it apart from
    /// the box's own name.
    pub note: Option<String>,
    /// The model the message will be sent to, named at the top right so the
    /// box says what is about to answer — the status line it covers is not
    /// readable while the box is open.
    pub model: Option<String>,
    /// Whether the agent confirmed that model, rather than it only being what
    /// the launch asked for. Dimmed until it has, as the status line does.
    pub model_reported: bool,
    /// The reasoning effort alongside the model, as the status line shows it.
    pub effort: Option<String>,
    /// Whether the agent confirmed that effort. Dimmed until it has, as the
    /// status line does.
    pub effort_reported: bool,
    /// Text above the buffer: already-composed messages still waiting. Wrapped
    /// with the buffer, and dimmed to set it apart from what is being typed.
    pub preceding: Vec<String>,
    /// A transient line under the box: how the last path was resolved, or why
    /// it could not be. The session view has action messages for this; a box
    /// opened over a picker has nowhere else to put it.
    pub notice: Option<String>,
    /// The boxes the message is built from, in order; never empty.
    pub parts: &'a [String],
    /// The box being typed into, or highlighted while choosing.
    pub focused: usize,
    /// Whether the operator is choosing between boxes rather than typing.
    pub choosing: bool,
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
pub fn height(input: &ModalInput<'_>, width: u16) -> u16 {
    let lines = display(input, width).lines.len().max(1);
    // Several boxes each spend a row on their rule, so they get more room.
    let cap = if input.parts.len() > 1 { 16 } else { 8 };
    (lines as u16 + 2).clamp(3, cap)
}

/// Draw the box over the whole frame: wash the finished screen beneath it down
/// to dark gray (and ask the terminal to dim it) so every color visibly
/// recedes, then clear and draw the box itself at normal brightness.
pub fn render(frame: &mut Frame, input: &ModalInput<'_>) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(theme::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        frame.area(),
    );
    let area = area(input, frame.area());
    frame.render_widget(Clear, area);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Span::styled(
            input.title.clone(),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    if let Some(note) = &input.note {
        block = block.title(Span::styled(
            note.clone(),
            Style::default().fg(theme::ACCENT),
        ));
    }
    if let Some(model) = &input.model {
        let mut spans = vec![Span::styled(
            format!(" {model}"),
            Style::default().fg(if input.model_reported {
                theme::TEXT
            } else {
                theme::ADDITIONAL_INFO
            }),
        )];
        if let Some(effort) = &input.effort {
            spans.push(Span::styled(" · ", Style::default().fg(theme::TEXT)));
            spans.push(Span::styled(
                format!("{effort} "),
                Style::default().fg(if input.effort_reported {
                    theme::TEXT
                } else {
                    theme::ADDITIONAL_INFO
                }),
            ));
        } else {
            spans.push(Span::raw(" "));
        }
        block = block.title(Line::from(spans).right_aligned());
    }
    if let Some(notice) = &input.notice {
        block = block.title_bottom(Span::styled(
            format!(" {notice} "),
            Style::default().fg(theme::MUTED_WARNING),
        ));
    }
    let inner = block.inner(area);
    let display = display(input, inner.width);
    // Keep the current box's last row in view: with one box that is the
    // bottom, with several it may be one above the others.
    let scroll = (display.cursor_row + 1).saturating_sub(inner.height);
    frame.render_widget(
        Paragraph::new(display.lines)
            .block(block)
            .scroll((scroll, 0)),
        area,
    );

    if input.cursor && !input.choosing {
        frame.set_cursor_position(Position {
            x: inner.x + display.cursor_col,
            y: inner.y + display.cursor_row.saturating_sub(scroll),
        });
    }
}

pub struct InputDisplay {
    pub lines: Vec<Line<'static>>,
    pub cursor_col: u16,
    pub cursor_row: u16,
}

/// Wrap the box's content to `width` and place the cursor at the end of the
/// current box, which is where typing continues.
pub fn display(input: &ModalInput<'_>, width: u16) -> InputDisplay {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for text in &input.preceding {
        lines.extend(wrapped_input_lines(
            text,
            width,
            Style::default().fg(theme::ADDITIONAL_INFO),
        ));
    }

    let several = input.parts.len() > 1;
    let mut cursor = (0, lines.len());
    for (index, text) in input.parts.iter().enumerate() {
        let current = index == input.focused;
        if several {
            lines.push(rule(index, current, input.choosing, width));
        }
        if text.is_empty() {
            let (placeholder, color) = if current && !input.choosing {
                (input.placeholder, theme::MUTED_TEXT)
            } else {
                ("empty", theme::INACTIVE)
            };
            if current {
                cursor = (0, lines.len());
            }
            lines.push(Line::from(Span::styled(
                placeholder.to_owned(),
                Style::default().fg(color),
            )));
            continue;
        }

        let style = match (current, input.choosing) {
            (true, true) => Style::default()
                .fg(theme::TEXT)
                .bg(theme::SELECTED_ROW_BACKGROUND),
            (true, false) => Style::default().fg(theme::TEXT),
            (false, _) => Style::default().fg(theme::MUTED_TEXT),
        };
        let mut part_lines = wrapped_input_lines(text, width, style);
        if current {
            let mut cursor_col = part_lines
                .last()
                .map(|line| line.width())
                .unwrap_or_default();
            // At the right edge, a terminal cursor advances to the next visual
            // row. Represent that row explicitly so the cursor never lands on
            // the border.
            if cursor_col == width {
                part_lines.push(Line::default());
                cursor_col = 0;
            }
            cursor = (cursor_col, lines.len() + part_lines.len().saturating_sub(1));
        }
        lines.extend(part_lines);
    }

    InputDisplay {
        lines,
        cursor_col: cursor.0 as u16,
        cursor_row: cursor.1 as u16,
    }
}

/// The rule above a box when there are several: its number, drawn in the
/// accent for the current box so the operator can see where typing goes.
fn rule(index: usize, current: bool, choosing: bool, width: usize) -> Line<'static> {
    let marker = if current && choosing { "▶" } else { "─" };
    let label = format!("{marker}─ {} ", index + 1);
    let fill = width.saturating_sub(label.chars().count());
    let style = if current {
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::INACTIVE)
    };
    Line::from(Span::styled(format!("{label}{}", "─".repeat(fill)), style))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(parts: &'a [String], preceding: Vec<String>) -> ModalInput<'a> {
        ModalInput {
            title: " message ".into(),
            note: None,
            model: None,
            model_reported: false,
            effort: None,
            effort_reported: false,
            preceding,
            notice: None,
            parts,
            focused: parts.len() - 1,
            choosing: false,
            placeholder: "type a message, Enter to send",
            cursor: true,
        }
    }

    #[test]
    fn wraps_input_and_keeps_the_cursor_on_the_final_visual_row() {
        let wrapped = display(&input(&["abcdefghijk".into()], Vec::new()), 5);
        assert_eq!(wrapped.lines.len(), 3);
        assert_eq!((wrapped.cursor_col, wrapped.cursor_row), (1, 2));

        let display = display(&input(&["abcde".into()], Vec::new()), 5);
        assert_eq!(display.lines.len(), 2);
        assert_eq!((display.cursor_col, display.cursor_row), (0, 1));
    }

    #[test]
    fn queued_lines_are_visually_secondary() {
        let display = display(
            &input(&["draft".into()], vec!["queued: send later".into()]),
            40,
        );
        let queued = display.lines[0]
            .spans
            .iter()
            .find(|span| span.content.contains("queued:"))
            .unwrap();
        assert_eq!(queued.style.fg, Some(theme::ADDITIONAL_INFO));
    }

    /// Several boxes each sit under a rule, and the cursor stays in the
    /// current one even when a later box follows it.
    #[test]
    fn several_boxes_are_ruled_and_the_cursor_stays_in_the_current_one() {
        let parts = ["first".to_owned(), "second".to_owned(), String::new()];
        let mut boxes = input(&parts, Vec::new());
        boxes.focused = 1;
        let display = display(&boxes, 20);
        let rows: Vec<String> = display.lines.iter().map(|line| line.to_string()).collect();
        assert_eq!(rows.len(), 6);
        assert!(rows[0].starts_with("── 1 "));
        assert_eq!(rows[3], "second");
        assert_eq!(rows[5], "empty");
        assert_eq!((display.cursor_col, display.cursor_row), (6, 3));
    }
}

pub(crate) fn wrapped_input_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
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
