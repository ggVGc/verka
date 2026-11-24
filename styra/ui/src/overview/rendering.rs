use super::*;
#[cfg(doc)]
use crate::event_list::EventEntry;
use crate::event_list::{faded_style, message_rows, status_tail};
use crate::interactions::{status_marker, InteractionStatus};
use crate::markdown::LinkDisplay;
use crate::search::{self};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Narrower than this and a tile cannot hold its name and selection on a line
/// each, so the grid takes fewer columns instead.
const MIN_TILE_WIDTH: u16 = 38;
/// Room for the border, the two description lines, two of the message and
/// the status tail. Shorter than this and the grid scrolls instead.
const MIN_TILE_HEIGHT: u16 = 7;
/// How far, in tenths, a tile off the cursor fades toward the background, as
/// far as the navigator fades its rows off the cursor.
const UNSELECTED_FADE: u16 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Grid {
    /// How many tiles each row holds, top to bottom.
    rows: Vec<usize>,
    /// How many rows of tiles fit at once.
    visible_rows: usize,
}

impl Grid {
    /// The row tile `index` is on.
    fn row_of(&self, index: usize) -> usize {
        let mut start = 0;
        for (row, length) in self.rows.iter().enumerate() {
            start += length;
            if index < start {
                return row;
            }
        }
        self.rows.len().saturating_sub(1)
    }
}

/// The tiles laid out as evenly as they go: as many columns as make the grid
/// square, or as fit across when fewer do, and the tiles shared out between
/// the rows so no two differ by more than one, the longer rows first. Five
/// tiles are three over two, seven three over two over two.
fn grid(area: Rect, tiles: usize) -> Grid {
    let tiles = tiles.max(1);
    let square = (1..=tiles)
        .find(|columns| columns * columns >= tiles)
        .unwrap_or(tiles);
    let columns = square.min((area.width / MIN_TILE_WIDTH).max(1) as usize);
    let rows = tiles.div_ceil(columns);
    let (base, extra) = (tiles / rows, tiles % rows);
    let fit = (area.height / MIN_TILE_HEIGHT).max(1) as usize;
    Grid {
        rows: (0..rows)
            .map(|row| base + usize::from(row < extra))
            .collect(),
        visible_rows: rows.min(fit),
    }
}

fn layout_grid(area: Rect, tiles: usize, layout: OverviewLayout) -> Grid {
    if layout == OverviewLayout::Auto {
        return grid(area, tiles);
    }
    let tiles = tiles.max(1);
    let columns = 2.min((area.width / MIN_TILE_WIDTH).max(1) as usize);
    let rows = tiles.div_ceil(columns);
    Grid {
        rows: (0..rows)
            .map(|row| columns.min(tiles - row * columns))
            .collect(),
        visible_rows: rows.min((area.height / MIN_TILE_HEIGHT).max(1) as usize),
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
    let counts = format!(
        " · {running} running · {} idle · r running only: {} · t {} · c layout: {} · ? keys ",
        view.tiles.len() - running,
        if view.running_only { "on" } else { "off" },
        view.time_filter,
        view.layout.label()
    );
    // A narrowed grid names its Workspace up front, on a band of the
    // Workspace hue, and takes that hue for its whole frame.
    let (title, frame_color) = match &view.workspace_scope {
        Some(name) => (
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!(" Workspace: {name} "),
                    Style::default()
                        .fg(Color::Black)
                        .bg(theme::OVERVIEW_WORKSPACE_FRAME)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" overview · w scope: this Workspace"),
                Span::raw(counts),
            ]),
            theme::OVERVIEW_WORKSPACE_FRAME,
        ),
        None => (
            Line::from(format!(" overview · w scope: All{counts}")),
            theme::ACCENT,
        ),
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if view.workspace_scope.is_some() {
            BorderType::Double
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(frame_color))
        .title(title);
    if let Some(prompt) = search::prompt(&view.search) {
        block = block.title_bottom(prompt);
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if view.tiles.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                if view.running_only {
                    " no interaction is currently running"
                } else {
                    " no interaction is running or idle"
                },
                Style::default()
                    .fg(theme::MUTED_TEXT)
                    .add_modifier(Modifier::DIM),
            ))),
            inner,
        );
        return OverviewFeedback { rows: Vec::new() };
    }
    let grid = layout_grid(inner, view.tiles.len(), view.layout);
    let selected = view.selected.min(view.tiles.len() - 1);
    // Scrolled by whole rows of tiles, just far enough to keep the cursor's
    // row on screen.
    let first_row = (grid.row_of(selected) + 1).saturating_sub(grid.visible_rows);
    let heights = split(inner.height, grid.visible_rows);
    let mut start = grid.rows[..first_row].iter().sum::<usize>();
    for (screen_row, (y, height)) in heights.into_iter().enumerate() {
        let Some(&length) = grid.rows.get(first_row + screen_row) else {
            break;
        };
        // Automatic rows share the whole width. Two-column mode keeps the
        // columns aligned even when the last row holds only one tile.
        let columns = if view.layout == OverviewLayout::TwoColumns {
            2.min((inner.width / MIN_TILE_WIDTH).max(1) as usize)
        } else {
            length
        };
        for (offset, (x, width)) in split(inner.width, columns)
            .into_iter()
            .take(length)
            .enumerate()
        {
            let index = start + offset;
            let tile_area = Rect {
                x: inner.x + x,
                y: inner.y + y,
                width,
                height,
            };
            render_tile(
                frame,
                &view.tiles[index],
                index == selected,
                tile_area,
                view.links,
                view.search.term(),
            );
        }
        start += length;
    }
    OverviewFeedback { rows: grid.rows }
}

fn render_tile(
    frame: &mut Frame,
    tile: &OverviewTile<'_>,
    selected: bool,
    area: Rect,
    links: LinkDisplay,
    search: Option<&str>,
) {
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
    let border_color = if tile.rate_limited.is_some() {
        theme::STOP_RATE_LIMITED
    } else {
        match tile.status {
            InteractionStatus::Idle | InteractionStatus::Pending => {
                if tile.newly_idle {
                    theme::OVERVIEW_NEWLY_IDLE_FRAME
                } else {
                    theme::OVERVIEW_IDLE_FRAME
                }
            }
            _ => marker_color,
        }
    };
    // The frame keeps its status color under the cursor: its heavier border
    // marks selection, other tiles are dashed, and the attached interaction
    // has the navigator's `•`.
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::LightDoubleDashed
        })
        .border_style(Style::default().fg(border_color))
        .title(search::highlight_lines(vec![Line::from(title)], search).remove(0));
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
    ];
    lines.extend(marks_line(tile));
    if !tile.tags.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("#{}", tile.tags.join(" #")),
            Style::default().fg(theme::INTERACTION_TAG),
        )));
    }
    let lines = search::highlight_lines(lines, search);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let description = Paragraph::new(lines).wrap(Wrap { trim: true });
    let description_height = (description.line_count(inner.width.max(1)) as u16).min(inner.height);
    frame.render_widget(
        description,
        Rect {
            height: description_height,
            ..inner
        },
    );
    let tail = status_tail(&tile.tail);
    let tail_height = u16::from(!tail.spans.is_empty()).min(inner.height - description_height);
    let room = inner.height - description_height - tail_height;
    // The latest message first, then as many before it as fit whole above
    // it. A latest one too long for the room is shown from its start. Each
    // is drawn as the event list draws it, already wrapped to the width.
    let mut shown = Vec::new();
    let mut used = 0;
    for message in tile.messages.iter().rev() {
        let rows = message_rows(
            &message.event,
            message.contract.as_ref(),
            message.branch.clone(),
            usize::from(inner.width.max(1)),
            tile.protocol,
            links,
        );
        if used + rows.len() > usize::from(room) && !shown.is_empty() {
            break;
        }
        used += rows.len();
        shown.push(search::highlight_lines(rows, search));
    }
    let shown = shown.into_iter().rev().flatten().collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(shown),
        Rect {
            y: inner.y + description_height,
            height: room,
            ..inner
        },
    );
    // Right under the messages, as the list has it right under its last
    // entry, or along the bottom when they fill the room.
    frame.render_widget(
        Paragraph::new(tail),
        Rect {
            y: inner.y + description_height + used.min(usize::from(room)) as u16,
            height: tail_height,
            ..inner
        },
    );
    if !selected {
        fade(frame, area);
    }
}

/// Fade a tile off the cursor: every foreground a step toward the background,
/// as the navigator fades the rows off its cursor, so the tile keeps its hues
/// — its frame still tells its state — but the one under the cursor stands
/// out.
fn fade(frame: &mut Frame, area: Rect) {
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            let fg = match cell.fg {
                Color::Reset => theme::TEXT,
                fg => fg,
            };
            let style = faded_style(Style::default().fg(fg), UNSELECTED_FADE);
            cell.set_style(style);
        }
    }
}

/// The marks that ask for the operator, or `None` when there are none. What
/// the interaction is doing is the status tail's to say.
fn marks_line(tile: &OverviewTile<'_>) -> Option<Line<'static>> {
    let mut marks = Vec::new();
    if let Some(window) = &tile.rate_limited {
        marks.push(Span::styled(
            format!("RATE LIMITED ({window})"),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if tile.newly_idle {
        marks.push(Span::styled(
            "NEWLY IDLE",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if tile.uncommitted {
        marks.push(Span::styled(
            "uncommitted",
            Style::default().fg(theme::UNCOMMITTED),
        ));
    }
    if marks.is_empty() {
        return None;
    }
    let mut spans = Vec::new();
    for mark in marks {
        if !spans.is_empty() {
            spans.push(Span::raw(" · "));
        }
        spans.push(mark);
    }
    Some(Line::from(spans))
}

#[cfg(test)]
mod tests;
