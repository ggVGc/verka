use super::*;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::theme;

/// Below this, at full boost, there is not enough signal for the transcriber
/// to find words in — so say so rather than let the operator talk into it.
const TOO_QUIET: f32 = 0.05;

/// The quietest amplitude the bar draws at all. Speech at conversational
/// distance sits near the top of this range; anything below it is room noise,
/// and a linear bar would leave all of it flat against the left-hand end.
const FLOOR_DB: f32 = -54.0;

/// The meter's centered area: the message box's width, and the three rows of
/// content it needs.
fn area(frame_area: Rect) -> Rect {
    let width = frame_area.width.saturating_sub(4).min(80);
    let height = 5.min(frame_area.height);
    Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Draw the meter over the whole frame, dimming what is beneath it exactly as
/// the message box does — this is the same modal moment, with the keyboard
/// belonging to it alone.
pub fn render(frame: &mut Frame, view: &RecordingView) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(theme::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        frame.area(),
    );
    let area = area(frame.area());
    frame.render_widget(Clear, area);

    // An input that is still opening wears neither the red border nor the
    // word: what is true of it is that nothing is being recorded yet, and a
    // box that says "recording" over a device that has not answered is the
    // one thing this view exists to stop saying.
    let (title, tone) = match &view.captured {
        Some(captured) => (format!(" ● recording · {captured} "), theme::ERROR),
        None => (" ○ opening the input… ".to_owned(), theme::MUTED_WARNING),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(tone))
        .title(Span::styled(title, Style::default().fg(tone)))
        .title(Span::styled(
            format!(" boost ×{} ", gain(view.gain)),
            Style::default().fg(theme::ACCENT),
        ))
        .title_bottom(Span::styled(
            " Enter transcribe · Esc cancel · ↑/↓ boost ",
            Style::default().fg(theme::MUTED_TEXT),
        ));
    let inner = block.inner(area);
    let lines = vec![Line::default(), bar(view, inner.width), advice(view)];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The bar itself: filled to the current level, with the loudest of the
/// recording left standing as a mark the level has to be read against.
fn bar(view: &RecordingView, width: u16) -> Line<'static> {
    let width = usize::from(width.max(1));
    let filled = cells(view.level, width);
    let held = cells(view.loudest, width).min(width.saturating_sub(1));
    let mut spans = Vec::new();
    for cell in 0..width {
        // Colored by where the cell is, not by how far the bar has reached:
        // the warning is about the end of the scale, so a level that runs into
        // it turns that part of the bar yellow and then red while everything
        // below stays as it was.
        let (symbol, color) = if cell < filled {
            ("█", tone(cell, width))
        } else if cell == held && view.loudest > 0.0 {
            ("│", tone(cell, width))
        } else {
            ("·", theme::INACTIVE)
        };
        spans.push(Span::styled(symbol, Style::default().fg(color)));
    }
    Line::from(spans)
}

/// How many of `width` cells an amplitude fills, on the decibel scale the ear
/// hears in: halving the amplitude takes 6 dB off the bar rather than half of
/// its length.
fn cells(level: f32, width: usize) -> usize {
    let filled = (decibels(level) - FLOOR_DB) / -FLOOR_DB;
    ((filled.clamp(0.0, 1.0) * width as f32).round() as usize).min(width)
}

fn decibels(level: f32) -> f32 {
    if level <= 0.0 {
        return FLOOR_DB;
    }
    20.0 * level.log10()
}

/// Green through most of the range, yellow where a recording is getting hot,
/// red at the end where it is about to be clipped.
fn tone(cell: usize, width: usize) -> ratatui::style::Color {
    let position = (cell + 1) as f32 / width as f32;
    if position > 0.95 {
        theme::ERROR
    } else if position > 0.8 {
        theme::WARNING
    } else {
        theme::SUCCESS
    }
}

/// The one line under the bar: what to do about what it is showing.
///
/// A meter that only shows a level leaves the operator to work out that the
/// short green stub means their words will come back empty. Clipping is worth
/// the same sentence in the other direction.
fn advice(view: &RecordingView) -> Line<'static> {
    if view.captured.is_none() {
        return Line::from(Span::styled(
            "waiting for the device — nothing is being recorded yet",
            Style::default().fg(theme::MUTED_WARNING),
        ));
    }
    if view.loudest > 1.0 {
        return Line::from(Span::styled(
            "too loud — it is being clipped; ↓ to lower the boost",
            Style::default().fg(theme::WARNING),
        ));
    }
    if view.loudest < TOO_QUIET {
        return Line::from(Span::styled(
            "very quiet — ↑ to boost the input",
            Style::default().fg(theme::MUTED_WARNING),
        ));
    }
    Line::from(Span::styled(
        format!("peak {:.0} dB", decibels(view.loudest)),
        Style::default().fg(theme::MUTED_TEXT),
    ))
}

/// A boost as the operator reads it back: `2` and `1.5`, not `2.0`.
fn gain(gain: f32) -> String {
    if (gain - gain.round()).abs() < 0.01 {
        format!("{gain:.0}")
    } else {
        format!("{gain:.1}")
    }
}

#[cfg(test)]
mod tests;
