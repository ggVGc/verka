//! The overview: every active interaction laid out as a tile in a grid, so
//! the whole fleet can be watched at once instead of one row at a time.

use crate::event_list::message_rows;
use crate::interactions::{status_marker, InteractionStatus};
use crate::markdown::LinkDisplay;
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::borrow::Cow;
use styra_protocol::event::{AgentEvent, Protocol};
use styra_protocol::Contract;

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
    /// How the interaction's provider presents its events, so its messages
    /// read as they do in its own event list.
    pub protocol: Protocol,
    /// The tail of the conversation, oldest first. As many as fit are shown,
    /// counted back from the latest.
    pub messages: Vec<OverviewMessage<'a>>,
}

pub struct OverviewMessage<'a> {
    pub from_operator: bool,
    pub text: &'a str,
    /// The answer shape an operator message asked for, if it asked for one.
    pub contract: Option<Contract>,
}

pub struct OverviewView<'a> {
    pub tiles: Vec<OverviewTile<'a>>,
    pub running_only: bool,
    /// The tile under the cursor, an index into [`Self::tiles`].
    pub selected: usize,
    /// How links in the messages are drawn, as in the event list.
    pub links: LinkDisplay,
}

/// How the grid was laid out, which the application needs to move the cursor
/// up and down between rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverviewFeedback {
    /// How many tiles each row holds, top to bottom.
    pub rows: Vec<usize>,
}

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
            " overview · {running} running · {} idle · r running only: {} · ? keys ",
            view.tiles.len() - running,
            if view.running_only { "on" } else { "off" }
        ));
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
    let grid = grid(inner, view.tiles.len());
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
        // A shorter row shares the whole width between its tiles rather than
        // leaving the cells of the missing ones empty.
        for (offset, (x, width)) in split(inner.width, length).into_iter().enumerate() {
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
    let room = inner.height - description_height;
    // The latest message first, then as many before it as fit whole above
    // it. A latest one too long for the room is shown from its start. Each
    // is drawn as the event list draws it, already wrapped to the width.
    let mut shown = Vec::new();
    let mut used = 0;
    for message in tile.messages.iter().rev() {
        let rows = message_rows(
            &message_event(message),
            message.contract.as_ref(),
            usize::from(inner.width.max(1)),
            tile.protocol,
            links,
        );
        if used + rows.len() > usize::from(room) && !shown.is_empty() {
            break;
        }
        used += rows.len();
        shown.push(rows);
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
}

fn message_event(message: &OverviewMessage<'_>) -> AgentEvent {
    let text = message.text.to_owned();
    if message.from_operator {
        AgentEvent::UserMessage { text }
    } else {
        AgentEvent::AgentMessage { text }
    }
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
            protocol: Protocol::default(),
            messages: Vec::new(),
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
        working.messages = vec![OverviewMessage {
            from_operator: false,
            text: "The checks are green.",
            contract: None,
        }];
        let mut waiting = tile("write the docs", InteractionStatus::Idle);
        waiting.newly_idle = true;
        let view = OverviewView {
            running_only: false,
            tiles: vec![working, waiting],
            selected: 0,
            links: LinkDisplay::Compact,
        };
        let (screen, feedback) = draw(&view, 100, 14);

        assert_eq!(feedback.rows, vec![2], "{screen}");
        assert!(screen.contains("·•· repair checkout"), "{screen}");
        assert!(screen.contains("Payments · styra/fix"), "{screen}");
        assert!(screen.contains("running 2m14s"), "{screen}");
        assert!(screen.contains("#bug"), "{screen}");
        assert!(screen.contains("« The checks are green."), "{screen}");
        assert!(screen.contains("● write the docs"), "{screen}");
        assert!(screen.contains("idle · NEWLY IDLE"), "{screen}");
        assert!(screen.contains("1 running · 1 idle"), "{screen}");
    }

    /// A tile shows the conversation's latest messages, as many as its room
    /// holds counted back from the end.
    #[test]
    fn tiles_show_as_much_of_the_conversation_as_fits() {
        let mut talking = tile("talking", InteractionStatus::Idle);
        talking.messages = ["oldest", "older", "fix the checkout", "newer", "newest"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| OverviewMessage {
                from_operator: index == 2,
                text,
                contract: None,
            })
            .collect();
        let view = OverviewView {
            running_only: false,
            tiles: vec![talking],
            selected: 0,
            links: LinkDisplay::Compact,
        };
        // Three description lines and three of messages inside the borders.
        let (screen, _) = draw(&view, 40, 10);
        assert!(screen.contains("» fix the checkout"), "{screen}");
        assert!(screen.contains("« newer"), "{screen}");
        assert!(screen.contains("« newest"), "{screen}");
        assert!(!screen.contains("older"), "{screen}");
        let row = |text: &str| screen.lines().position(|line| line.contains(text));
        assert!(row("fix the checkout") < row("newer"), "{screen}");
        assert!(row("newer") < row("newest"), "{screen}");
    }

    /// A message reads as it does in the interaction's own event list: its
    /// Markdown rendered over as many rows as it has lines, and an operator
    /// message marked with the answer shape it asked for.
    #[test]
    fn tile_messages_are_rendered_as_the_event_list_renders_them() {
        let mut talking = tile("talking", InteractionStatus::Idle);
        talking.messages = vec![
            OverviewMessage {
                from_operator: true,
                text: "List the flaky tests",
                contract: Some(Contract::Lines),
            },
            OverviewMessage {
                from_operator: false,
                text: "Two **flaky** tests:\n\n- `checkout`\n- `refund`",
                contract: None,
            },
        ];
        let view = OverviewView {
            running_only: false,
            tiles: vec![talking],
            selected: 0,
            links: LinkDisplay::Compact,
        };
        let (screen, _) = draw(&view, 50, 14);
        assert!(
            screen.contains("» List the flaky tests ⟨lines⟩"),
            "{screen}"
        );
        assert!(screen.contains("« Two flaky tests:"), "{screen}");
        assert!(!screen.contains("**"), "{screen}");
        assert!(!screen.contains('`'), "{screen}");
        let row = |text: &str| screen.lines().position(|line| line.contains(text));
        assert!(row("Two flaky") < row("checkout"), "{screen}");
        assert!(row("checkout") < row("refund"), "{screen}");
    }

    #[test]
    fn a_narrow_screen_stacks_the_tiles_in_one_column() {
        let view = OverviewView {
            running_only: false,
            tiles: vec![
                tile("first", InteractionStatus::Idle),
                tile("second", InteractionStatus::Idle),
            ],
            selected: 0,
            links: LinkDisplay::Compact,
        };
        let (screen, feedback) = draw(&view, 50, 20);
        assert_eq!(feedback.rows, vec![1, 1], "{screen}");
        let first = screen.lines().position(|line| line.contains("first"));
        let second = screen.lines().position(|line| line.contains("second"));
        assert!(first < second, "{screen}");
    }

    /// The tiles cover the whole frame: rows and columns share out what does
    /// not divide evenly, and a short last row stretches across the width.
    #[test]
    fn the_tiles_fill_the_whole_frame() {
        let mut view = OverviewView {
            running_only: false,
            tiles: vec![
                tile("first", InteractionStatus::Idle),
                tile("second", InteractionStatus::Idle),
                tile("third", InteractionStatus::Idle),
            ],
            // Off the tiles whose corners are checked, which are not drawn
            // thick then.
            selected: 0,
            links: LinkDisplay::Compact,
        };
        let (screen, feedback) = draw(&view, 151, 31);
        assert_eq!(feedback.rows, vec![2, 1], "{screen}");
        let cell = |x: usize, y: usize| screen.lines().nth(y).unwrap().chars().nth(x).unwrap();
        // The frame is wide enough for three columns, but three tiles use
        // two above one. Its inner area runs from (1, 1) to (149, 29).
        assert_eq!(
            cell(149, 1),
            '╮',
            "second tile meets the right border\n{screen}"
        );
        assert_eq!(cell(1, 29), '╰', "third tile meets the bottom\n{screen}");
        assert_eq!(cell(149, 29), '╯', "third tile spans the width\n{screen}");

        view.tiles.push(tile("fourth", InteractionStatus::Idle));
        let (screen, feedback) = draw(&view, 201, 31);
        assert_eq!(feedback.rows, vec![2, 2], "{screen}");
        let row = |name: &str| screen.lines().position(|line| line.contains(name)).unwrap();
        assert_eq!(row("first"), row("second"), "{screen}");
        assert_eq!(row("third"), row("fourth"), "{screen}");
        assert!(row("first") < row("third"), "{screen}");
    }

    /// The grid is as square as the width allows, and its rows differ by at
    /// most one tile, the longer ones on top.
    #[test]
    fn tiles_are_shared_out_as_evenly_as_they_go() {
        let wide = Rect::new(0, 0, 400, 100);
        let rows = |tiles| grid(wide, tiles).rows;
        assert_eq!(rows(1), [1]);
        assert_eq!(rows(2), [2]);
        assert_eq!(rows(3), [2, 1]);
        assert_eq!(rows(4), [2, 2]);
        assert_eq!(rows(5), [3, 2]);
        assert_eq!(rows(6), [3, 3]);
        for (width, height) in [(200, 50), (300, 40), (250, 20), (400, 14)] {
            assert_eq!(grid(Rect::new(0, 0, width, height), 6).rows, [3, 3]);
        }
        assert_eq!(rows(7), [3, 2, 2]);
        assert_eq!(rows(8), [3, 3, 2]);
        assert_eq!(rows(9), [3, 3, 3]);
        assert_eq!(rows(10), [4, 3, 3]);
        assert_eq!(rows(13), [4, 3, 3, 3]);
        // Only two tiles fit across, so seven take four rows of two.
        let narrow = Rect::new(0, 0, 2 * MIN_TILE_WIDTH, 100);
        assert_eq!(grid(narrow, 7).rows, [2, 2, 2, 1]);
    }

    /// Seven tiles are drawn three over two over two, the second and third
    /// rows sharing the width between two tiles each.
    #[test]
    fn a_short_row_stretches_its_tiles_across() {
        let names = ["one", "two", "three", "four", "five", "six", "seven"];
        let view = OverviewView {
            running_only: false,
            tiles: names
                .iter()
                .map(|name| tile(name, InteractionStatus::Idle))
                .collect(),
            selected: 0,
            links: LinkDisplay::Compact,
        };
        let (screen, feedback) = draw(&view, 150, 30);
        assert_eq!(feedback.rows, vec![3, 2, 2], "{screen}");
        let row = |name: &str| screen.lines().position(|line| line.contains(name)).unwrap();
        assert_eq!(row("one"), row("three"), "{screen}");
        assert_eq!(row("four"), row("five"), "{screen}");
        assert_eq!(row("six"), row("seven"), "{screen}");
        assert!(row("three") < row("four"), "{screen}");
        assert!(row("five") < row("six"), "{screen}");
        let column = |name: &str| {
            let line = screen.lines().nth(row(name)).unwrap();
            line[..line.find(name).unwrap()].chars().count()
        };
        // Two tiles split the row at its middle, between where the top
        // row's second and third tiles start.
        assert!(column("two") < column("five"), "{screen}");
        assert!(column("five") < column("three"), "{screen}");
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
            running_only: false,
            tiles: names
                .iter()
                .map(|name| tile(name, InteractionStatus::Idle))
                .collect(),
            selected: 5,
            links: LinkDisplay::Compact,
        };
        // One column, two seven-row tiles high.
        let (screen, _) = draw(&view, 40, 16);
        assert!(screen.contains("six"), "{screen}");
        assert!(!screen.contains("one"), "{screen}");
    }

    #[test]
    fn an_empty_fleet_says_so() {
        let view = OverviewView {
            running_only: false,
            tiles: Vec::new(),
            selected: 0,
            links: LinkDisplay::Compact,
        };
        let (screen, _) = draw(&view, 60, 5);
        assert!(
            screen.contains("no interaction is running or idle"),
            "{screen}"
        );
    }

    #[test]
    fn an_empty_running_filter_shows_its_state() {
        let view = OverviewView {
            running_only: true,
            tiles: Vec::new(),
            selected: 0,
            links: LinkDisplay::Compact,
        };
        let (screen, feedback) = draw(&view, 100, 5);
        assert!(screen.contains("r running only: on"), "{screen}");
        assert!(
            screen.contains("no interaction is currently running"),
            "{screen}"
        );
        assert!(feedback.rows.is_empty());
    }
}
