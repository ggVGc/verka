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

fn overview<'a>() -> OverviewView<'a> {
    OverviewView {
        tiles: Vec::new(),
        layout: OverviewLayout::Auto,
        running_only: false,
        time_filter: "time: disabled",
        workspace_scope: None,
        selected: 0,
        links: LinkDisplay::Compact,
        search: SearchView::default(),
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
            assert_eq!(corner.symbol(), if selected { "┏" } else { "┌" });
        }
    }
}

#[test]
fn tiles_off_the_cursor_are_faded() {
    let view = OverviewView {
        tiles: vec![
            tile("first", InteractionStatus::Idle),
            tile("second", InteractionStatus::Idle),
        ],
        selected: 1,
        ..overview()
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
        tiles: vec![working, waiting],
        ..overview()
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
        tiles: vec![talking],
        ..overview()
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
        tiles: vec![talking],
        ..overview()
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
        tiles: vec![working, chatty],
        ..overview()
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
        tiles: vec![
            tile("first", InteractionStatus::Idle),
            tile("second", InteractionStatus::Idle),
        ],
        ..overview()
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
        tiles: vec![
            tile("first", InteractionStatus::Idle),
            tile("second", InteractionStatus::Idle),
            tile("third", InteractionStatus::Idle),
        ],
        // Off the tiles whose corners are checked, which are not drawn
        // thick then.
        ..overview()
    };
    let (screen, feedback) = draw(&view, 151, 31);
    assert_eq!(feedback.rows, vec![2, 1], "{screen}");
    let cell = |x: usize, y: usize| screen.lines().nth(y).unwrap().chars().nth(x).unwrap();
    // The frame is wide enough for three columns, but three tiles use
    // two above one. Its inner area runs from (1, 1) to (149, 29).
    assert_eq!(
        cell(149, 1),
        '┐',
        "second tile meets the right border\n{screen}"
    );
    assert_eq!(cell(1, 29), '└', "third tile meets the bottom\n{screen}");
    assert_eq!(cell(149, 29), '┘', "third tile spans the width\n{screen}");

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

#[test]
fn two_columns_stay_aligned_and_scroll_to_the_selected_tile() {
    let view = OverviewView {
        tiles: [
            "window-1", "window-2", "window-3", "window-4", "window-5", "window-6", "window-7",
        ]
        .into_iter()
        .map(|name| tile(name, InteractionStatus::Idle))
        .collect(),
        layout: OverviewLayout::TwoColumns,
        selected: 6,
        ..overview()
    };
    let (screen, feedback) = draw(&view, 100, 23);
    assert_eq!(feedback.rows, [2, 2, 2, 1]);
    assert!(!screen.contains("window-1"), "{screen}");
    assert!(screen.contains("window-7"), "{screen}");
    let column = |name: &str| screen.lines().find_map(|line| line.find(name)).unwrap();
    assert_eq!(column("window-3"), column("window-7"));
    assert!(column("window-4") > column("window-7"));
    let (_, feedback) = draw(&view, 50, 23);
    assert_eq!(feedback.rows, [1; 7]);
}

/// Seven tiles are drawn three over two over two, the second and third
/// rows sharing the width between two tiles each.
#[test]
fn a_short_row_stretches_its_tiles_across() {
    let names = ["one", "two", "three", "four", "five", "six", "seven"];
    let view = OverviewView {
        tiles: names
            .iter()
            .map(|name| tile(name, InteractionStatus::Idle))
            .collect(),
        ..overview()
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
        tiles: names
            .iter()
            .map(|name| tile(name, InteractionStatus::Idle))
            .collect(),
        selected: 5,
        ..overview()
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
                worktree: None,
                tags: &[],
                children: Vec::new(),
            },
        },
    ];
    let view = OverviewView {
        tiles: vec![talking],
        ..overview()
    };
    let (screen, _) = draw(&view, 60, 14);
    assert!(screen.contains("« Split the retry work off."), "{screen}");
    assert!(screen.contains("▌  ■  Retry backoff  paused"), "{screen}");
    assert!(screen.contains("« Backoff now doubles."), "{screen}");
    assert!(!screen.contains("stale snapshot"), "{screen}");
    let row = |text: &str| screen.lines().position(|line| line.contains(text));
    assert!(row("Split the retry") < row("Retry backoff"), "{screen}");
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
        tiles: vec![fixing, other],
        search: SearchView {
            query: Some("retry"),
            typing: true,
        },
        workspace_scope: None,
        ..overview()
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
    let view = OverviewView { ..overview() };
    let (screen, _) = draw(&view, 60, 5);
    assert!(
        screen.contains("no interaction is running or idle"),
        "{screen}"
    );
}

#[test]
fn the_grid_shows_the_shared_workspace_scope() {
    let mut view = OverviewView {
        workspace_scope: Some("Payments".into()),
        ..overview()
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
        ..overview()
    };
    let (screen, feedback) = draw(&view, 100, 5);
    assert!(screen.contains("r running only: on"), "{screen}");
    assert!(
        screen.contains("no interaction is currently running"),
        "{screen}"
    );
    assert!(feedback.rows.is_empty());
}
