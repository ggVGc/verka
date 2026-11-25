use super::*;
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

fn rendered(view: &InteractionNavigator<'_>) -> String {
    rendered_at(view, 80)
}

/// The screen in a pane `width` cells wide, for a row too long for the
/// usual one.
fn rendered_at(view: &InteractionNavigator<'_>, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, view, frame.area());
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// The rendered screen with each run of spaces cut to one, for checking
/// what follows what on a row regardless of how its columns are padded.
fn squeezed(screen: &str) -> String {
    screen
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A lead column, padded out to the width it is held to in the test
/// pane: half the 78 cells inside its border, short of
/// [`LEAD_COLUMN_WIDTH`].
fn lead(text: &str) -> String {
    format!("{text:<39}")
}

fn row(
    name: &'static str,
    depth: usize,
    last_message: Option<&'static str>,
) -> InteractionRow<'static> {
    InteractionRow::Interaction {
        name: name.into(),
        branched_from: None,
        branches: 0,
        branch_link: BranchLink::None,
        depth,
        model: "gpt-5",
        branch: None,
        expected_branch: None,
        worktree: None,
        same_as_source: false,
        shared_worktree: false,
        into: None,
        status: InteractionStatus::Stopped(StopTone::Paused),
        selected: false,
        newly_idle: false,
        flashing: false,
        stop_reason: None,
        rate_limited: None,
        uncommitted: false,
        completion: CompletionState::Active,
        tags: &[],
        last_message,
    }
}

#[test]
fn a_switched_branch_is_visible() {
    for actual in ["other", "detached head"] {
        let mut switched = row("task", 0, None);
        if let InteractionRow::Interaction {
            branch,
            expected_branch,
            ..
        } = &mut switched
        {
            *branch = Some(actual);
            *expected_branch = Some("assigned");
        }
        let view = InteractionNavigator {
            rows: vec![switched],
            ..many_rows(0)
        };
        let screen = squeezed(&rendered_at(&view, 120));
        assert!(
            screen.contains(&format!("task {actual} (≠ assigned) gpt-5")),
            "{screen}"
        );
    }
}

/// A linked worktree is named ahead of the branch, also on a row hanging
/// beneath its source.
#[test]
fn a_worktree_is_named_in_the_branch_column() {
    for depth in [0, 1] {
        let mut named = row("task", depth, None);
        if let InteractionRow::Interaction {
            branch, worktree, ..
        } = &mut named
        {
            *branch = Some("feature");
            *worktree = Some("auth".into());
        }
        let view = InteractionNavigator {
            rows: vec![named],
            ..many_rows(0)
        };
        let screen = squeezed(&rendered_at(&view, 120));
        assert!(screen.contains("task [auth] feature gpt-5"), "{screen}");
    }
}

/// A worktree's row says where its work goes after its branch.
#[test]
fn a_worktree_names_its_target() {
    for (actual, shown) in [
        ("styra/feature", "task [feature] → main gpt-5"),
        ("other", "task [feature] other → main gpt-5"),
    ] {
        let mut named = row("task", 0, None);
        if let InteractionRow::Interaction {
            branch,
            worktree,
            into,
            ..
        } = &mut named
        {
            *branch = Some(actual);
            *worktree = Some("feature".into());
            *into = Some("main".into());
        }
        let view = InteractionNavigator {
            rows: vec![named],
            ..many_rows(0)
        };
        let screen = squeezed(&rendered_at(&view, 120));
        assert!(screen.contains(shown), "{screen}");
    }
}

/// A branch on its source's worktree and branch points up at the source's
/// branch column rather than repeating it, unless it has switched away.
#[test]
fn a_branch_sharing_its_sources_checkout_points_up() {
    /// A branch on its source's worktree `auth` and branch `feature`,
    /// assigned `expected` when it has switched away from it.
    fn sharing(expected: Option<&'static str>) -> InteractionRow<'static> {
        let mut row = row("task", 1, None);
        if let InteractionRow::Interaction {
            branch,
            expected_branch,
            worktree,
            same_as_source,
            ..
        } = &mut row
        {
            *branch = Some("feature");
            *expected_branch = expected;
            *worktree = Some("auth".into());
            *same_as_source = true;
        }
        row
    }
    fn drawn(row: InteractionRow<'static>) -> String {
        let view = InteractionNavigator {
            rows: vec![row],
            ..many_rows(0)
        };
        squeezed(&rendered_at(&view, 120))
    }

    let screen = drawn(sharing(None));
    assert!(screen.contains("task ↑ gpt-5"), "{screen}");

    let screen = drawn(sharing(Some("assigned")));
    assert!(
        screen.contains("task [auth] feature (≠ assigned) gpt-5"),
        "{screen}"
    );
}

/// A worktree named after its branch, or its branch's last part, is not
/// followed by the branch again, unless the checkout has switched away.
#[test]
fn a_worktree_named_after_its_branch_is_named_once() {
    for (actual, expected, shown) in [
        ("styra/feature", None, "task [feature] gpt-5"),
        (
            "other/featured",
            None,
            "task [feature] other/featured gpt-5",
        ),
        ("feature", None, "task [feature] gpt-5"),
        (
            "feature",
            Some("assigned"),
            "task [feature] feature (≠ assigned) gpt-5",
        ),
    ] {
        let mut named = row("task", 0, None);
        if let InteractionRow::Interaction {
            branch,
            expected_branch,
            worktree,
            ..
        } = &mut named
        {
            *branch = Some(actual);
            *expected_branch = expected;
            *worktree = Some("feature".into());
        }
        let view = InteractionNavigator {
            rows: vec![named],
            ..many_rows(0)
        };
        let screen = squeezed(&rendered_at(&view, 120));
        assert!(screen.contains(shown), "{screen}");
    }
}

/// A branch too long for a narrow pane loses its middle, keeping the
/// start and end that tell it apart, before the name is cut at all.
#[test]
fn a_long_branch_is_cut_in_the_middle() {
    let title = "a title that fits its column";
    let mut long = row(title, 0, None);
    if let InteractionRow::Interaction { branch, .. } = &mut long {
        *branch = Some("feature/start-of-a-very-long-branch-name-end");
    }
    let view = InteractionNavigator {
        rows: vec![long],
        ..many_rows(0)
    };
    let screen = squeezed(&rendered_at(&view, 70));
    assert!(screen.contains(title), "{screen}");
    assert!(
        screen.contains("feature/start-of…g-branch-name-end"),
        "{screen}"
    );
}

#[test]
fn time_filter_is_visible_in_the_title() {
    for label in ["time: disabled", "past day", "past 3 days", "past week"] {
        let view = InteractionNavigator {
            time_filter: label,
            ..many_rows(0)
        };
        let screen = rendered_at(&view, 120);
        assert!(screen.contains(label), "{screen}");
    }
}

/// A branch hangs beneath its source with the Session picker's tree edge,
/// and its last message is indented along with it.
#[test]
fn a_branch_is_drawn_nested_beneath_its_source() {
    let view = InteractionNavigator {
        rows: vec![row("source", 0, None), row("branch", 1, Some("forked"))],
        ..many_rows(0)
    };
    let screen = rendered(&view);
    assert!(
        screen.contains(&format!("┃{}  gpt-5", lead(" ■  source"))),
        "{screen}"
    );
    assert!(
        screen.contains(&format!("┃{}  gpt-5", lead("  └─  ■  branch"))),
        "{screen}"
    );
    assert!(screen.contains("┃         « forked"), "{screen}");
}

#[test]
fn renders_grouping_activity_and_continuation_rows() {
    let tags = vec!["bug".into(), "urgent".into()];
    let view = InteractionNavigator {
        scope: "All".into(),
        all_workspaces: true,
        completion_filter: "completed hidden".into(),
        time_filter: "time: disabled",
        tag_filter: &[],
        filter: None,
        typing_filter: false,
        requested_offset: Some(0),
        scrolled_away: false,
        rows: vec![
            InteractionRow::Workspace("Payments".into()),
            InteractionRow::Interaction {
                name: "repair checkout".into(),
                branched_from: None,
                branches: 0,
                branch_link: BranchLink::None,
                depth: 0,
                model: "gpt-5",
                branch: Some("fix"),
                expected_branch: None,
                worktree: None,
                same_as_source: false,
                shared_worktree: false,
                into: None,
                status: InteractionStatus::Running { events: 2 },
                selected: true,
                newly_idle: true,
                flashing: false,
                stop_reason: None,
                rate_limited: None,
                uncommitted: false,
                completion: CompletionState::Active,
                tags: &tags,
                last_message: Some("The checks are green."),
            },
        ],
    };
    let screen = rendered_at(&view, 120);
    assert!(screen.contains("Payments"), "{screen}");
    assert!(
        squeezed(&screen).contains("repair checkout fix #bug #urgent NEWLY IDLE gpt-5"),
        "{screen}"
    );
    assert!(screen.contains("  NEWLY IDLE"), "{screen}");
    assert!(screen.contains("The checks are green."), "{screen}");
}

#[test]
fn the_dropdown_height_does_not_depend_on_its_rows() {
    assert_eq!(height(40), 20);
    assert_eq!(height(4), 3);
    assert_eq!(height(2), 2);
}

fn placed(
    name: &'static str,
    depth: usize,
    branched_from: Option<&'static str>,
) -> InteractionRow<'static> {
    InteractionRow::Interaction {
        name: name.into(),
        depth,
        branched_from: branched_from.map(Cow::Borrowed),
        branches: 0,
        branch_link: BranchLink::None,
        model: "sonnet",
        branch: Some("feature"),
        expected_branch: None,
        worktree: None,
        same_as_source: false,
        shared_worktree: false,
        into: None,
        status: InteractionStatus::Idle,
        selected: false,
        newly_idle: false,
        flashing: false,
        stop_reason: None,
        rate_limited: None,
        uncommitted: false,
        completion: CompletionState::Active,
        tags: &[],
        last_message: None,
    }
}

fn lines(view: &InteractionNavigator<'_>) -> Vec<String> {
    rendered(view)
        .chars()
        .collect::<Vec<_>>()
        .chunks(80)
        .map(|line| line.iter().collect::<String>())
        .collect()
}

/// Rows step in under a Workspace heading, a branch beneath its source,
/// and a message line beneath its row's name.
#[test]
fn rows_step_in_under_their_workspace_and_source() {
    let mut branch = placed("branch", 1, None);
    if let InteractionRow::Interaction { last_message, .. } = &mut branch {
        *last_message = Some("preview");
    }
    let all = InteractionNavigator {
        all_workspaces: true,
        rows: vec![
            InteractionRow::Workspace("Payments".into()),
            placed("source", 0, None),
            branch,
        ],
        ..many_rows(0)
    };
    let screen = lines(&all);
    assert!(screen[1].starts_with("┃Payments ───"), "{screen:#?}");
    assert!(screen[1].ends_with("─┃"), "{screen:#?}");
    assert!(
        screen[2].starts_with(&format!("┃{}  feature  sonnet", lead("  ●  source"))),
        "{screen:#?}"
    );
    assert!(
        screen[3].starts_with(&format!("┃{}  feature  sonnet", lead("   └─  ●  branch"))),
        "{screen:#?}"
    );
    assert!(screen[4].starts_with("┃          « preview"), "{screen:#?}");

    // In Workspace scope there is no Workspace heading to step in under.
    let one = InteractionNavigator {
        rows: vec![placed("alone", 0, None)],
        ..many_rows(0)
    };
    let screen = lines(&one);
    assert!(
        screen[1].starts_with(&format!("┃{}  feature  sonnet", lead(" ●  alone"))),
        "{screen:#?}"
    );
}

/// A branch drawn apart from its source is tied to the cursor from
/// either end: the other end's name is underlined, and the fork icons at
/// both ends are lit, so a source reads apart from a branch.
#[test]
fn a_branch_is_marked_from_either_end_of_it() {
    // `rework` on line 1 counts `spike`, on line 2, as its branch.
    let drawn = |cursor_on_source: bool| {
        let link = |source: bool| match (source, cursor_on_source) {
            (true, false) => BranchLink::Source,
            (false, true) => BranchLink::Branch,
            _ => BranchLink::None,
        };
        let mut rows = vec![
            placed("rework", 0, None),
            placed("spike", 0, Some("rework")),
        ];
        for (index, row) in rows.iter_mut().enumerate() {
            if let InteractionRow::Interaction {
                branches,
                branch_link,
                selected,
                last_message,
                ..
            } = row
            {
                let source = index == 0;
                *branches = source.into();
                *branch_link = link(source);
                *selected = source == cursor_on_source;
                *last_message = None;
            }
        }
        let view = InteractionNavigator {
            rows,
            ..many_rows(0)
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &view, frame.area());
            })
            .unwrap();
        terminal.backend().buffer().clone()
    };
    let cell = |buffer: &Buffer, symbol: &str, y: u16| {
        let x = (0..80)
            .find(|&x| buffer[(x, y)].symbol() == symbol)
            .expect("line draws the symbol");
        buffer[(x, y)].clone()
    };
    let lit = |selected: bool| {
        let style = Style::default().fg(theme::BRANCH_LINK);
        if selected {
            style
        } else {
            faded_style(style, UNSELECTED_ROW_FADE)
        }
        .fg
        .unwrap()
    };
    for cursor_on_source in [true, false] {
        let buffer = drawn(cursor_on_source);
        assert_eq!(cell(&buffer, "↱", 1).fg, lit(cursor_on_source));
        assert_eq!(cell(&buffer, "↳", 2).fg, lit(!cursor_on_source));
        let (other, y) = if cursor_on_source { ("s", 2) } else { ("r", 1) };
        assert!(cell(&buffer, other, y)
            .modifier
            .contains(Modifier::UNDERLINED));
    }
}

/// A row sharing the cursor's worktree has the worktree's name lifted as
/// the cursor's row has it, unfaded; a row in another worktree does not.
#[test]
fn a_worktree_shared_with_the_cursor_is_highlighted() {
    let worktree_row = |name: &'static str, worktree: &'static str, shared: bool| {
        let mut row = row(name, 0, None);
        if let InteractionRow::Interaction {
            branch,
            worktree: named,
            shared_worktree,
            last_message,
            ..
        } = &mut row
        {
            *branch = Some("feature");
            *named = Some(worktree.into());
            *shared_worktree = shared;
            *last_message = None;
        }
        row
    };
    let mut rows = vec![
        worktree_row("one", "auth", false),
        worktree_row("two", "auth", true),
        worktree_row("three", "docs", false),
    ];
    if let InteractionRow::Interaction { selected, .. } = &mut rows[0] {
        *selected = true;
    }
    let view = InteractionNavigator {
        rows,
        ..many_rows(0)
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 5)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &view, frame.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let worktree_cell = |y: u16| {
        let x = (0..100)
            .find(|&x| buffer[(x, y)].symbol() == "[")
            .expect("the row names its worktree");
        buffer[(x + 1, y)].clone()
    };
    let lifted = shared_worktree_style().fg.unwrap();
    assert_eq!(worktree_cell(1).fg, lifted);
    assert_eq!(worktree_cell(2).fg, lifted);
    assert!(worktree_cell(2).modifier.contains(Modifier::BOLD));
    assert_ne!(worktree_cell(3).fg, lifted);
}

/// Long branch names remain whole, and every row reserves the same
/// branch column even when it has no branch of its own.
#[test]
fn a_long_branch_is_shown_in_full() {
    let branch_name = "styra/align-live-interactions-columns-1792571015193";
    let mut long = placed("solo", 0, None);
    if let InteractionRow::Interaction { branch, .. } = &mut long {
        *branch = Some(branch_name);
    }
    let mut first = placed("first", 0, None);
    if let InteractionRow::Interaction { branch, .. } = &mut first {
        *branch = None;
    }
    let view = InteractionNavigator {
        rows: vec![placed("alone", 0, None), long, first],
        ..many_rows(0)
    };
    for width in [80, 120, 200] {
        let widths = column_widths(&view, width);
        assert_eq!(widths.branch, crate::text::width(branch_name) + 2);
        let screen = rendered_at(&view, width as u16);
        assert!(screen.contains(branch_name), "{screen}");
        for row in &view.rows {
            if matches!(row, InteractionRow::Interaction { .. }) {
                let item = row_item(row, false, width, &widths);
                let line = item[0].to_string();
                let branch = line
                    .chars()
                    .skip(widths.lead)
                    .take(widths.branch)
                    .collect::<String>();
                if line.contains("solo") {
                    assert_eq!(branch, format!("  {branch_name}"));
                } else if line.contains("first") {
                    assert!(branch.trim().is_empty());
                }
            }
        }
    }
}

/// Long names use spare space while branch, tags and model stay aligned.
/// A narrow pane still truncates names to leave room for those columns.
#[test]
fn long_names_use_spare_space_and_columns_stay_aligned() {
    let bug = vec!["bug".to_owned()];
    let mut long = placed(
        "a prompt far too long to fit in the first column of the list",
        0,
        None,
    );
    let mut short = placed("short", 0, None);
    for row in [&mut long, &mut short] {
        if let InteractionRow::Interaction { tags, .. } = row {
            *tags = &bug;
        }
    }
    let view = InteractionNavigator {
        rows: vec![long, short],
        ..many_rows(0)
    };
    let name = "a prompt far too long to fit in the first column of the list";
    for width in [80u16, 120, 200] {
        let screen = rendered_at(&view, width)
            .chars()
            .collect::<Vec<_>>()
            .chunks(width.into())
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>();
        if width >= 120 {
            assert!(screen[1].contains(name), "{screen:#?}");
        } else {
            assert!(!screen[1].contains(name), "{screen:#?}");
            assert!(screen[1].contains('…'), "{screen:#?}");
        }
        for column in ["  feature", "  #bug", "  sonnet"] {
            let at = |line: &str| {
                let byte = line.find(column).expect("column is visible");
                line[..byte].chars().count()
            };
            assert_eq!(at(&screen[1]), at(&screen[2]), "{screen:#?}");
        }
    }
}

/// Tags keep their full width while long models are cut to fit, with
/// state and model columns aligned across rows.
#[test]
fn tags_stay_whole_and_models_fit_the_remaining_room() {
    let many = ["refactor", "performance", "urgent", "frontend", "backend"]
        .map(String::from)
        .to_vec();
    let mut wide = placed("wide", 0, None);
    let mut narrow = placed("narrow", 0, None);
    if let InteractionRow::Interaction {
        tags,
        model,
        stop_reason,
        ..
    } = &mut wide
    {
        *tags = &many;
        *model = "gpt-5.1-codex-max-preview-with-a-long-model-name";
        *stop_reason = Some("paused".into());
    }
    if let InteractionRow::Interaction { stop_reason, .. } = &mut narrow {
        *stop_reason = Some("paused".into());
    }
    let view = InteractionNavigator {
        rows: vec![wide, narrow],
        ..many_rows(0)
    };
    let full_tags = "  #refactor #performance #urgent #frontend #backend";
    for width in [80, 120] {
        let screen = rendered_at(&view, width);
        assert!(screen.contains(full_tags), "{screen}");
    }
    for width in [140u16, 160] {
        let screen = rendered_at(&view, width)
            .chars()
            .collect::<Vec<_>>()
            .chunks(width.into())
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>();
        let at = |line: &str, needle: &str| {
            line.find(needle)
                .map(|byte| line[..byte].chars().count())
                .unwrap_or_else(|| panic!("no {needle:?} in {screen:#?}"))
        };
        assert!(screen[1].contains(full_tags), "{screen:#?}");
        assert!(screen[1].contains('…'), "{screen:#?}");
        for (wide, narrow) in [("  gpt", "  sonnet"), ("  paused", "  paused")] {
            assert_eq!(at(&screen[1], wide), at(&screen[2], narrow), "{screen:#?}");
        }
    }
}

/// One row's uncommitted flag keeps room on every row, so the names
/// still start together; a prompt's line breaks read as spaces.
#[test]
fn names_line_up_beside_an_uncommitted_flag() {
    let mut flagged = placed("flagged", 0, None);
    if let InteractionRow::Interaction { uncommitted, .. } = &mut flagged {
        *uncommitted = true;
    }
    let view = InteractionNavigator {
        rows: vec![flagged, placed("two\nlines", 0, None)],
        ..many_rows(0)
    };
    let screen = lines(&view);
    assert!(screen[1].starts_with("┃ ●  ! flagged "), "{screen:#?}");
    assert!(screen[2].starts_with("┃ ●    two lines "), "{screen:#?}");
}

/// Rows explain whether they stopped, failed a turn, or are waiting on
/// a plan window, so the operator can decide what to do next.
#[test]
fn interaction_state_labels_explain_what_is_holding_up_work() {
    for (status, stop_reason, rate_limited, width, label) in [
        (
            InteractionStatus::Stopped(StopTone::Paused),
            Some("paused"),
            None,
            80,
            "paused",
        ),
        (InteractionStatus::IdleFailed, None, None, 80, "TURN FAILED"),
        (
            InteractionStatus::Idle,
            None,
            Some("five_hour"),
            120,
            "RATE LIMITED (five_hour)",
        ),
    ] {
        let mut row = placed("repair checkout", 0, None);
        if let InteractionRow::Interaction {
            branch,
            status: row_status,
            stop_reason: row_stop_reason,
            rate_limited: row_rate_limited,
            ..
        } = &mut row
        {
            *branch = None;
            *row_status = status;
            *row_stop_reason = stop_reason.map(Into::into);
            *row_rate_limited = rate_limited.map(Into::into);
        }
        let view = InteractionNavigator {
            rows: vec![row],
            ..many_rows(0)
        };
        let screen = squeezed(&rendered_at(&view, width));
        assert!(
            screen.contains(&format!("repair checkout {label} sonnet")),
            "{label}: {screen}"
        );
    }
    assert_eq!(status_marker(InteractionStatus::IdleFailed).1, theme::ERROR);
}

fn many_rows(selected: usize) -> InteractionNavigator<'static> {
    InteractionNavigator {
        scope: "Payments".into(),
        all_workspaces: false,
        completion_filter: "completed hidden".into(),
        time_filter: "time: disabled",
        tag_filter: &[],
        filter: None,
        typing_filter: false,
        requested_offset: Some(0),
        scrolled_away: false,
        rows: (0..8)
            .map(|index| InteractionRow::Interaction {
                name: format!("task {index}").into(),
                branched_from: None,
                branches: 0,
                branch_link: BranchLink::None,
                depth: 0,
                model: "sonnet",
                branch: None,
                expected_branch: None,
                worktree: None,
                same_as_source: false,
                shared_worktree: false,
                into: None,
                status: InteractionStatus::Idle,
                selected: index == selected,
                newly_idle: false,
                flashing: false,
                stop_reason: None,
                rate_limited: None,
                uncommitted: false,
                completion: CompletionState::Active,
                tags: &[],
                last_message: Some("done"),
            })
            .collect(),
    }
}

/// A row whose message line does not fit is drawn as far as it goes, so
/// the pane is filled to its foot, and the border says how much is left.
#[test]
fn a_list_longer_than_the_pane_fills_it_and_says_how_much_is_below() {
    let screen = rendered(&many_rows(0));
    let lines = screen
        .chars()
        .collect::<Vec<_>>()
        .chunks(80)
        .map(|line| line.iter().collect::<String>())
        .collect::<Vec<_>>();
    // Ten lines inside the border: five rows of two lines each.
    assert!(lines[9].contains("task 4"), "{screen}");
    assert!(lines[10].contains("done"), "{screen}");
    assert!(lines[11].contains("6 more lines"), "{screen}");
    assert!(!lines[0].contains("more line"), "{screen}");
}

#[test]
fn the_list_scrolls_to_keep_the_cursor_in_view() {
    let screen = rendered(&many_rows(7));
    let lines = screen
        .chars()
        .collect::<Vec<_>>()
        .chunks(80)
        .map(|line| line.iter().collect::<String>())
        .collect::<Vec<_>>();
    assert!(lines[9].contains("task 7"), "{screen}");
    assert!(lines[10].contains("done"), "{screen}");
    assert!(!screen.contains("task 2 "), "{screen}");
    assert!(lines[0].contains("6 more lines"), "{screen}");
    assert!(!lines[11].contains("more line"), "{screen}");
}

#[test]
fn a_fresh_list_starts_at_the_top_with_the_current_interaction_selected() {
    let mut view = many_rows(4);
    view.requested_offset = None;
    assert_eq!(scroll_offset(&view, Some(4), 14, 16), 0);
    // A saved offset still wins when the selection fits within its margins.
    view.requested_offset = Some(2);
    assert_eq!(scroll_offset(&view, Some(4), 14, 16), 2);
    // An older current interaction must remain visible on the first draw.
    view = many_rows(7);
    view.requested_offset = None;
    assert_eq!(scroll_offset(&view, Some(7), 10, 16), 6);
}

#[test]
fn an_unselected_list_keeps_its_place() {
    // An index outside the list leaves every interaction unselected.
    assert_eq!(drawn_offset(usize::MAX, 0), 0);
    assert_eq!(drawn_offset(usize::MAX, 2), 2);
    let screen = rendered(&many_rows(usize::MAX));
    assert!(screen.contains("task 0"), "{screen}");
    assert!(!screen.contains("task 7"), "{screen}");
}

#[test]
fn an_unselected_list_that_fits_starts_at_the_top() {
    let mut view = many_rows(usize::MAX);
    view.rows.truncate(2);
    assert_eq!(scroll_offset(&view, None, 10, 4), 0);
    view.rows.clear();
    assert_eq!(scroll_offset(&view, None, 10, 0), 0);
}

#[test]
fn a_view_scrolled_away_leaves_the_cursor_behind() {
    let mut view = many_rows(0);
    view.scrolled_away = true;
    view.requested_offset = Some(6);
    assert_eq!(scroll_offset(&view, Some(0), 10, 16), 6);
    // It still cannot scroll past the foot of the list.
    view.requested_offset = Some(100);
    assert_eq!(scroll_offset(&view, Some(0), 10, 16), 6);
}

/// The line [`render`] draws [`many_rows`] from, in a pane ten lines tall
/// inside its border: five two-line rows.
fn drawn_offset(selected: usize, requested_offset: usize) -> usize {
    let view = InteractionNavigator {
        requested_offset: Some(requested_offset),
        ..many_rows(selected)
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    let mut offset = None;
    terminal
        .draw(|frame| offset = Some(render(frame, &view, frame.area())))
        .unwrap();
    offset.unwrap()
}

#[test]
fn scrolling_keeps_two_rows_of_margin_until_the_list_ends() {
    for (rule, selected, requested, expected) in [
        ("free movement at top", 0, 0, 0),
        ("free movement at top", 1, 0, 0),
        ("free movement at top", 2, 0, 0),
        ("free movement between margins", 4, 4, 4),
        ("approaching lower edge", 3, 0, 2),
        ("inside bottom margin", 5, 6, 6),
        ("approaching upper edge", 4, 6, 4),
        ("stop at first row", 0, 6, 0),
        ("stop at last row", 7, 0, 6),
        ("clamp past end", 7, 100, 6),
    ] {
        assert_eq!(
            drawn_offset(selected, requested),
            expected,
            "{rule}: selected={selected}, requested={requested}"
        );
    }
}

/// Top-level names stay bright; nested names and supporting text fade.
#[test]
fn the_cursor_row_is_lifted_and_the_rest_faded() {
    let mut view = many_rows(0);
    if let InteractionRow::Interaction { depth, .. } = &mut view.rows[2] {
        *depth = 1;
    }
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &view, frame.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    // The first cell drawing `symbol` on line `y`: `t` starts each name,
    // `o` the model, `d` the message.
    let color = |symbol: &str, y: u16| {
        let x = (0..80)
            .find(|&x| buffer[(x, y)].symbol() == symbol)
            .expect("line draws the symbol");
        buffer[(x, y)].fg
    };
    let faded = |color| {
        faded_style(Style::default().fg(color), UNSELECTED_FADE)
            .fg
            .unwrap()
    };
    let brightened = |color| {
        blended_style(Style::default().fg(color), theme::TEXT, SELECTED_BRIGHTEN)
            .fg
            .unwrap()
    };
    let unselected = |color| {
        faded_style(Style::default().fg(color), UNSELECTED_ROW_FADE)
            .fg
            .unwrap()
    };
    // The cursor's row on lines 1 and 2, the next row on lines 3 and 4.
    assert_eq!(color("●", 1), theme::SUCCESS);
    assert_eq!(color("t", 1), theme::SELECTED_LIVE_INTERACTION_TEXT);
    assert_eq!(color("o", 1), brightened(theme::INTERACTION_STATUS_INFO));
    assert_eq!(color("d", 2), brightened(theme::SUBORDINATE_TEXT));
    assert_eq!(color("●", 3), unselected(theme::SUCCESS));
    assert_eq!(color("t", 3), theme::TOP_LEVEL_INTERACTION_NAME);
    assert_eq!(color("t", 5), unselected(theme::NESTED_INTERACTION_NAME));
    assert_eq!(
        color("o", 3),
        unselected(faded(theme::INTERACTION_STATUS_INFO))
    );
    assert_eq!(color("d", 4), unselected(faded(theme::SUBORDINATE_TEXT)));
}

/// The cursor's row, message line and all, is marked by a bar over the
/// left border in the selection color; the dropdown has a thick frame.
#[test]
fn the_cursor_row_is_marked_on_the_left_border() {
    let lines = rendered(&many_rows(1))
        .chars()
        .collect::<Vec<_>>()
        .chunks(80)
        .map(|line| line.iter().collect::<String>())
        .collect::<Vec<_>>();
    assert!(
        lines[3].starts_with("┃") && lines[3].contains("task 1"),
        "{lines:#?}"
    );
    assert!(
        lines[4].starts_with("┃") && lines[4].contains("done"),
        "{lines:#?}"
    );
    for line in [1, 2, 5, 6] {
        assert!(lines[line].starts_with("┃"), "{lines:#?}");
    }
}

/// Each way of being finished with a Session has its own badge, so one
/// given up on never reads as one that got done.
#[test]
fn each_completion_state_has_its_own_badge() {
    for (completion, badge) in [
        (CompletionState::Completed, "  COMPLETED"),
        (CompletionState::Abandoned, "  ABANDONED"),
        (CompletionState::Sealed, "  SEALED"),
    ] {
        let mut view = many_rows(0);
        view.rows.truncate(1);
        if let InteractionRow::Interaction {
            completion: row, ..
        } = &mut view.rows[0]
        {
            *row = completion;
        }
        let screen = rendered(&view);
        assert!(screen.contains(badge), "{completion:?}: {screen}");
        for (other, other_badge) in [
            (CompletionState::Completed, "  COMPLETED"),
            (CompletionState::Abandoned, "  ABANDONED"),
            (CompletionState::Sealed, "  SEALED"),
        ] {
            if other != completion {
                assert!(!screen.contains(other_badge), "{completion:?}: {screen}");
            }
        }
    }
    let mut view = many_rows(0);
    view.rows.truncate(1);
    let screen = rendered(&view);
    assert!(!screen.contains("COMPLETED"), "{screen}");
    assert!(!screen.contains("ABANDONED"), "{screen}");
    assert!(!screen.contains("SEALED"), "{screen}");
}

/// A Session sealed by integration says so, and is marked as done rather
/// than as merely stopped.
#[test]
fn a_session_sealed_by_integration_reads_as_integrated() {
    let mut view = many_rows(0);
    view.rows.truncate(1);
    if let InteractionRow::Interaction {
        completion, status, ..
    } = &mut view.rows[0]
    {
        *completion = CompletionState::Sealed;
        *status = InteractionStatus::Stopped(StopTone::Integrated);
    }
    let screen = rendered(&view);
    assert!(screen.contains("✓"), "{screen}");
    assert!(screen.contains("  INTEGRATED"), "{screen}");
    assert!(!screen.contains("SEALED"), "{screen}");
}
