//! The notice that stands in the message box's place while the client is
//! blocked on a synchronous call the operator started from it.
//!
//! Sending into a new Git workspace branches the repository and checks out a
//! linked worktree before the prompt goes anywhere, and nothing is read from
//! the keyboard until that returns. Leaving the box on screen over that wait
//! offers typing that does not happen; a notice down among the others is easy
//! to miss and says nothing about the keyboard. So the box goes, and in its
//! place is the one thing that is true: the client is working, and waiting is
//! all there is to do.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::palette;

/// What the blocking notice says: the work in progress, in the operator's
/// terms.
pub struct BusyView<'a> {
    pub message: &'a str,
}

/// The notice's centered area: the message box's width, and the one row of
/// content it needs.
fn area(frame_area: Rect) -> Rect {
    let width = frame_area.width.saturating_sub(4).min(80);
    let height = 3.min(frame_area.height);
    Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Draw the notice over the whole frame, dimming what is beneath it as the
/// message box does: nothing under it will answer a key until it is gone.
pub fn render(frame: &mut Frame, view: &BusyView<'_>) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(palette::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        frame.area(),
    );
    let area = area(frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::MUTED_WARNING))
        .title(Span::styled(
            " working · please wait ",
            Style::default().fg(palette::MUTED_WARNING),
        ));
    let line = Line::from(Span::styled(
        view.message.to_owned(),
        Style::default().fg(palette::ACCENT),
    ));
    frame.render_widget(Paragraph::new(line).block(block), area);
}
