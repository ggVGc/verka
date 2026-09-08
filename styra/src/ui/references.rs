//! The list of files a reply cites, floating over the view it was opened from.
//!
//! Small and centered rather than full-screen like the launch and template
//! pickers: what is behind it is the reply the citations were read in, and
//! keeping that on screen is most of why the list is worth having.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

use super::palette;
use crate::references::{Reference, References};

/// Rows of chrome the box wears: its two border lines.
const CHROME: u16 = 2;

/// Most rows shown at once, beyond which the list scrolls. A reply citing
/// dozens of files should not black out the screen behind it.
const MAX_ROWS: u16 = 12;

/// The box's centered area over `frame_area`: as wide as prose allows and as
/// tall as the citations need, up to [`MAX_ROWS`].
fn area(rows: usize, frame_area: Rect) -> Rect {
    let width = frame_area.width.saturating_sub(4).min(90);
    let height = (rows as u16)
        .min(MAX_ROWS)
        .saturating_add(CHROME)
        .min(frame_area.height);
    Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(crate) fn render(frame: &mut Frame, references: &References, frame_area: Rect) {
    let area = area(references.items().len(), frame_area);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(Span::styled(
            " files in this entry · Enter open · q cancel ",
            Style::default().fg(palette::MUTED_TEXT),
        ));
    let items: Vec<ListItem> = references.items().iter().map(row).collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(references.selected_index()));
    frame.render_stateful_widget(list, area, &mut state);
}

/// One citation: the line it names set apart from the file, since the line is
/// what the operator read the citation for and cannot be opened at.
fn row(reference: &Reference) -> ListItem<'static> {
    let mut spans = vec![Span::styled(
        reference.label(),
        Style::default().fg(palette::TEXT),
    )];
    if let Some(line) = reference.line {
        spans.push(Span::styled(
            format!("  line {line}"),
            Style::default().fg(palette::ADDITIONAL_INFO),
        ));
    }
    ListItem::new(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use std::path::PathBuf;

    fn reference(path: &str, line: Option<u32>) -> Reference {
        Reference {
            reported: path.into(),
            line,
            resolved: PathBuf::from(path),
        }
    }

    /// The list names every citation and marks the one Enter would open. Drawn
    /// through the whole screen renderer, since floating over the view the
    /// citations were read in is the point of it.
    #[test]
    fn every_citation_is_listed_with_the_line_it_named() {
        let mut app = testing::app("s1");
        app.references = References::new(vec![
            reference("/work/src/monitor.c", Some(484)),
            reference("/work/README.md", None),
        ]);
        app.references.as_mut().unwrap().select_next();

        let screen = testing::screen(&app);

        assert!(screen.all().contains("/work/src/monitor.c"));
        assert!(screen.all().contains("line 484"));
        let (x, y) = screen.find("/work/README.md");
        assert_eq!(
            screen.buffer().cell((x, y)).unwrap().style().bg,
            Some(palette::SELECTION_BACKGROUND),
            "the second row is selected, so it wears the selection background"
        );
    }

    /// A reply citing more files than fit leaves the terminal usable.
    #[test]
    fn a_long_list_is_capped_to_the_rows_that_fit() {
        let area = area(50, Rect::new(0, 0, 100, 20));
        assert_eq!(area.height, MAX_ROWS + CHROME);
        assert!(area.width <= 90);
    }
}
