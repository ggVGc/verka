//! The small floating box a single value is typed into: a path, a mount, a
//! name. Every such prompt is drawn here, so they all behave the same way —
//! above all, the box grows to show the whole value rather than cutting a long
//! path off at its right edge, which is the part that is usually being edited.
//!
//! Wrapped the way the message box wraps (see [`crate::modal_input`]): by
//! character, since a path has no spaces to break at, with the cursor at the
//! end of the text, where typing continues.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::modal_input::wrapped_input_lines;
use crate::theme;

/// One prompt: what it is for, and what has been typed into it.
pub struct TextPrompt<'a> {
    /// The top border: what is being asked for and the keys that answer it.
    pub title: Line<'a>,
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
            bottom: None,
            text,
            border: theme::ACCENT,
        }
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
/// then it keeps the end — where the cursor is — in sight.
pub fn render(frame: &mut Frame, prompt: &TextPrompt<'_>, area: Rect) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(text: &str, width: u16, height: u16) -> (Vec<String>, Position) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &TextPrompt::new(" path ", text).bottom(" for this interaction "),
                    frame.area(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect();
        let cursor = terminal.get_cursor_position().unwrap();
        (rows, cursor)
    }

    /// The reported case: a long path was cut off at the box's right edge.
    /// Every character of it has to be on screen.
    #[test]
    fn a_long_value_wraps_onto_as_many_rows_as_it_needs() {
        let path = "/home/operator/projects/a-rather-long-directory-name/with/several/nested/levels/data.csv";
        let (rows, cursor) = screen(path, 40, 20);

        let shown: String = rows
            .iter()
            .filter_map(|row| {
                let start = row.find('│')?;
                let end = row.rfind('│')?;
                (end > start).then(|| row[start + '│'.len_utf8()..end].trim_end().to_owned())
            })
            .collect();
        assert_eq!(shown, path);
        // 36 columns of content: the path takes three rows, plus borders.
        assert_eq!(height(path, 36), 5);
        // The cursor follows the end of the text onto its last row.
        let last = rows
            .iter()
            .rposition(|row| row.contains("data.csv"))
            .unwrap();
        assert_eq!(cursor.y as usize, last);
    }

    #[test]
    fn a_short_value_keeps_the_one_row_box() {
        assert_eq!(height("notes.txt", 36), 3);
        assert_eq!(height("", 36), 3);
    }

    /// Filling the last column exactly moves the cursor to a row of its own
    /// rather than onto the border.
    #[test]
    fn a_value_that_fills_the_row_gives_the_cursor_the_next_one() {
        assert_eq!(height(&"x".repeat(36), 36), 4);
        let (lines, row, column) = layout(&"x".repeat(36), 36);
        assert_eq!(lines.len(), 2);
        assert_eq!((row, column), (1, 0));
    }

    /// Taller than the view, it scrolls to keep the end — and the cursor — in
    /// sight rather than drawing past the screen.
    #[test]
    fn a_value_taller_than_the_view_keeps_its_end_visible() {
        let text = format!("{}END", "x".repeat(36 * 10));
        let (rows, cursor) = screen(&text, 40, 6);
        assert!(rows.iter().any(|row| row.contains("END")), "{rows:#?}");
        assert!(cursor.y < 6);
    }
}
