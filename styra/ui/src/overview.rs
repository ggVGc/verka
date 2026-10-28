//! The overview: every active interaction laid out as a tile in a grid, so
//! the whole fleet can be watched at once instead of one row at a time.

#[cfg(doc)]
use crate::event_list::EventEntry;
use crate::event_list::{faded_style, message_rows, status_tail, BranchLink, EventListStatus};
use crate::interactions::{status_marker, InteractionStatus};
use crate::markdown::LinkDisplay;
use crate::search::{self, SearchView};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::borrow::Cow;
use styra_protocol::event::{AgentEvent, Protocol};
use styra_protocol::Contract;

/// Narrower than this and a tile cannot hold its name and selection on a line
/// each, so the grid takes fewer columns instead.
const MIN_TILE_WIDTH: u16 = 38;
/// Room for the border, the two description lines, two of the message and
/// the status tail. Shorter than this and the grid scrolls instead.
const MIN_TILE_HEIGHT: u16 = 7;
/// How far, in tenths, a tile off the cursor fades toward the background, as
/// far as the navigator fades its rows off the cursor.
const UNSELECTED_FADE: u16 = 2;

pub struct OverviewTile<'a> {
    pub name: Cow<'a, str>,
    pub workspace: Cow<'a, str>,
    /// The provider, model and effort, as the launcher names them.
    pub selection: Cow<'a, str>,
    /// As [`crate::interactions::InteractionRow::Interaction::branch`].
    pub branch: Option<&'a str>,
    pub status: InteractionStatus,
    /// The line its own event list ends with, drawn under the messages as
    /// the list draws it under the last entry.
    pub tail: EventListStatus,
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

/// One entry of a tile's conversation, drawn as the interaction's own event
/// list draws it.
pub struct OverviewMessage<'a> {
    /// A message from either side, or a branch marker.
    pub event: AgentEvent,
    /// The answer shape an operator message asked for, if it asked for one.
    pub contract: Option<Contract>,
    /// For a branch marker, where it leads, as [`EventEntry::branch`].
    pub branch: BranchLink<'a>,
}

pub struct OverviewView<'a> {
    pub tiles: Vec<OverviewTile<'a>>,
    pub running_only: bool,
    /// The name of the one Workspace the grid is narrowed to, or `None` when
    /// it shows every Workspace.
    pub workspace_scope: Option<Cow<'a, str>>,
    /// The tile under the cursor, an index into [`Self::tiles`].
    pub selected: usize,
    /// How links in the messages are drawn, as in the event list.
    pub links: LinkDisplay,
    /// The `/` search, marking matching words across every tile as it marks
    /// them in the event list. See [`crate::search`].
    pub search: SearchView<'a>,
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
    let counts = format!(
        " · {running} running · {} idle · r running only: {} · ? keys ",
        view.tiles.len() - running,
        if view.running_only { "on" } else { "off" }
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
    // marks selection, and the attached interaction has the navigator's `•`.
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
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
            tail: EventListStatus::Idle {
                reason: None,
                failed: false,
            },
            current: false,
            newly_idle: false,
            rate_limited: None,
            uncommitted: false,
            tags: &[],
            protocol: Protocol::default(),
            messages: Vec::new(),
        }
    }

    fn operator(text: &str) -> OverviewMessage<'static> {
        OverviewMessage {
            event: AgentEvent::UserMessage { text: text.into() },
            contract: None,
            branch: BranchLink::default(),
        }
    }

    fn agent(text: &str) -> OverviewMessage<'static> {
        OverviewMessage {
            event: AgentEvent::AgentMessage { text: text.into() },
            contract: None,
            branch: BranchLink::default(),
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
    fn tile_frames_show_state_even_when_selected() {
        let cases = [
            (
                InteractionStatus::Running { events: 1 },
                false,
                false,
                theme::RUNNING,
            ),
            (
                InteractionStatus::Idle,
                false,
                false,
                theme::OVERVIEW_IDLE_FRAME,
            ),
            (
                InteractionStatus::Pending,
                true,
                false,
                theme::OVERVIEW_NEWLY_IDLE_FRAME,
            ),
            (
                InteractionStatus::Background,
                false,
                false,
                theme::MUTED_WARNING,
            ),
            (InteractionStatus::Error, false, false, theme::ERROR),
            (InteractionStatus::Ended, false, false, theme::INACTIVE),
            (
                InteractionStatus::Idle,
                true,
                true,
                theme::STOP_RATE_LIMITED,
            ),
        ];
        for (status, newly_idle, rate_limited, color) in cases {
            let mut interaction = tile("interaction", status);
            interaction.newly_idle = newly_idle;
            interaction.rate_limited = rate_limited.then(|| "1h".into());
            for selected in [false, true] {
                let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
                terminal
                    .draw(|frame| {
                        render_tile(
                            frame,
                            &interaction,
                            selected,
                            frame.area(),
                            LinkDisplay::Compact,
                            None,
                        )
                    })
                    .unwrap();
                let corner = &terminal.backend().buffer()[(0, 0)];
                let expected = if selected {
                    Style::default().fg(color)
                } else {
                    faded_style(Style::default().fg(color), UNSELECTED_FADE)
                };
                assert_eq!(
                    Some(corner.fg),
                    expected.fg,
                    "{status:?}, selected={selected}"
                );
                assert_eq!(corner.symbol(), if selected { "┏" } else { "╭" });
            }
        }
    }

    #[test]
    fn tiles_off_the_cursor_are_faded() {
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: vec![
                tile("first", InteractionStatus::Idle),
                tile("second", InteractionStatus::Idle),
            ],
            selected: 1,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &view, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let frame = theme::OVERVIEW_IDLE_FRAME;
        let faded = faded_style(Style::default().fg(frame), UNSELECTED_FADE).fg;
        assert_ne!(faded, Some(frame));
        // Inside the outer border, the first tile spans x 1..50, the second
        // 50..99; their top borders run along row 1.
        let fg = |x| Some(buffer[(x, 1)].fg);
        assert_eq!(fg(1), faded);
        assert_eq!(fg(49), faded);
        assert_eq!(fg(50), Some(frame));
        assert_eq!(fg(98), Some(frame));
        // The text inside fades too, not only the frame.
        let name = (1..50).find(|&x| buffer[(x, 1)].symbol() == "f").unwrap();
        assert_eq!(
            Some(buffer[(name, 1)].fg),
            faded_style(Style::default().fg(theme::TEXT), UNSELECTED_FADE).fg
        );
    }

    #[test]
    fn tiles_say_where_each_interaction_is_and_what_it_last_said() {
        let tags = vec!["bug".to_owned()];
        let mut working = tile("repair checkout", InteractionStatus::Running { events: 1 });
        working.branch = Some("styra/fix");
        working.tags = &tags;
        working.tail = EventListStatus::Running {
            elapsed: std::time::Duration::from_secs(134),
            quiet: None,
            events: 1,
        };
        working.messages = vec![agent("The checks are green.")];
        let mut waiting = tile("write the docs", InteractionStatus::Idle);
        waiting.newly_idle = true;
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: vec![working, waiting],
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        let (screen, feedback) = draw(&view, 100, 14);

        assert_eq!(feedback.rows, vec![2], "{screen}");
        assert!(screen.contains("·●· repair checkout"), "{screen}");
        assert!(screen.contains("Payments · styra/fix"), "{screen}");
        assert!(screen.contains("·●· working 2m14s"), "{screen}");
        assert!(
            !screen.contains("┃running"),
            "the status is the tail's alone\n{screen}"
        );
        assert!(screen.contains("#bug"), "{screen}");
        assert!(screen.contains("« The checks are green."), "{screen}");
        assert!(screen.contains("● write the docs"), "{screen}");
        assert!(screen.contains("NEWLY IDLE"), "{screen}");
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
            .map(|(index, text)| {
                if index == 2 {
                    operator(text)
                } else {
                    agent(text)
                }
            })
            .collect();
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: vec![talking],
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        // Two description lines, three of messages and the status tail
        // inside the borders.
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
                contract: Some(Contract::Lines),
                ..operator("List the flaky tests")
            },
            agent("Two **flaky** tests:\n\n- `checkout`\n- `refund`"),
        ];
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: vec![talking],
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
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

    /// Each tile ends with its event list's status tail, right under its
    /// messages, or along the bottom when they fill the tile.
    #[test]
    fn tiles_end_with_the_event_list_status_tail() {
        let mut working = tile("working", InteractionStatus::Running { events: 1 });
        working.tail = EventListStatus::Running {
            elapsed: std::time::Duration::from_secs(134),
            quiet: None,
            events: 1,
        };
        working.messages = vec![agent("Running the checks.")];
        let mut chatty = tile("chatty", InteractionStatus::Idle);
        chatty.tail = EventListStatus::Idle {
            reason: Some("interrupted".into()),
            failed: false,
        };
        chatty.messages = (0..20).map(|n| agent(&format!("message {n}"))).collect();
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: vec![working, chatty],
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        let (screen, _) = draw(&view, 100, 14);
        let row = |text: &str| screen.lines().position(|line| line.contains(text));
        assert!(screen.contains("working 2m14s"), "{screen}");
        assert_eq!(
            row("working 2m14s"),
            row("Running the checks.").map(|row| row + 1),
            "{screen}"
        );
        assert!(screen.contains("── idle · interrupted ──"), "{screen}");
        assert_eq!(
            row("message 19"),
            row("idle · interrupted").map(|row| row - 1)
        );
        assert_eq!(row("idle · interrupted"), Some(11), "{screen}");
    }

    #[test]
    fn a_narrow_screen_stacks_the_tiles_in_one_column() {
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: vec![
                tile("first", InteractionStatus::Idle),
                tile("second", InteractionStatus::Idle),
            ],
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
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
            workspace_scope: None,
            tiles: vec![
                tile("first", InteractionStatus::Idle),
                tile("second", InteractionStatus::Idle),
                tile("third", InteractionStatus::Idle),
            ],
            // Off the tiles whose corners are checked, which are not drawn
            // thick then.
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
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
            workspace_scope: None,
            tiles: names
                .iter()
                .map(|name| tile(name, InteractionStatus::Idle))
                .collect(),
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
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
            workspace_scope: None,
            tiles: names
                .iter()
                .map(|name| tile(name, InteractionStatus::Idle))
                .collect(),
            selected: 5,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        // One column, two seven-row tiles high.
        let (screen, _) = draw(&view, 40, 16);
        assert!(screen.contains("six"), "{screen}");
        assert!(!screen.contains("one"), "{screen}");
    }

    /// A branch marker in the conversation reads as it does in the event
    /// list: by the live name and state of where it leads, not the name it
    /// was written with.
    #[test]
    fn a_tile_shows_the_branch_markers_in_its_conversation() {
        use crate::chrome::StopTone;
        use crate::interactions::InteractionState;
        use styra_protocol::event::BranchDirection;
        use styra_protocol::CompletionState;
        let mut talking = tile("talking", InteractionStatus::Idle);
        talking.messages = vec![
            agent("Split the retry work off."),
            OverviewMessage {
                event: AgentEvent::Branched {
                    direction: BranchDirection::To,
                    session: "4f1c-session-id".into(),
                    name: Some("stale snapshot".into()),
                },
                contract: None,
                branch: BranchLink {
                    name: Some("Retry backoff"),
                    state: Some(InteractionState {
                        status: InteractionStatus::Stopped(StopTone::Paused),
                        stop_reason: Some("paused".into()),
                        rate_limited: None,
                        newly_idle: false,
                        completion: CompletionState::Active,
                    }),
                    message: Some("Backoff now doubles."),
                },
            },
        ];
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: vec![talking],
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        let (screen, _) = draw(&view, 60, 14);
        assert!(screen.contains("« Split the retry work off."), "{screen}");
        assert!(
            screen.contains("Branch to:  ■  Retry backoff  paused"),
            "{screen}"
        );
        assert!(screen.contains("« Backoff now doubles."), "{screen}");
        assert!(!screen.contains("stale snapshot"), "{screen}");
        let row = |text: &str| screen.lines().position(|line| line.contains(text));
        assert!(row("Split the retry") < row("Branch to:"), "{screen}");
    }

    /// A search marks matching words across every tile — names, places and
    /// messages alike — and shows its prompt along the bottom of the grid.
    #[test]
    fn a_search_marks_matches_in_every_tile() {
        let mut fixing = tile("fix retry_backoff", InteractionStatus::Idle);
        fixing.messages = vec![agent("Moved it to src/retry.rs")];
        let mut other = tile("write the docs", InteractionStatus::Idle);
        other.branch = Some("styra/retry-docs");
        let view = OverviewView {
            running_only: false,
            tiles: vec![fixing, other],
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView {
                query: Some("retry"),
                typing: true,
            },
            workspace_scope: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &view, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut marked = Vec::new();
        for y in 0..14 {
            let mut word = String::new();
            for x in 0..100 {
                let cell = &buffer[(x, y)];
                if cell.bg == theme::ACCENT {
                    word.push_str(cell.symbol());
                } else if !word.is_empty() {
                    marked.push(std::mem::take(&mut word));
                }
            }
            if !word.is_empty() {
                marked.push(word);
            }
        }
        assert!(marked.contains(&"retry_backoff".to_owned()), "{marked:?}");
        assert!(marked.contains(&"src/retry.rs".to_owned()), "{marked:?}");
        assert!(
            marked.contains(&"styra/retry-docs".to_owned()),
            "{marked:?}"
        );
        assert!(!marked
            .iter()
            .any(|word| word.contains("docs") && !word.contains("retry")));
        let (screen, _) = draw(&view, 100, 14);
        assert!(screen.contains("/retry▌"), "{screen}");
    }

    #[test]
    fn an_empty_fleet_says_so() {
        let view = OverviewView {
            running_only: false,
            workspace_scope: None,
            tiles: Vec::new(),
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        let (screen, _) = draw(&view, 60, 5);
        assert!(
            screen.contains("no interaction is running or idle"),
            "{screen}"
        );
    }

    #[test]
    fn the_grid_shows_the_shared_workspace_scope() {
        let mut view = OverviewView {
            running_only: false,
            workspace_scope: Some("Payments".into()),
            tiles: Vec::new(),
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
        };
        let (screen, _) = draw(&view, 120, 5);
        assert!(screen.contains("Workspace: Payments"), "{screen}");
        assert!(screen.contains("w scope: this Workspace"), "{screen}");
        assert!(screen.starts_with('╔'), "{screen}");
        view.workspace_scope = None;
        let (screen, _) = draw(&view, 120, 5);
        assert!(screen.contains("w scope: All"), "{screen}");
        assert!(!screen.contains("Workspace:"), "{screen}");
        assert!(screen.starts_with('┌'), "{screen}");
    }

    #[test]
    fn an_empty_running_filter_shows_its_state() {
        let view = OverviewView {
            running_only: true,
            workspace_scope: None,
            tiles: Vec::new(),
            selected: 0,
            links: LinkDisplay::Compact,
            search: SearchView::default(),
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
