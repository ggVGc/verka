use super::*;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::theme;

/// Inactive boxes remain short enough to scan while editing another one.
const COLLAPSED_LINES: usize = 3;

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
    // bottom, with several it may be one above the others. Below the last
    // box, the source note too, as long as that leaves the cursor in view.
    let bottom = if input.focused + 1 == input.parts.len() {
        (display.lines.len() as u16).min(display.cursor_row + inner.height)
    } else {
        display.cursor_row + 1
    };
    let scroll = bottom.saturating_sub(inner.height);
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

/// Wrap the box's content to `width`, collapse boxes that are not being
/// edited, and place the cursor at the end of the current box.
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
        if (!current || input.choosing) && part_lines.len() > COLLAPSED_LINES {
            part_lines.truncate(COLLAPSED_LINES - 1);
            part_lines.push(Line::from(Span::styled("…", style)));
        }
        if current {
            let mut cursor_col = part_lines
                .last()
                .map(|line| line.width())
                .unwrap_or_default();
            // At the right edge, a terminal cursor advances to the next visual
            // row. Represent that row explicitly so the cursor never lands on
            // the border.
            if !input.choosing && cursor_col == width {
                part_lines.push(Line::default());
                cursor_col = 0;
            }
            cursor = (cursor_col, lines.len() + part_lines.len().saturating_sub(1));
        }
        lines.extend(part_lines);
    }
    if let Some(source) = &input.source {
        lines.extend(wrapped_input_lines(
            &format!("Source: {source}"),
            width,
            Style::default().fg(theme::ADDITIONAL_INFO),
        ));
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
    let fill = width.saturating_sub(crate::text::width(&label));
    let style = if current {
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::INACTIVE)
    };
    Line::from(Span::styled(format!("{label}{}", "─".repeat(fill)), style))
}

pub(crate) fn wrapped_input_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    crate::text::wrap_input(text, width, style)
}

#[cfg(test)]
mod tests;
