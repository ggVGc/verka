//! The overview: every active interaction laid out as a tile in a grid, so
//! the whole fleet can be watched at once instead of one row at a time.

use crate::interactions::{status_marker, InteractionStatus};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::borrow::Cow;

/// Narrower than this and a tile cannot hold its name and selection on a line
/// each, so the grid takes fewer columns instead.
const MIN_TILE_WIDTH: u16 = 38;
/// Room for the border, the three description lines and two of the message.
/// Shorter than this and the grid scrolls instead.
const MIN_TILE_HEIGHT: u16 = 7;

pub struct OverviewTile<'a> {
    pub name: Cow<'a, str>,
    pub workspace: Cow<'a, str>,
    /// The provider, model and effort, as the launcher names them.
    pub selection: Cow<'a, str>,
    /// As [`crate::interactions::InteractionRow::Interaction::branch`].
    pub branch: Option<&'a str>,
    pub status: InteractionStatus,
    /// How long a working interaction has been at it. `None` for one waiting
    /// on the operator, which the event list does not time either.
    pub elapsed: Option<String>,
    /// The interaction this client is attached to.
    pub current: bool,
    pub newly_idle: bool,
    /// As [`crate::interactions::InteractionRow::Interaction::rate_limited`].
    pub rate_limited: Option<Cow<'a, str>>,
    pub uncommitted: bool,
    pub tags: &'a [String],
    pub last_message: Option<&'a str>,
}

pub struct OverviewView<'a> {
    pub tiles: Vec<OverviewTile<'a>>,
    /// The tile under the cursor, an index into [`Self::tiles`].
    pub selected: usize,
}

/// How the grid was laid out, which the application needs to move the cursor
/// up and down a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverviewFeedback {
    pub columns: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grid {
    columns: usize,
    rows: usize,
    /// How many rows of tiles fit at once.
    visible_rows: usize,
}

fn grid(area: Rect, tiles: usize) -> Grid {
    let columns = ((area.width / MIN_TILE_WIDTH).max(1) as usize).min(tiles.max(1));
    let rows = tiles.div_ceil(columns).max(1);
    let fit = (area.height / MIN_TILE_HEIGHT).max(1) as usize;
    Grid {
        columns,
        rows,
        visible_rows: rows.min(fit),
    }
}

/// `total` cells cut into `parts` runs as even as they divide, the first ones
/// a cell longer to take up the remainder, as `(offset, length)` pairs. The
/// runs always add up to `total`, so the tiles meet the border with no gap.
fn split(total: u16, parts: usize) -> Vec<(u16, u16)> {
    let parts = parts.max(1) as u16;
    let (base, extra) = (total / parts, total % parts);
    let mut offset = 0;
    (0..parts)
        .map(|part| {
            let length = base + u16::from(part < extra);
            let run = (offset, length);
            offset += length;
            run
        })
        .collect()
}

pub fn render(frame: &mut Frame, view: &OverviewView<'_>, area: Rect) -> OverviewFeedback {
    let running = view
        .tiles
        .iter()
        .filter(|tile| {
            matches!(
                tile.status,
                InteractionStatus::Running { .. } | InteractionStatus::Background
            )
        })
        .count();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(format!(
            " overview · {running} running · {} idle · ? keys ",
            view.tiles.len() - running
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if view.tiles.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no interaction is running or idle",
                Style::default()
                    .fg(theme::MUTED_TEXT)
                    .add_modifier(Modifier::DIM),
            ))),
            inner,
        );
        return OverviewFeedback { columns: 1 };
    }
    let grid = grid(inner, view.tiles.len());
    let selected = view.selected.min(view.tiles.len() - 1);
    // Scrolled by whole rows of tiles, just far enough to keep the cursor's
    // row on screen.
    let first_row = (selected / grid.columns + 1).saturating_sub(grid.visible_rows);
    let heights = split(inner.height, grid.visible_rows);
    for (screen_row, (y, height)) in heights.into_iter().enumerate() {
        let row = first_row + screen_row;
        if row >= grid.rows {
            break;
        }
        let start = row * grid.columns;
        let end = (start + grid.columns).min(view.tiles.len());
        // A short last row shares the whole width between its tiles rather
        // than leaving the cells of the missing ones empty.
        for (offset, (x, width)) in split(inner.width, end - start).into_iter().enumerate() {
            let index = start + offset;
            let tile_area = Rect {
                x: inner.x + x,
                y: inner.y + y,
                width,
                height,
            };
            render_tile(frame, &view.tiles[index], index == selected, tile_area);
        }
    }
    OverviewFeedback {
        columns: grid.columns,
    }
}

fn render_tile(frame: &mut Frame, tile: &OverviewTile<'_>, selected: bool, area: Rect) {
    let (marker, marker_color) = status_marker(tile.status);
    let running = matches!(tile.status, InteractionStatus::Running { .. });
    let mut title = vec![
        Span::raw(" "),
        Span::styled(
            marker,
            Style::default()
                .fg(marker_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if tile.uncommitted {
        title.push(Span::styled(
            "! ",
            Style::default()
                .fg(theme::UNCOMMITTED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    title.push(Span::styled(
        tile.name.to_string(),
        if running || selected {
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT)
        },
    ));
    title.push(Span::raw(" "));
    // The cursor's tile is told apart by a heavier border in the accent; the
    // one attached below by the navigator's own `•`.
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(if selected {
            theme::SELECTION_MARKER
        } else {
            theme::INACTIVE
        }))
        .title(Line::from(title));
    if tile.current {
        block = block.title(
            Line::from(Span::styled(
                " • ",
                Style::default().fg(theme::SELECTION_MARKER),
            ))
            .right_aligned(),
        );
    }

    let mut place = vec![Span::styled(
        tile.workspace.to_string(),
        Style::default()
            .fg(theme::WORKSPACE_NAME)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(branch) = tile.branch {
        place.push(Span::styled(
            format!(" · {branch}"),
            Style::default().fg(theme::ACCENT),
        ));
    }
    let mut lines = vec![
        Line::from(place),
        Line::from(Span::styled(
            tile.selection.to_string(),
            Style::default().fg(theme::INTERACTION_STATUS_INFO),
        )),
        status_line(tile),
    ];
    if !tile.tags.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("#{}", tile.tags.join(" #")),
            Style::default().fg(theme::INTERACTION_TAG),
        )));
    }
    if let Some(text) = tile.last_message {
        lines.push(Line::from(Span::styled(
            format!("« {text}"),
            Style::default().fg(theme::SUBORDINATE_TEXT),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

/// What the interaction is doing, and the marks that ask for the operator.
fn status_line(tile: &OverviewTile<'_>) -> Line<'static> {
    let (word, color) = match tile.status {
        InteractionStatus::Running { .. } => ("running", theme::RUNNING),
        InteractionStatus::Background => ("background task", theme::MUTED_WARNING),
        InteractionStatus::Pending | InteractionStatus::Idle => ("idle", theme::SUCCESS),
        InteractionStatus::Stopped(tone) => ("stopped", tone.color()),
        InteractionStatus::Error => ("failed", theme::ERROR),
        InteractionStatus::Ended => ("ended", theme::INACTIVE),
    };
    let mut spans = vec![Span::styled(word, Style::default().fg(color))];
    if let Some(elapsed) = &tile.elapsed {
        spans.push(Span::styled(
            format!(" {elapsed}"),
            Style::default().fg(color),
        ));
    }
    if let Some(window) = &tile.rate_limited {
        spans.push(Span::styled(
            format!(" · RATE LIMITED ({window})"),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if tile.newly_idle {
        spans.push(Span::styled(
            " · NEWLY IDLE",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if tile.uncommitted {
        spans.push(Span::styled(
            " · uncommitted",
            Style::default().fg(theme::UNCOMMITTED),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn tile(name: &'static str, status: InteractionStatus) -> OverviewTile<'static> {
        OverviewTile {
            name: name.into(),
            workspace: "Payments".into(),
            selection: "claude:claude-opus-5/high".into(),
            branch: None,
            status,
            elapsed: None,
            current: false,
            newly_idle: false,
            rate_limited: None,
            uncommitted: false,
            tags: &[],
            last_message: None,
        }
    }

    fn draw(view: &OverviewView<'_>, width: u16, height: u16) -> (String, OverviewFeedback) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut feedback = None;
        terminal
            .draw(|frame| feedback = Some(render(frame, view, frame.area())))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        (screen, feedback.unwrap())
    }

    #[test]
    fn tiles_say_where_each_interaction_is_and_what_it_last_said() {
        let tags = vec!["bug".to_owned()];
        let mut working = tile("repair checkout", InteractionStatus::Running { events: 1 });
        working.branch = Some("styra/fix");
        working.elapsed = Some("2m14s".into());
        working.tags = &tags;
        working.last_message = Some("The checks are green.");
        let mut waiting = tile("write the docs", InteractionStatus::Idle);
        waiting.newly_idle = true;
        let view = OverviewView {
            tiles: vec![working, waiting],
            selected: 0,
        };
        let (screen, feedback) = draw(&view, 100, 14);

        assert_eq!(feedback.columns, 2, "{screen}");
        assert!(screen.contains("·•· repair checkout"), "{screen}");
        assert!(screen.contains("Payments · styra/fix"), "{screen}");
        assert!(screen.contains("running 2m14s"), "{screen}");
        assert!(screen.contains("#bug"), "{screen}");
        assert!(screen.contains("« The checks are green."), "{screen}");
        assert!(screen.contains("● write the docs"), "{screen}");
        assert!(screen.contains("idle · NEWLY IDLE"), "{screen}");
        assert!(screen.contains("1 running · 1 idle"), "{screen}");
    }

    #[test]
    fn a_narrow_screen_stacks_the_tiles_in_one_column() {
        let view = OverviewView {
            tiles: vec![
                tile("first", InteractionStatus::Idle),
                tile("second", InteractionStatus::Idle),
            ],
            selected: 0,
        };
        let (screen, feedback) = draw(&view, 50, 20);
        assert_eq!(feedback.columns, 1);
        let first = screen.lines().position(|line| line.contains("first"));
        let second = screen.lines().position(|line| line.contains("second"));
        assert!(first < second, "{screen}");
    }

    /// The tiles cover the whole frame: rows and columns share out what does
    /// not divide evenly, and a short last row stretches across the width.
    #[test]
    fn the_tiles_fill_the_whole_frame() {
        let view = OverviewView {
            tiles: vec![
                tile("first", InteractionStatus::Idle),
                tile("second", InteractionStatus::Idle),
                tile("third", InteractionStatus::Idle),
            ],
            // Off the tiles whose corners are checked, which are not drawn
            // thick then.
            selected: 0,
        };
        let (screen, feedback) = draw(&view, 101, 31);
        assert_eq!(feedback.columns, 2, "{screen}");
        let cell = |x: usize, y: usize| screen.lines().nth(y).unwrap().chars().nth(x).unwrap();
        // The frame's inner area runs from (1, 1) to (99, 29).
        assert_eq!(
            cell(99, 1),
            '╮',
            "second tile meets the right border\n{screen}"
        );
        assert_eq!(cell(1, 29), '╰', "third tile meets the bottom\n{screen}");
        assert_eq!(cell(99, 29), '╯', "third tile spans the width\n{screen}");
    }

    #[test]
    fn a_split_hands_out_the_remainder_and_adds_up() {
        assert_eq!(split(10, 3), [(0, 4), (4, 3), (7, 3)]);
        assert_eq!(split(9, 3), [(0, 3), (3, 3), (6, 3)]);
    }

    /// More tiles than fit scroll by whole rows, keeping the cursor's on
    /// screen.
    #[test]
    fn the_cursor_tile_stays_on_screen() {
        let names = ["one", "two", "three", "four", "five", "six"];
        let view = OverviewView {
            tiles: names
                .iter()
                .map(|name| tile(name, InteractionStatus::Idle))
                .collect(),
            selected: 5,
        };
        // One column, two seven-row tiles high.
        let (screen, _) = draw(&view, 40, 16);
        assert!(screen.contains("six"), "{screen}");
        assert!(!screen.contains("one"), "{screen}");
    }

    #[test]
    fn an_empty_fleet_says_so() {
        let view = OverviewView {
            tiles: Vec::new(),
            selected: 0,
        };
        let (screen, _) = draw(&view, 60, 5);
        assert!(
            screen.contains("no interaction is running or idle"),
            "{screen}"
        );
    }
}
