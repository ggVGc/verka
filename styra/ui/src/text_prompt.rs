//! The small floating box a single value is typed into: a path, a mount, a
//! name. Every such prompt is drawn here, so they all behave the same way —
//! above all, the box grows to show the whole value rather than cutting a long
//! path off at its right edge, which is the part that is usually being edited.
//!
//! Wrapped the way the message box wraps (see [`crate::modal_input`]): by
//! character, since a path has no spaces to break at, with the cursor at the
//! end of the text, where typing continues.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::fuzzy_list::{marked, FuzzyList};
use crate::modal_input::wrapped_input_lines;
use crate::theme;

/// One prompt: what it is for, and what has been typed into it.
pub struct TextPrompt<'a> {
    /// The top border: what is being asked for and the keys that answer it.
    pub title: Line<'a>,
    /// The top border's right end, for where the answer goes — the branch an
    /// integration lands on.
    pub top_right: Option<Line<'a>>,
    /// The bottom border, for whatever qualifies the answer — which layer a
    /// mount lands in, or that adding one restarts the interaction.
    pub bottom: Option<Line<'a>>,
    /// The value being typed.
    pub text: &'a str,
    /// The border's color.
    pub border: Color,
}

impl<'a> TextPrompt<'a> {
    /// A prompt in the ordinary accent border, with nothing under it.
    pub fn new(title: impl Into<Line<'a>>, text: &'a str) -> Self {
        Self {
            title: title.into(),
            top_right: None,
            bottom: None,
            text,
            border: theme::ACCENT,
        }
    }

    pub fn top_right(mut self, top_right: impl Into<Line<'a>>) -> Self {
        self.top_right = Some(top_right.into().right_aligned());
        self
    }

    pub fn bottom(mut self, bottom: impl Into<Line<'a>>) -> Self {
        self.bottom = Some(bottom.into());
        self
    }
}

/// The text wrapped to `width` columns, and the row and column the cursor
/// takes after it.
fn layout(text: &str, width: u16) -> (Vec<Line<'static>>, u16, u16) {
    let width = usize::from(width.max(1));
    let mut lines = wrapped_input_lines(text, width, Style::default().fg(theme::TEXT));
    let mut column = lines.last().map(Line::width).unwrap_or_default();
    // A cursor past the last column belongs at the start of the next row, not
    // on the border, so that row is drawn too.
    if column >= width {
        lines.push(Line::default());
        column = 0;
    }
    let row = lines.len().saturating_sub(1);
    (lines, row as u16, column as u16)
}

/// Rows the box needs for `text` at `width` columns of content, borders
/// included: one per wrapped row, so it is never cut off while there is room.
pub fn height(text: &str, width: u16) -> u16 {
    layout(text, width).0.len() as u16 + 2
}

/// Draw `prompt` centered in `area`, taking the terminal cursor: a prompt is
/// modal, so it is where typing goes.
///
/// As wide as the view allows up to a comfortable reading width, and as tall
/// as the wrapped text. Only a value taller than the whole view scrolls, and
/// then it keeps the end — where the cursor is — in sight. Returns the box,
/// for whatever is drawn against it.
pub fn render(frame: &mut Frame, prompt: &TextPrompt<'_>, area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(72);
    let (lines, row, column) = layout(prompt.text, width.saturating_sub(2));
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(prompt.border))
        .title(prompt.title.clone());
    if let Some(top_right) = &prompt.top_right {
        block = block.title(top_right.clone());
    }
    if let Some(bottom) = &prompt.bottom {
        block = block.title_bottom(bottom.clone());
    }
    let inner = block.inner(rect);
    let scroll = (lines.len() as u16).saturating_sub(inner.height);
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
    if inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position(Position {
            x: inner.x + column,
            y: inner.y + row.saturating_sub(scroll),
        });
    }
    rect
}

/// Rows of choices the list under a prompt shows at once.
const CHOICE_ROWS: u16 = 10;

/// Draw the `rows` that `list` leaves standing under `prompt`, the box
/// [`render`] drew, with the letters each matched on marked and the cursor's
/// row highlighted and scrolled into view. Above the box instead when there is
/// more room there. Narrowed down to nothing, it says so rather than vanish,
/// so the query that did it is plainly the reason.
pub fn render_choices(
    frame: &mut Frame,
    prompt: Rect,
    area: Rect,
    rows: &[String],
    list: &FuzzyList,
) {
    if rows.is_empty() {
        return;
    }
    let matches = list.matches(rows);
    let selected = list.selected.min(matches.len().saturating_sub(1));
    let below = (area.y + area.height).saturating_sub(prompt.y + prompt.height);
    let above = prompt.y.saturating_sub(area.y);
    let room = below.max(above);
    let height = (matches.len().max(1) as u16)
        .min(CHOICE_ROWS)
        .saturating_add(2)
        .min(room);
    if height < 3 {
        return;
    }
    let y = if below >= above {
        prompt.y + prompt.height
    } else {
        prompt.y - height
    };
    let rect = Rect {
        y,
        height,
        ..prompt
    };
    let visible = usize::from(height - 2);
    let first = (selected + 1).saturating_sub(visible);
    let mut lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(index, found)| {
            let mut spans = vec![Span::raw(" ")];
            spans.extend(marked(&rows[found.index], &found.positions));
            let line = Line::from(spans);
            if index == selected {
                line.style(
                    Style::default()
                        .bg(theme::SELECTION_BACKGROUND)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                line
            }
        })
        .collect();
    if matches.is_empty() {
        lines.push(Line::styled(
            " nothing here matches",
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    let position = match matches.len() {
        0 => format!(" 0/{} ", rows.len()),
        count => format!(" {}/{count} ", selected + 1),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::MUTED_TEXT))
        .title(" ↑↓ select · Tab open · Shift-Tab up · Enter add ")
        .title_bottom(Line::from(position).right_aligned());
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

#[cfg(test)]
mod tests;
