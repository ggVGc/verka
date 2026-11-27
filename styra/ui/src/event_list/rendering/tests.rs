use super::*;
use super::{format_tokens, list_offset_with_scrolloff, wrap_log_lines};
use crate::chrome::StatusTone;
use crate::markdown::markdown_block_lines;
use ratatui::backend::TestBackend;
use ratatui::style::Style;
use ratatui::Terminal;

/// An identity no other entry in this process has.
///
/// The row cache is keyed on the version and lives in a thread-local, and
/// the test harness is free to run several tests on one thread. Two tests
/// that both wrote a literal id would then be asking the same cache the
/// same question about different events, and the second would be answered
/// with the first one's rows. Minting these the way a host does keeps each
/// test's entry its own.
fn version() -> EntryVersion {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    EntryVersion {
        id: NEXT.fetch_add(1, Ordering::Relaxed),
        revision: 0,
    }
}

fn list_view<'a>(entries: Vec<EventEntry<'a>>) -> EventListView<'a> {
    EventListView {
        chrome: PanelChrome {
            focused: true,
            workspace: None,
            worktree: None,
            into: None,
            source_branch: None,
            parent: None,
            agent: "codex".into(),
            model: "gpt-5.6-sol".into(),
            model_reported: true,
            effort: None,
            effort_reported: true,
            status: "running".into(),
            status_tone: StatusTone::Running,
            elapsed: None,
            suffix: None,
            session: None,
        },
        entries,
        activity: ActivityCounts::default(),
        all_events: false,
        show_minor: false,
        uncommitted_changes: false,
        read_only: false,
        usage: None,
        can_configure_launch: false,
        selection_name: "codex".into(),
        requested_offset: 0,
        requested_row_offset: 0,
        scroll_delta: 0,
        rendered_selection_row: None,
        anchor: ListAnchor::Selection,
        moved_backward: false,
        protocol: Protocol::default(),
        links: LinkDisplay::Compact,
        search: SearchView {
            query: None,
            typing: false,
        },
        status: EventListStatus::Idle {
            reason: None,
            failed: false,
        },
        queued: &[],
    }
}

fn buffer_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    (buffer.area.top()..buffer.area.bottom())
        .map(|y| {
            (buffer.area.left()..buffer.area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

/// Draw a list of `events` with `selected` under the cursor, anchored at
/// `requested_offset`, and report the screen and the offset the render
/// asked to keep.
fn scrolled_screen(
    events: &[AgentEvent],
    selected: usize,
    requested_offset: usize,
    anchor_selection: bool,
    scroll_delta: i32,
) -> (Vec<String>, usize) {
    let entries = events
        .iter()
        .enumerate()
        .map(|(index, event)| EventEntry {
            expanded: false,
            has_detail: false,
            selected: index == selected,
            ..entry_of(event, version())
        })
        .collect();
    let view = EventListView {
        requested_offset,
        scroll_delta,
        anchor: if anchor_selection {
            ListAnchor::Selection
        } else {
            ListAnchor::Free
        },
        ..list_view(entries)
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    let mut offset = 0;
    terminal
        .draw(|frame| {
            offset = render(frame, &view, frame.area()).effective_offset;
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = buffer_rows(&buffer);
    (rows, offset)
}

fn numbered(count: usize) -> Vec<AgentEvent> {
    (0..count)
        .map(|n| AgentEvent::AgentMessage {
            text: format!("message {n}"),
        })
        .collect()
}

/// Window offsets remain in the caller's numbering. Anchored windows
/// follow selection; unanchored windows follow explicit scroll requests.
#[test]
fn scrolled_windows_preserve_offsets_and_visible_entries() {
    for (case, (count, selected, offset, anchored, delta), (expected_offset, visible, hidden)) in [
        (
            "rebased window",
            (500, 300, 300, true, 0),
            (Some(300), &[300][..], &[0][..]),
        ),
        (
            "whole short list",
            (4, 0, 0, true, 0),
            (Some(0), &[0, 1, 2, 3][..], &[][..]),
        ),
        // Consecutive anchors must share the boundary entry: no skipped row.
        (
            "first anchor",
            (200, 100, 100, true, 0),
            (None, &[101][..], &[][..]),
        ),
        (
            "next anchor",
            (200, 101, 101, true, 0),
            (None, &[101][..], &[][..]),
        ),
        (
            "unanchored",
            (200, 0, 100, false, 0),
            (Some(100), &[100][..], &[0][..]),
        ),
        (
            "ten-line scroll",
            (30, 0, 0, false, 10),
            (Some(10), &[10][..], &[9][..]),
        ),
    ] {
        let events = numbered(count);
        let (rows, actual_offset) = scrolled_screen(&events, selected, offset, anchored, delta);
        if let Some(expected) = expected_offset {
            assert_eq!(actual_offset, expected, "{case}: caller offset");
        }
        for n in visible {
            assert!(
                rows.iter().any(|row| row.contains(&format!("message {n}"))),
                "{case}: missing message {n}: {rows:#?}"
            );
        }
        for n in hidden {
            assert!(
                !rows.iter().any(|row| row.contains(&format!("message {n}"))),
                "{case}: unexpected message {n}: {rows:#?}"
            );
        }
    }
}

/// One agent message, drawn with `search` typed into the `/` prompt.
fn searched_screen(text: &str, search: SearchView<'_>) -> (Vec<String>, Vec<String>) {
    let event = AgentEvent::AgentMessage { text: text.into() };
    let view = EventListView {
        search,
        ..list_view(vec![EventEntry {
            expanded: false,
            has_detail: false,
            selected: true,
            ..entry_of(&event, version())
        }])
    };
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &view, frame.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = buffer_rows(&buffer);
    // The marked text, row by row, so a test can say what was marked
    // without depending on where on the row it landed.
    let marked = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .filter(|x| buffer.cell((*x, y)).unwrap().bg == theme::ACCENT)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect::<String>()
        })
        .collect();
    (rows, marked)
}

#[test]
fn a_search_marks_the_matching_words_of_the_rows_on_screen() {
    let (_, marked) = searched_screen(
        "reworked the retry in src/retry.rs",
        SearchView {
            query: Some("retry"),
            typing: false,
        },
    );

    assert_eq!(marked.concat(), "retrysrc/retry.rs");
}

#[test]
fn a_search_marks_nothing_until_three_characters_are_typed() {
    let (rows, marked) = searched_screen(
        "reworked the retry",
        SearchView {
            query: Some("re"),
            typing: true,
        },
    );

    assert_eq!(marked.concat(), "");
    // And the prompt says why nothing is marked yet.
    assert!(
        rows.concat().contains("/re▌ · 1 more character"),
        "{rows:?}"
    );
}

/// The selected entry's text is drawn in the live list's cursor color,
/// and its tag keeps its own so what the entry is still reads.
#[test]
fn the_selected_entry_text_is_soft_cyan() {
    let event = AgentEvent::AgentMessage {
        text: "the checks are green".into(),
    };
    let fg = |selected: bool, content: &str| {
        let entry = EventEntry {
            expanded: false,
            has_detail: false,

            selected,
            ..entry_of(&event, version())
        };
        summary_line(&entry, SummaryContext::EventList, Protocol::default())
            .spans
            .into_iter()
            .find(|span| span.content.contains(content))
            .and_then(|span| span.style.fg)
    };
    assert_eq!(
        fg(true, "the checks are green"),
        Some(theme::SELECTED_INTERACTION_TEXT)
    );
    assert_ne!(
        fg(false, "the checks are green"),
        Some(theme::SELECTED_INTERACTION_TEXT)
    );
    assert_ne!(fg(true, "«"), Some(theme::SELECTED_INTERACTION_TEXT));
}

/// A heavy red bar covers the left border beside the selected entry's
/// rows, and only there.
#[test]
fn a_red_bar_marks_the_selected_entry_on_the_left_border() {
    let events = [
        AgentEvent::AgentMessage {
            text: "first".into(),
        },
        AgentEvent::AgentMessage {
            text: "second, long enough to wrap onto a row of its own ".repeat(2),
        },
        AgentEvent::AgentMessage {
            text: "third".into(),
        },
    ];
    let view = list_view(
        events
            .iter()
            .enumerate()
            .map(|(index, event)| EventEntry {
                expanded: index == 1,
                has_detail: false,
                selected: index == 1,
                ..entry_of(event, version())
            })
            .collect(),
    );
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &view, frame.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = buffer_rows(&buffer);
    let marked = (0..buffer.area.height)
        .filter(|y| buffer.cell((0, *y)).unwrap().fg == theme::SELECTED_ROW_EDGE)
        .collect::<Vec<_>>();
    assert!(marked.len() > 1, "{marked:?} {rows:#?}");
    for y in &marked {
        assert_eq!(buffer.cell((0, *y)).unwrap().symbol(), "┃");
    }
    let text = |y: u16| rows[usize::from(y)].clone();
    assert!(text(marked[0]).contains("second"), "{rows:#?}");
    assert!(
        !marked.iter().any(|y| text(*y).contains("first")),
        "{rows:#?}"
    );
    assert!(
        !marked.iter().any(|y| text(*y).contains("third")),
        "{rows:#?}"
    );
}

/// A branch's `branched from` marker heads the pane however far down the
/// list is read, and offsets stay in the caller's numbering around it.
#[test]
fn the_branched_from_marker_stays_locked_at_the_top() {
    use styra_protocol::event::BranchDirection;
    let mut events = vec![AgentEvent::Branched {
        direction: BranchDirection::From,
        session: "parent".into(),
        name: None,
    }];
    events.extend(numbered(20));
    let divider = "─".repeat(78);
    let (rows, offset) = scrolled_screen(&events, 20, 0, true, 0);
    assert!(offset > 1, "the list scrolled past the marker: {offset}");
    assert!(rows[1].contains(&divider), "{rows:#?}");
    assert!(rows[2].contains("from: untitled"), "{rows:#?}");
    assert!(rows[3].contains(&divider), "{rows:#?}");
    assert!(
        rows[4].contains(&format!("message {}", offset - 1)),
        "{rows:#?}"
    );
    assert!(rows.iter().any(|row| row.contains("message 19")));

    // From the top, the marker is where it always was, with the branch's
    // own first entry right beneath it: the scrolling part starts there.
    let (rows, offset) = scrolled_screen(&events, 0, 0, true, 0);
    assert_eq!(offset, 1);
    assert!(rows[2].contains("from: untitled"), "{rows:#?}");
    assert!(rows[4].contains("message 0"), "{rows:#?}");
}

/// The top border says when the list is scrolled away from its top, and
/// says nothing once it is back there.
#[test]
fn the_top_border_marks_a_list_scrolled_away_from_its_top() {
    let events = numbered(20);
    let (rows, offset) = scrolled_screen(&events, 0, 0, true, 0);
    assert_eq!(offset, 0);
    assert!(!rows[0].contains("above"), "{rows:#?}");

    let (rows, offset) = scrolled_screen(&events, 19, 0, true, 0);
    assert!(offset > 1, "the list scrolled: {offset}");
    assert!(
        rows[0].contains(&format!("▲ {offset} entries above")),
        "{rows:#?}"
    );

    // Under a pinned marker, the top is the branch's own first entry.
    use styra_protocol::event::BranchDirection;
    let mut events = vec![AgentEvent::Branched {
        direction: BranchDirection::From,
        session: "parent".into(),
        name: None,
    }];
    events.extend(numbered(20));
    let (rows, _) = scrolled_screen(&events, 0, 0, true, 0);
    assert!(!rows[0].contains("above"), "{rows:#?}");
}

#[test]
fn consecutive_branch_markers_share_one_thin_divider() {
    use styra_protocol::event::BranchDirection;
    let marker = AgentEvent::Branched {
        direction: BranchDirection::From,
        session: "parent".into(),
        name: None,
    };
    let entries = vec![entry_of(&marker, version()), entry_of(&marker, version())];
    let first = entry_at(&entries, 0);
    let second = entry_at(&entries, 1);
    let normal = entry_rows_with_max_rows(&entries[1], 40, 20, render_of());
    let shared = entry_rows_with_max_rows(&second, 40, 20, render_of());
    assert_eq!(normal.len(), shared.len() + 1);
    assert_eq!(normal[0].to_string(), "─".repeat(40));
    assert_eq!(shared[0].to_string(), normal[1].to_string());
    let first_rows = entry_rows_with_max_rows(&first, 40, 20, render_of());
    assert_eq!(first_rows.last().unwrap().to_string(), "─".repeat(40));
    assert_eq!(shared.last().unwrap().to_string(), "─".repeat(40));
}

#[test]
fn a_branch_marker_reads_by_the_name_of_where_it_leads() {
    use styra_protocol::event::BranchDirection;
    let marker = |direction| AgentEvent::Branched {
        direction,
        session: "4f1c-session-id".into(),
        name: Some("stale snapshot".into()),
    };
    let summary = |event: &AgentEvent, name| {
        let entry = EventEntry {
            expanded: false,
            branch: BranchLink {
                name,
                ..Default::default()
            },
            ..entry_of(event, version())
        };
        summary_line(&entry, SummaryContext::EventList, Protocol::default())
            .spans
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>()
    };
    let to = summary(&marker(BranchDirection::To), Some("Retry backoff"));
    assert!(to.contains("▌ Retry backoff"), "{to}");
    assert!(!to.contains("4f1c-session-id"), "{to}");
    let from = summary(&marker(BranchDirection::From), Some("Payments"));
    assert!(from.contains("▌ from: Payments"), "{from}");
    let unnamed = summary(&marker(BranchDirection::To), None);
    assert!(unnamed.contains("▌ untitled"), "{unnamed}");
    assert!(!unnamed.contains("stale snapshot"), "{unnamed}");
}

/// The branches off where a marker leads sit inside its band, each a
/// marker of its own, indented a step further per generation.
#[test]
fn a_branch_marker_shows_its_child_branches_indented_beneath_it() {
    use styra_protocol::event::BranchDirection;
    let event = AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "4f1c-session-id".into(),
        name: None,
    };
    let named = |name, children| BranchLink {
        name: Some(name),
        children,
        ..Default::default()
    };
    let entry = EventEntry {
        expanded: false,
        has_detail: false,
        branch: named(
            "Retry backoff",
            vec![
                named("Jitter", vec![named("Cap", Vec::new())]),
                named("Metrics", Vec::new()),
            ],
        ),
        ..entry_of(&event, version())
    };
    let rows = entry_rows_with_max_rows(&entry, 40, 20, render_of())
        .into_iter()
        .map(|row| row.to_string().trim_end().to_owned())
        .collect::<Vec<_>>();
    let divider = "─".repeat(40);
    assert_eq!(
        rows,
        [
            divider.as_str(),
            "▌ Retry backoff",
            "  ▌ Jitter",
            "    ▌ Cap",
            "  ▌ Metrics",
            divider.as_str(),
        ]
    );
    // Short of room, the marker's own row is kept over its branches.
    let cut = entry_rows_with_max_rows(&entry, 40, 4, render_of());
    assert_eq!(cut.len(), 4);
    assert_eq!(cut[1].to_string().trim_end(), "▌ Retry backoff");

    // With the cursor on a branch, that row is drawn selected instead of
    // the marker's own.
    let cursor = Some(theme::SELECTION_BACKGROUND);
    let selected = EventEntry {
        selected: true,
        selected_branch: Some(1),
        ..entry.clone()
    };
    let rows = entry_rows_with_max_rows(&selected, 40, 20, render_of());
    let marked = rows
        .iter()
        .map(|row| row.style.bg == cursor)
        .collect::<Vec<_>>();
    assert_eq!(marked, [false, false, false, true, false, false]);
    assert_eq!(rows[3].to_string().trim_end(), "    • Cap");
    let on_marker = EventEntry {
        selected_branch: None,
        ..selected
    };
    let rows = entry_rows_with_max_rows(&on_marker, 40, 20, render_of());
    assert_eq!(rows[1].style.bg, cursor);
    assert!(rows[2..].iter().all(|row| row.style.bg != cursor));
}

/// A branch beneath a marker quotes its last message under its row, as
/// the marker does its own, at that branch's indent.
#[test]
fn a_child_branch_shows_its_last_message_beneath_it() {
    use styra_protocol::event::BranchDirection;
    let event = AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "4f1c-session-id".into(),
        name: None,
    };
    let link = |name, message, children| BranchLink {
        name: Some(name),
        message,
        children,
        ..Default::default()
    };
    let entry = EventEntry {
        has_detail: false,
        branch: link(
            "Retry backoff",
            Some("Doubled it"),
            vec![
                link(
                    "Jitter",
                    Some("Added jitter"),
                    vec![link("Cap", Some("Capped at 30s"), Vec::new())],
                ),
                link("Metrics", None, Vec::new()),
            ],
        ),
        ..entry_of(&event, version())
    };
    let rows = entry_rows_with_max_rows(&entry, 40, 20, render_of())
        .into_iter()
        .map(|row| row.to_string().trim_end().to_owned())
        .filter(|row| !row.starts_with('─'))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            "▌ Retry backoff",
            "    « Doubled it",
            "  ▌ Jitter",
            "      « Added jitter",
            "    ▌ Cap",
            "        « Capped at 30s",
            "  ▌ Metrics",
        ]
    );

    // The cursor still counts branches, not rows: the second is `Cap`.
    let selected = EventEntry {
        selected: true,
        selected_branch: Some(1),
        ..entry
    };
    let rows = entry_rows_with_max_rows(&selected, 40, 20, render_of());
    let cursor = Some(theme::SELECTION_BACKGROUND);
    let marked = rows
        .iter()
        .filter(|row| row.style.bg == cursor)
        .map(|row| row.to_string().trim_end().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(marked, ["    • Cap"]);
}

/// A marker and the branches beneath it carry their `#tags` after the
/// name, as the navigator's rows do.
#[test]
fn a_branch_marker_shows_the_tags_of_where_it_leads() {
    use styra_protocol::event::BranchDirection;
    let event = AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "4f1c-session-id".into(),
        name: None,
    };
    let tags = ["retries".to_owned(), "infra".to_owned()];
    let child_tags = ["cap".to_owned()];
    let entry = EventEntry {
        has_detail: false,
        branch: BranchLink {
            name: Some("Retry backoff"),
            tags: &tags,
            children: vec![
                BranchLink {
                    name: Some("Cap"),
                    tags: &child_tags,
                    ..Default::default()
                },
                BranchLink {
                    name: Some("Untagged"),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        ..entry_of(&event, version())
    };
    let rows = entry_rows_with_max_rows(&entry, 40, 20, render_of());
    let text = rows
        .iter()
        .map(|row| row.to_string().trim_end().to_owned())
        .filter(|row| !row.starts_with('─'))
        .collect::<Vec<_>>();
    assert_eq!(
        text,
        [
            "▌ Retry backoff  #retries #infra",
            "  ▌ Cap  #cap",
            "  ▌ Untagged",
        ]
    );
    let tag = rows[1]
        .spans
        .iter()
        .find(|span| span.content.contains('#'))
        .unwrap();
    assert_eq!(tag.style.fg, Some(theme::INTERACTION_TAG));
}

#[test]
fn a_branch_marker_names_a_worktree_only_when_it_is_another_one() {
    use styra_protocol::event::BranchDirection;
    let summary = |direction, worktree| {
        let event = AgentEvent::Branched {
            direction,
            session: "4f1c-session-id".into(),
            name: None,
        };
        let entry = EventEntry {
            expanded: false,
            branch: BranchLink {
                name: Some("Retry backoff"),
                worktree,
                ..Default::default()
            },
            ..entry_of(&event, version())
        };
        summary_line(&entry, SummaryContext::EventList, Protocol::default())
            .spans
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>()
    };
    let elsewhere = summary(BranchDirection::To, Some("fix-retries"));
    assert!(
        elsewhere.contains("Retry backoff  ⎇ fix-retries"),
        "{elsewhere}"
    );
    let from = summary(BranchDirection::From, Some("main checkout"));
    assert!(
        from.contains("from: Retry backoff  ⎇ main checkout"),
        "{from}"
    );
    let shared = summary(BranchDirection::To, None);
    assert!(!shared.contains('⎇'), "{shared}");
}

#[test]
fn an_expanded_branch_marker_reads_as_the_navigator_row_of_where_it_leads() {
    use crate::chrome::StopTone;
    use crate::interactions::{InteractionState, InteractionStatus};
    use styra_protocol::event::BranchDirection;
    use styra_protocol::CompletionState;
    let event = AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "4f1c-session-id".into(),
        name: Some("stale snapshot".into()),
    };
    let paused = InteractionState {
        status: InteractionStatus::Stopped(StopTone::Paused),
        stop_reason: Some("paused".into()),
        rate_limited: None,
        newly_idle: false,
        completion: CompletionState::Active,
    };
    let long = format!("Backoff now doubles up to 30s. {}", "More. ".repeat(40));
    let mut entry = EventEntry {
        branch: BranchLink {
            name: Some("Retry backoff"),
            state: Some(paused),
            message: Some(&long),
            worktree: None,
            tags: &[],
            children: Vec::new(),
        },
        ..entry_of(&event, version())
    };
    let render = render_of();
    let rows = |entry: &EventEntry<'_>| {
        entry_rows_with_max_rows(entry, 80, 20, render)
            .into_iter()
            .filter(|line| line.to_string() != "─".repeat(80))
            .map(|line| line.to_string())
            .collect::<Vec<String>>()
    };
    let shown = rows(&entry);
    // The state on the summary row, the message alone on one row under it,
    // and the name said once.
    assert_eq!(shown.len(), 2, "{shown:#?}");
    assert!(
        shown[0].contains("▌  ■  Retry backoff  paused"),
        "{shown:#?}"
    );
    assert!(shown[1].contains("« Backoff now doubles up to 30s."));
    assert!(shown[1].ends_with('…'), "{shown:#?}");
    let all = shown.join("\n");
    assert!(!all.contains("name:"), "{all}");
    assert!(!all.contains("stale snapshot"), "{all}");
    assert!(!all.contains("4f1c-session-id"), "{all}");

    let from = AgentEvent::Branched {
        direction: BranchDirection::From,
        session: "parent".into(),
        name: None,
    };
    let from_entry = EventEntry {
        event: &from,
        ..entry.clone()
    };
    let from_rows =
        vec![summary_line(&from_entry, SummaryContext::EventList, Protocol::default()).to_string()];
    assert!(
        from_rows[0].contains("▌ from: Retry backoff"),
        "{from_rows:#?}"
    );
    assert!(!from_rows[0].contains('■'), "{from_rows:#?}");

    // A change of state on the other side is a different row, though the
    // marker itself has not changed.
    entry.branch.state = Some(InteractionState {
        status: InteractionStatus::Running { events: 3 },
        stop_reason: None,
        ..entry.branch.state.clone().unwrap()
    });
    let running = rows(&entry);
    assert!(!running[0].contains("paused"), "{running:#?}");
}

#[test]
fn a_branch_marker_has_horizontal_dividers_without_a_background() {
    use styra_protocol::event::BranchDirection;
    let marker = |direction| AgentEvent::Branched {
        direction,
        session: "4f1c-session-id".into(),
        name: None,
    };
    let render = render_of();
    let backgrounds = |event: &AgentEvent, message| {
        let entry = EventEntry {
            branch: BranchLink {
                message,
                ..Default::default()
            },
            ..entry_of(event, version())
        };
        let rows = entry_rows_with_max_rows(&entry, 80, 20, render);
        if let Some(color) = branch_border(event) {
            assert_eq!(rows.first().unwrap().to_string(), "─".repeat(80));
            assert_eq!(rows.last().unwrap().to_string(), "─".repeat(80));
            assert_eq!(rows[0].spans[0].style.fg, Some(color));
            for row in &rows[1..rows.len() - 1] {
                assert!(!row.to_string().contains('│'));
                assert!(row.width() <= 80);
            }
        }
        rows.into_iter().map(|row| row.style.bg).collect::<Vec<_>>()
    };
    // The border and content keep the terminal background.
    assert_eq!(
        backgrounds(&marker(BranchDirection::To), Some("Done.")),
        [None; 4]
    );
    assert_eq!(backgrounds(&marker(BranchDirection::From), None), [None; 3]);
    let model = AgentEvent::ModelChanged {
        model: Some("opus".into()),
        effort: None,
    };
    assert_eq!(
        backgrounds(&model, None)[0],
        Some(theme::CONVERSATION_CHANGE_BACKGROUND)
    );
}

#[test]
fn finished_branch_markers_darken_and_refresh_when_their_state_changes() {
    use crate::interactions::{InteractionState, InteractionStatus};
    use styra_protocol::{event::BranchDirection, CompletionState};

    for direction in [BranchDirection::To, BranchDirection::From] {
        let event = AgentEvent::Branched {
            direction,
            session: "finished-branch".into(),
            name: None,
        };
        let mut entry = EventEntry {
            branch: BranchLink {
                name: Some("Branch name"),
                state: Some(InteractionState {
                    status: InteractionStatus::Idle,
                    stop_reason: None,
                    rate_limited: None,
                    newly_idle: false,
                    completion: CompletionState::Active,
                }),
                message: Some("Latest message"),
                worktree: None,
                tags: &[],
                children: Vec::new(),
            },
            ..entry_of(&event, version())
        };
        let active = entry_rows_with_max_rows(&entry, 80, 20, render_of());
        for (completion, status) in [
            (CompletionState::Completed, InteractionStatus::Idle),
            (CompletionState::Abandoned, InteractionStatus::Idle),
            (CompletionState::Sealed, InteractionStatus::Idle),
            (
                CompletionState::Active,
                InteractionStatus::Stopped(StopTone::Integrated),
            ),
        ] {
            let state = entry.branch.state.as_mut().unwrap();
            state.completion = completion;
            state.status = status;
            let finished = entry_rows_with_max_rows(&entry, 80, 20, render_of());
            assert_eq!(finished[0].style.bg, active[0].style.bg);
            // Both dividers, the marker, and the latest message all dim.
            for (finished, active) in finished.iter().zip(&active) {
                assert_eq!(
                    finished.spans[0].style,
                    faded_style(active.spans[0].style, 3)
                );
            }
            entry.selected = true;
            let selected = entry_rows_with_max_rows(&entry, 80, 20, render_of());
            assert_eq!(selected[1].style.bg, Some(theme::SELECTION_BACKGROUND));
            assert_eq!(selected[0].spans[0].style, active[0].spans[0].style);
            entry.selected = false;
        }
        let state = entry.branch.state.as_mut().unwrap();
        state.completion = CompletionState::Active;
        state.status = InteractionStatus::Idle;
        assert_eq!(
            entry_rows_with_max_rows(&entry, 80, 20, render_of()),
            active
        );
    }
}

#[test]
fn provider_switch_is_shown_in_branch_summary_and_invalidates_cached_rows() {
    use styra_protocol::agent::Provider;
    use styra_protocol::event::BranchDirection;
    let event = AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "destination".into(),
        name: None,
    };
    let mut entry = EventEntry {
        expanded: false,
        ..entry_of(&event, version())
    };
    let render = render_of();
    let text = |rows: Vec<Line<'static>>| {
        rows.into_iter()
            .flat_map(|line| line.spans)
            .map(|span| span.content.into_owned())
            .collect::<String>()
    };
    let plain = text(entry_rows_with_max_rows(&entry, 120, 5, render));
    assert!(!plain.contains("provider switch"));
    entry.branch_provider_switch = Some((Provider::Claude, Provider::Codex));
    let switched = text(entry_rows_with_max_rows(&entry, 120, 5, render));
    assert!(
        switched.contains("provider switch: claude → codex)"),
        "{switched}"
    );
    assert!(!switched.contains("sealed"), "{switched}");
    let destination_marker = AgentEvent::Branched {
        direction: BranchDirection::From,
        session: "source".into(),
        name: None,
    };
    entry.event = &destination_marker;
    let destination_summary = text(vec![summary_line(
        &entry,
        SummaryContext::EventList,
        Protocol::default(),
    )]);
    assert!(destination_summary.contains("provider switch: claude → codex"));
    assert!(!destination_summary.contains("sealed"));
}

#[test]
fn a_highlighted_link_does_not_repeat_the_selected_summary() {
    let event = AgentEvent::AgentMessage {
        text: "see [guide](https://example.com/guide)".into(),
    };
    let entry = EventEntry {
        selected: true,
        link_highlight: Some(0),
        ..entry_of(&event, version())
    };
    let render = render_of();
    let mut terminal = Terminal::new(TestBackend::new(80, 4)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(
                List::new(vec![entry_item(&entry, 80, 4, render)]),
                frame.area(),
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let text = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol()))
        .collect::<String>();
    assert_eq!(text.matches("see guide").count(), 1);
    assert!(
        (0..buffer.area.width).any(|x| {
            let cell = buffer.cell((x, 0)).unwrap();
            cell.symbol() == "g" && cell.bg == theme::LINK_HIGHLIGHT_BACKGROUND
        }),
        "the selected link remains visible on the summary"
    );
}

#[test]
fn token_counts_read_as_k_and_m_past_a_thousand() {
    assert_eq!(format_tokens(999), "999");
    assert_eq!(format_tokens(1_000), "1.0k");
    assert_eq!(format_tokens(9_450), "9.4k");
    assert_eq!(format_tokens(126_400), "126k");
    assert_eq!(format_tokens(1_350_000), "1.3M");
    assert_eq!(format_tokens(12_000_000), "12M");
}

#[test]
fn unicode_summary_keeps_its_fold_marker_inside_the_pane() {
    let line = Line::from(vec![Span::raw("界界e\u{301}👩‍💻xyz"), Span::raw(" [+]")]);
    let clipped = truncate_line(line.clone(), 10, true);
    assert_eq!(clipped.to_string(), "界界e\u{301}… [+]");
    for width in 0..12 {
        assert!(truncate_line(line.clone(), width, true).width() <= width);
    }
}

#[test]
fn unicode_table_cells_and_prefix_stay_aligned_after_resizing() {
    let markdown = "| 名前 | Description |\n|---|---|\n| 界界界界 | e\u{301}👩‍💻説明説明説明説明 |";
    // The prefix is one character but two terminal columns.
    let lines = markdown_block_lines(markdown, Style::default(), "　");
    let wrapped = wrap_log_lines(lines, 22, 0);
    assert!(wrapped.iter().all(|line| line.width() == 22));
    let body: Vec<_> = wrapped
        .iter()
        .filter(|line| line.to_string().trim_start().starts_with('│'))
        .collect();
    assert!(!body.is_empty());
    let border_columns = |line: &Line<'_>| {
        let text = line.to_string();
        text.match_indices('│')
            .map(|(byte, _)| text::width(&text[..byte]))
            .collect::<Vec<_>>()
    };
    let expected = border_columns(body[0]);
    assert_eq!(expected.len(), 3);
    assert!(body.iter().all(|line| border_columns(line) == expected));
    let rendered: String = wrapped.iter().map(ToString::to_string).collect();
    assert!(rendered.contains("e\u{301}"));
    assert!(rendered.contains("👩‍💻"));
}

#[test]
fn wide_markdown_tables_wrap_the_widest_column_inside_the_borders() {
    let markdown = "| State | Explanation |\n\
                        |---|---|\n\
                        | stable | This deliberately long explanation wraps inside its cell |";
    let lines = markdown_block_lines(markdown, Style::default(), "    ");
    let wrapped = wrap_log_lines(lines, 36, 0);
    let rendered: Vec<String> = wrapped
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert!(wrapped.iter().all(|line| line.width() == 36));
    assert_eq!(
        rendered.first().and_then(|line| line.chars().nth(4)),
        Some('┌')
    );
    assert_eq!(
        rendered.last().and_then(|line| line.chars().nth(4)),
        Some('└')
    );
    assert!(rendered.iter().all(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('│') || trimmed.matches('│').count() == 3
    }));

    let body: Vec<&String> = rendered
        .iter()
        .skip_while(|line| !line.trim_start().starts_with('├'))
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('└'))
        .collect();
    assert!(body.len() > 1, "the long cell should add table rows");
    assert_eq!(
        body.iter().filter(|line| line.contains("stable")).count(),
        1
    );
    assert!(body.iter().skip(1).any(|line| line.contains("inside")));
}

#[test]
fn a_wrapped_table_keeps_code_backgrounds_off_the_cell_padding() {
    let markdown = "| State | Explanation |\n\
                        |---|---|\n\
                        | `stable` | This long explanation has `some code` that wraps inside its cell |";
    let lines = markdown_block_lines(markdown, Style::default(), "    ");
    let wrapped = wrap_log_lines(lines, 36, 0);
    assert!(wrapped.len() > 6, "the long cell should add table rows");

    for line in &wrapped {
        for (index, span) in line.spans.iter().enumerate() {
            let beside_border = [index.wrapping_sub(1), index + 1].iter().any(|&neighbour| {
                line.spans
                    .get(neighbour)
                    .is_some_and(|span| span.content.as_ref() == "│")
            });
            if beside_border && span.content.trim().is_empty() {
                assert_eq!(span.style.bg, None, "padding in {line:?}");
            }
        }
    }
    let code_background = |text: &str| {
        wrapped
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.as_ref() == text)
            .and_then(|span| span.style.bg)
    };
    assert!(code_background("stable").is_some());
    assert!(code_background("code").is_some());
}

/// A live row can change height without the selection moving: streamed
/// thinking replaces the previous thinking body, and task/tool completion
/// replaces its in-flight row.  The viewport must remain anchored when a
/// replacement is shorter; revealing older entries makes the interaction
/// log appear to jump backwards even though the operator did nothing.
#[test]
fn shrinking_live_tail_entry_does_not_reveal_older_entries() {
    let viewport_height = 6;
    let selected = Some(5);

    // Five old one-line rows precede a five-line live row. With the status
    // tail after it, following the live row legitimately advances the
    // viewport to item 5.
    let tall_live_row = [1, 1, 1, 1, 1, 5, 1];
    let anchored =
        list_offset_with_scrolloff(0, selected, &mut &tall_live_row[..], viewport_height, false);
    assert_eq!(anchored, 5);

    // The same selected row is replaced by a one-line update. No
    // navigation occurred, so its item anchor should not change.
    let short_live_row = [1, 1, 1, 1, 1, 1, 1];
    let after_update = list_offset_with_scrolloff(
        anchored,
        selected,
        &mut &short_live_row[..],
        viewport_height,
        false,
    );
    assert_eq!(after_update, anchored);
}

#[test]
fn backward_navigation_still_restores_scrolloff_above_the_selection() {
    let heights = [1, 1, 1, 1, 1, 1, 1];
    let offset = list_offset_with_scrolloff(5, Some(4), &mut &heights[..], 6, true);

    assert_eq!(offset, 2, "two rows of scrolloff above the selection");
}

/// Moving from a long entry to a short one below the view scrolls only far
/// enough to show the short one whole: the long one keeps the rest of the
/// pane rather than the short one jumping to the top.
#[test]
fn kept_view_scrolls_down_only_as_far_as_the_selection_needs() {
    // A short entry, a long one, a short one, then the status tail.
    let heights = [1, 12, 2, 1];

    assert_eq!(
        kept_anchor(0, 0, 2, &mut &heights[..], 10),
        Some((1, 4)),
        "the long entry's last eight rows stay above the selection"
    );
    assert_eq!(kept_anchor(1, 0, 2, &mut &heights[..], 10), Some((1, 4)),);
    assert_eq!(
        kept_anchor(0, 0, 1, &mut &[1, 3, 2, 1][..], 10),
        Some((0, 0)),
        "a selection already in view does not scroll"
    );
    assert_eq!(
        kept_anchor(0, 0, 1, &mut &heights[..], 10),
        Some((1, 0)),
        "a selection too tall for the pane fills it from its first row"
    );
    assert_eq!(
        kept_anchor(0, 0, 1, &mut &[1, 1, 1, 12, 1][..], 10),
        Some((0, 0)),
    );
    assert_eq!(
        kept_anchor(0, 0, 3, &mut &[1, 12, 2, 12, 1][..], 10),
        Some((3, 0)),
        "an entry taller than the pane shows its first rows"
    );
    assert_eq!(
        kept_anchor(0, 0, 3, &mut &[1, 1, 1, 4, 1][..], 5),
        Some((2, 0)),
        "a selection cut off at the foot scrolls until it is shown whole"
    );
    assert_eq!(
        kept_anchor(0, 0, 2, &mut &[1, 1, 8, 1][..], 5),
        Some((2, 0)),
        "as much of a tall selection as fits"
    );
}

#[test]
fn viewport_scroll_is_measured_in_rendered_rows_across_entries() {
    // Four entries followed by the one-row status tail.
    let heights = [3, 8, 5, 1, 1];

    assert_eq!(
        normalize_scroll_anchor(0, 0, 10, &mut &heights[..], 4),
        (1, 7),
        "ten rows crosses the three-row entry and advances seven into the next"
    );
    assert_eq!(
        normalize_scroll_anchor(2, 1, -10, &mut &heights[..], 4),
        (0, 2),
        "upward movement uses the same rendered-row distance"
    );
    assert_eq!(
        normalize_scroll_anchor(1, 7, 10, &mut &heights[..], 4),
        (2, 1),
        "the bottom keeps the final five rendered interaction rows visible"
    );
}

#[test]
fn bottom_scroll_keeps_five_lines_or_the_whole_short_interaction() {
    let long = [3, 8, 5, 1, 1];
    assert_eq!(bottom_scroll_anchor(&mut &long[..], 4, 5), (2, 1));

    let short = [1, 1, 1, 1];
    assert_eq!(
        normalize_scroll_anchor(0, 0, 10, &mut &short[..], 3),
        (0, 0),
        "all three interaction lines remain visible"
    );
}

#[test]
fn opening_an_interaction_centers_its_tail_in_the_viewport() {
    // A one-line tail in a ten-row viewport gets four lines above it: the
    // eight-line entry before it is clipped to its last four.
    let heights = [3, 8, 1];
    assert_eq!(
        selection_center_anchor(Some(2), &mut &heights[..], 10),
        (1, 4)
    );

    // The selected entry's own height is centered, not just its first row.
    assert_eq!(
        selection_center_anchor(Some(2), &mut &[3, 8, 4][..], 10),
        (1, 5)
    );

    // A tail taller than the viewport starts at the top.
    assert_eq!(
        selection_center_anchor(Some(1), &mut &[3, 12][..], 10),
        (1, 0)
    );

    // A short interaction never loses its beginning merely to create the
    // preferred position for its tail.
    assert_eq!(
        selection_center_anchor(Some(1), &mut &[3, 1][..], 20),
        (0, 0)
    );
}

#[test]
fn submitting_preserves_space_or_reveals_only_the_missing_rows() {
    for (heights, current, selected, viewport, expected) in [
        (vec![3, 2, 1], (0, 0), 1, 12, (0, 0)),
        // Exactly three response rows already fit.
        (vec![5, 2, 1], (0, 0), 1, 10, (0, 0)),
        // Free just two rows, without dropping the whole history entry.
        (vec![7, 2, 1], (0, 0), 1, 10, (0, 2)),
        // Preserve an existing partial-entry anchor when enough room remains.
        (vec![7, 2, 1], (0, 4), 1, 10, (0, 4)),
        // Bring the end back into view when reading older history.
        (vec![20, 8, 2, 1], (0, 0), 2, 10, (1, 3)),
        // A tall prompt can be clipped so its ending and the reply fit.
        (vec![4, 15, 1], (0, 0), 1, 10, (1, 8)),
        // Tiny viewports still show at least the end of the prompt.
        (vec![4, 5, 1], (0, 0), 1, 2, (1, 4)),
    ] {
        assert_eq!(
            tail_space_anchor(
                current.0,
                current.1,
                selected,
                &mut &heights[..],
                viewport,
                3
            ),
            expected,
            "heights={heights:?}, current={current:?}, viewport={viewport}"
        );
    }
    assert_eq!(
        tail_space_anchor(0, 2, 2, &mut &[7, 2, 1, 1][..], 10, 0),
        (0, 2),
        "the first response keeps the partial history anchor"
    );
}

/// Counts which items the offset math asked about, so laziness can be
/// asserted rather than assumed.
struct Counting {
    heights: Vec<usize>,
    asked: std::collections::BTreeSet<usize>,
}

impl Heights for Counting {
    fn len(&self) -> usize {
        self.heights.len()
    }

    fn height(&mut self, index: usize) -> usize {
        self.asked.insert(index);
        self.heights[index]
    }
}

/// The whole point of asking one height at a time: a thousand-row session
/// must not have to be rendered to decide where to scroll it.
///
/// The math reads a window around the viewport — forward from the offset
/// until the viewport is full, and at most `margin` rows back from it —
/// so what it touches is bounded by the viewport, not by the session.
#[test]
fn the_offset_math_only_asks_about_items_near_the_viewport() {
    let mut heights = Counting {
        heights: vec![1; 1000],
        asked: Default::default(),
    };
    let viewport_height = 20;

    let offset = list_offset_with_scrolloff(500, Some(504), &mut heights, viewport_height, true);

    assert!(
        heights
            .asked
            .iter()
            .all(|&index| (480..540).contains(&index)),
        "asked about items far from the viewport: {:?}",
        heights.asked,
    );
    assert!(
        heights.asked.len() < 100,
        "asked about {} of 1000 items",
        heights.asked.len(),
    );
    // Still the same answer the eager version gave for a flat list.
    let flat = vec![1usize; 1000];
    assert_eq!(
        offset,
        list_offset_with_scrolloff(500, Some(504), &mut &flat[..], viewport_height, true),
    );
}

/// A list with no selection is the one case that needs a count rather than
/// any height at all, and it must not start rendering to get one.
#[test]
fn an_unselected_list_asks_about_no_items_at_all() {
    let mut heights = Counting {
        heights: vec![1; 1000],
        asked: Default::default(),
    };

    let offset = list_offset_with_scrolloff(400, None, &mut heights, 20, false);

    assert_eq!(offset, 400);
    assert!(heights.asked.is_empty(), "{:?}", heights.asked);
}

/// The text of the rows an entry renders to, for comparing one render
/// against another.
fn rows_of(item: &ListItem<'static>) -> Vec<String> {
    let mut out = Vec::new();
    let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 120, 40));
    ratatui::widgets::Widget::render(List::new(vec![item.clone()]), buffer.area, &mut buffer);
    for row in 0..item.height() {
        let mut text = String::new();
        for column in 0..120 {
            text.push_str(buffer[(column, row as u16)].symbol());
        }
        out.push(text.trim_end().to_owned());
    }
    out
}

fn entry_of<'a>(event: &'a AgentEvent, version: EntryVersion) -> EventEntry<'a> {
    EventEntry {
        event,
        version,
        expanded: true,
        has_detail: true,
        contract: None,
        selected: false,
        link_highlight: None,
        branch: Default::default(),
        branch_provider_switch: None,
        branch_follows_marker: false,
        selected_branch: None,
    }
}

fn render_of() -> EntryRender<'static> {
    EntryRender {
        protocol: Protocol::default(),
        links: LinkDisplay::Compact,
        search: None,
    }
}

/// The whole point of keying on the version: a row that has been rewritten
/// is a different row, and must not be answered from what the previous
/// version rendered to.
///
/// This is the failure the cache could actually cause — a tool that has
/// finished still drawn as running — so it is worth asserting directly
/// rather than trusting the key by inspection.
#[test]
fn a_rewritten_entry_is_not_answered_with_its_previous_rendering() {
    let version = version();
    let started = AgentEvent::ToolStarted {
        id: "t1".into(),
        name: "Bash".into(),
        detail: "{\"command\":\"cargo build\"}".into(),
    };
    let before = rows_of(&entry_item(
        &entry_of(&started, version),
        120,
        40,
        render_of(),
    ));

    // The same row, one revision later — exactly what `ingest` does when
    // the tool finishes.
    let completed = AgentEvent::ToolCompleted {
        id: "t1".into(),
        name: "Bash".into(),
        detail: "{\"command\":\"cargo build\"}".into(),
        status: "error".into(),
        output: "could not compile".into(),
    };
    let rewritten = EntryVersion {
        revision: version.revision + 1,
        ..version
    };
    let after = rows_of(&entry_item(
        &entry_of(&completed, rewritten),
        120,
        40,
        render_of(),
    ));

    assert_ne!(before, after, "the finished tool must render as finished");
    assert!(
        after.iter().any(|row| row.contains("could not compile")),
        "the rewritten row shows the new event: {after:?}"
    );
}

/// A hit has to be indistinguishable from a build, or the cache is a
/// second renderer that can disagree with the first.
#[test]
fn a_second_render_of_an_unchanged_entry_matches_the_first() {
    let version = version();
    let event = AgentEvent::AgentMessage {
        text: "a paragraph\n\nand a second one with `code` and a [link](https://e.com)".into(),
    };
    let first = rows_of(&entry_item(
        &entry_of(&event, version),
        120,
        40,
        render_of(),
    ));
    let second = rows_of(&entry_item(
        &entry_of(&event, version),
        120,
        40,
        render_of(),
    ));

    assert_eq!(first, second);
}

/// A pane narrowed by a preview opening beside it re-wraps the entries
/// above the selection onto more rows. The selection stays on the row it
/// was drawn on, rather than being pushed down by them.
#[test]
fn the_selection_keeps_its_row_when_the_pane_changes_width() {
    let long = AgentEvent::AgentMessage {
        text: "a message long enough to wrap onto more rows in a narrow pane ".repeat(2),
    };
    let focused = AgentEvent::AgentMessage {
        text: "focused".into(),
    };
    let mut events = vec![&long; 6];
    events.push(&focused);
    events.push(&long);
    let draw = |width, previous: EventListFeedback, kept: bool| {
        let entries = events
            .iter()
            .enumerate()
            .map(|(index, event)| EventEntry {
                selected: index == 6,
                ..entry_of(event, version())
            })
            .collect();
        let view = EventListView {
            anchor: ListAnchor::Kept,
            requested_offset: previous.effective_offset,
            requested_row_offset: previous.effective_row_offset,
            rendered_selection_row: previous.selection_row.filter(|_| kept),
            ..list_view(entries)
        };
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
        let mut feedback = EventListFeedback::default();
        terminal
            .draw(|frame| feedback = render(frame, &view, frame.area()))
            .unwrap();
        let rows = buffer_rows(terminal.backend().buffer());
        let row = rows.iter().position(|row| row.contains("focused"));
        (feedback, row)
    };
    let start = EventListFeedback {
        effective_offset: 2,
        ..EventListFeedback::default()
    };
    let (wide, wide_row) = draw(160, start, true);
    assert!(wide.selection_row.is_some(), "{wide:?}");

    let (narrow, narrow_row) = draw(60, wide, true);
    assert_eq!(narrow_row, wide_row);
    assert_eq!(
        narrow.selection_row.map(|at| at.row),
        wide.selection_row.map(|at| at.row)
    );
    let (_, unkept_row) = draw(60, wide, false);
    assert_ne!(unkept_row, wide_row, "the entries above did re-wrap");

    let (_, again_row) = draw(60, narrow, true);
    assert_eq!(again_row, wide_row, "the next frame stays where it was put");
    let (_, wide_again_row) = draw(160, narrow, true);
    assert_eq!(wide_again_row, wide_row, "and so does widening back");
}

/// Width is in the key because nothing below the cache re-wraps: the rows
/// it holds are already wrapped. A narrower pane has to rebuild them.
/// An entry taller than the pane is cut short at its foot only until the
/// view scrolls down to its end, which then shows in full.
#[test]
fn scrolling_through_an_entry_taller_than_the_pane_reaches_its_end() {
    let event = AgentEvent::AgentMessage {
        text: (0..30)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let screen = |anchor, scroll_delta| {
        let view = EventListView {
            anchor,
            scroll_delta,
            ..list_view(vec![EventEntry {
                selected: true,
                ..entry_of(&event, version())
            }])
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut feedback = EventListFeedback::default();
        terminal
            .draw(|frame| feedback = render(frame, &view, frame.area()))
            .unwrap();
        let text = buffer_rows(terminal.backend().buffer()).join("\n");
        (text, feedback.selection_overflow.unwrap())
    };

    // The overflow is what `j`/`k` scroll through before leaving it.
    let (top, overflow) = screen(ListAnchor::Selection, 0);
    assert!(top.contains("more rows"), "{top}");
    assert_eq!(overflow.above, 0);
    let hidden = overflow.below;
    assert!(hidden > 10, "{overflow:?}");
    let (half, overflow) = screen(ListAnchor::Free, 10);
    assert!(half.contains("line 12"), "{half}");
    assert!(half.contains("more rows"), "{half}");
    assert_eq!((overflow.above, overflow.below), (10, hidden - 10));
    let (end, overflow) = screen(ListAnchor::Free, 100);
    assert!(end.contains("line 29"), "{end}");
    assert!(!end.contains("more rows"), "{end}");
    assert_eq!(overflow.below, 0);
}

#[test]
fn a_narrower_pane_does_not_reuse_rows_wrapped_for_a_wider_one() {
    let version = version();
    let event = AgentEvent::AgentMessage {
        text: "a long enough sentence that it must wrap when the pane is narrow".into(),
    };
    let wide = entry_item(&entry_of(&event, version), 120, 40, render_of());
    let narrow = entry_item(&entry_of(&event, version), 24, 40, render_of());

    assert!(
        narrow.height() > wide.height(),
        "wrapping at 24 columns takes more rows than at 120"
    );
}
