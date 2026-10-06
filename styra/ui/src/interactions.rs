//! The live-interaction navigator embedded above the event timeline.

use crate::chrome::StopTone;
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use std::borrow::Cow;
use styra_protocol::CompletionState;

/// Three dots, the heavy one moving rightward one step per event and wrapping
/// back to the left. It fills the whole status block, so a running row reads
/// apart from every other state's single marker, and it holds still when the
/// agent goes quiet.
const RUNNING_INDICATOR: [&str; 3] = ["•··", "·•·", "··•"];

/// The spinner frame for a running turn that has seen `events` events. The
/// navigator row and the conversation's running tail draw the same one.
pub(crate) fn running_indicator(events: usize) -> &'static str {
    RUNNING_INDICATOR[events % RUNNING_INDICATOR.len()]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionStatus {
    Pending,
    Running { events: usize },
    Idle,
    Background,
    Stopped(StopTone),
    Error,
    Ended,
}

pub enum InteractionRow<'a> {
    Workspace(Cow<'a, str>),
    /// The directory several interactions in one workspace share, heading the
    /// rows for them, which follow it with `grouped` set.
    Directory(Cow<'a, str>),
    Interaction {
        name: Cow<'a, str>,
        /// The row sits under a [`InteractionRow::Directory`] heading, and is
        /// indented beneath it.
        grouped: bool,
        /// How many sources deep this interaction was branched: nested
        /// beneath the entry it came from, as the Session picker nests it.
        depth: usize,
        provider: &'a str,
        /// The Git branch checked out for this interaction. `None` means the
        /// interaction is not associated with a Git checkout; a detached
        /// checkout is passed as the descriptive text `detached head`.
        branch: Option<&'a str>,
        status: InteractionStatus,
        selected: bool,
        loading: bool,
        newly_idle: bool,
        /// Why a stopped interaction stopped. Unlike the reasons the other
        /// statuses carry, this one is rendered on the row: the stopped
        /// entries sit at the foot of the list precisely so they can be read
        /// through, and "stopped" on its own does not say whether the operator
        /// paused it, a plan window refused it, or the agent fell over.
        stop_reason: Option<Cow<'a, str>>,
        /// The window that refused this interaction's work, when it is idle
        /// because a plan window turned it away rather than because its turn
        /// ended. Idling of that kind is not the operator's turn to speak:
        /// nothing they send will run until the window turns over, so the row
        /// says so instead of reading as ordinary idle.
        rate_limited: Option<Cow<'a, str>>,
        /// The interaction has stopped and left work uncommitted in its
        /// repository — see [`crate::footer::FooterView::uncommitted_changes`].
        uncommitted: bool,
        /// Whether, and how, the operator is finished with the Session. Each
        /// way of being done has its own badge, so a Session given up on
        /// does not read as one that got done, and the irreversible seal
        /// does not read as either.
        completion: CompletionState,
        tags: &'a [String],
        last_message: Option<&'a str>,
    },
}

pub struct InteractionNavigator<'a> {
    pub scope: Cow<'a, str>,
    pub all_workspaces: bool,
    pub completion_filter: Cow<'a, str>,
    /// The `/` filter's term, when one is being typed or is in force.
    pub filter: Option<&'a str>,
    /// Whether the filter is still being typed, so it is drawn with a caret.
    pub typing_filter: bool,
    /// The first line shown last frame. [`render`] keeps it unless the cursor
    /// comes within [`SCROLL_MARGIN`] rows of an edge, and returns the line it
    /// actually drew from.
    pub requested_offset: usize,
    pub rows: Vec<InteractionRow<'a>>,
}

/// How many rows the view keeps between the cursor and its top or bottom edge
/// before it scrolls, where the list has rows there to show.
const SCROLL_MARGIN: usize = 2;

pub fn height(view: &InteractionNavigator<'_>, available: u16) -> u16 {
    let message_rows = view
        .rows
        .iter()
        .filter(|row| {
            matches!(
                row,
                InteractionRow::Interaction {
                    last_message: Some(_),
                    ..
                }
            )
        })
        .count() as u16;
    (view.rows.len() as u16 + message_rows + 2)
        .max(3)
        .min(available.saturating_div(2).max(3))
        .min(available)
}

pub fn render(frame: &mut Frame, view: &InteractionNavigator<'_>, area: Rect) -> usize {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        // Scope and completion filter are state the list cannot be read
        // without; the commands that change them are behind `?`.
        .title(format!(
            " {} · interactions · {} · ? keys ",
            view.scope, view.completion_filter
        ));
    let block = match view.filter {
        Some(filter) if view.typing_filter || !filter.is_empty() => {
            block.title_bottom(Line::from(vec![
                Span::styled(" /", Style::default().fg(theme::MUTED_TEXT)),
                Span::styled(
                    filter.to_owned(),
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if view.typing_filter { "▏ " } else { " " },
                    Style::default().fg(theme::ACCENT),
                ),
            ]))
        }
        _ => block,
    };
    // Every other interaction under a heading is drawn on a faint stripe, so
    // the eye can follow a row across the pane and down to its message line.
    // The cursor's row is drawn on a slightly lighter tint in its place.
    let mut striped = false;
    let mut items = view
        .rows
        .iter()
        .map(|row| {
            let mut item = row_item(row, view.all_workspaces);
            match row {
                InteractionRow::Workspace(_) | InteractionRow::Directory(_) => striped = false,
                InteractionRow::Interaction { selected, .. } => {
                    let background = if *selected {
                        Some(theme::SELECTED_ROW_BACKGROUND)
                    } else {
                        striped.then_some(theme::ALTERNATE_ROW_BACKGROUND)
                    };
                    if let Some(background) = background {
                        for line in &mut item {
                            line.style = line.style.bg(background);
                        }
                    }
                    striped = !striped;
                }
            }
            item
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        if let Some(filter) = view.filter.filter(|filter| !filter.is_empty()) {
            items.push(vec![Line::from(Span::styled(
                format!("  no interaction matches {filter}"),
                Style::default()
                    .fg(theme::MUTED_TEXT)
                    .add_modifier(Modifier::DIM),
            ))]);
        }
    }
    let selected = view
        .rows
        .iter()
        .position(|row| matches!(row, InteractionRow::Interaction { selected: true, .. }));
    // The list is laid out by line rather than by row, so a row with a message
    // line that does not fit is drawn as far as it goes instead of leaving the
    // foot of the pane empty, as `List` would.
    let lines = items.into_iter().flatten().collect::<Vec<_>>();
    let inner = block.inner(area);
    let viewport = usize::from(inner.height);
    let offset = scroll_offset(view, selected, viewport, lines.len());
    let above = offset;
    let below = lines.len().saturating_sub(offset + viewport);
    let mut block = block;
    if above > 0 {
        block = block.title(more_lines("↑", above).right_aligned());
    }
    if below > 0 {
        block = block.title_bottom(more_lines("↓", below).right_aligned());
    }
    frame.render_widget(block, area);
    for (row, line) in lines.into_iter().skip(offset).take(viewport).enumerate() {
        let line_area = Rect {
            y: inner.y + row as u16,
            height: 1,
            ..inner
        };
        frame.render_widget(line, line_area);
    }
    offset
}

/// The first line to draw. The view stays where it was while the cursor moves
/// inside it, and scrolls only as far as it takes to keep [`SCROLL_MARGIN`]
/// rows showing past the cursor, fewer where the pane is too short for them.
fn scroll_offset(
    view: &InteractionNavigator<'_>,
    selected: Option<usize>,
    viewport: usize,
    total: usize,
) -> usize {
    // The foot of the pane is never left empty while there is list to fill it.
    let last = total.saturating_sub(viewport);
    let mut offset = view.requested_offset.min(last);
    let Some(selected) = selected else {
        return offset;
    };
    let lines = |rows: &[InteractionRow<'_>]| rows.iter().map(row_height).sum::<usize>();
    let start = lines(&view.rows[..selected]);
    let end = start + row_height(&view.rows[selected]);
    let mut margin = SCROLL_MARGIN;
    let (above, below) = loop {
        let above = lines(&view.rows[selected.saturating_sub(margin)..selected]);
        let below = lines(&view.rows[selected + 1..(selected + 1 + margin).min(view.rows.len())]);
        if margin == 0 || above + (end - start) + below <= viewport {
            break (above, below);
        }
        margin -= 1;
    };
    // The top edge is applied last, so a row taller than the pane shows its
    // first line rather than its last.
    offset = offset.max((end + below).saturating_sub(viewport));
    offset.min(start - above)
}

/// The note on the pane's border that the list runs on past its edge.
fn more_lines(arrow: &str, count: usize) -> Line<'static> {
    let noun = if count == 1 { "line" } else { "lines" };
    Line::from(Span::styled(
        format!(" {arrow} {count} more {noun} "),
        Style::default().fg(theme::MUTED_TEXT),
    ))
}

fn row_height(row: &InteractionRow<'_>) -> usize {
    match row {
        InteractionRow::Interaction {
            last_message: Some(_),
            ..
        } => 2,
        _ => 1,
    }
}

/// `under_workspaces` says whether the list is drawn under Workspace
/// headings, which every row then steps in beneath.
fn row_item(row: &InteractionRow<'_>, under_workspaces: bool) -> Vec<Line<'static>> {
    // Each heading's rows are stepped in past where its name starts, so which
    // heading a row belongs to reads from the left edge. The Workspace
    // headings, the outermost, sit against the edge itself.
    let step = |levels: usize| " ".repeat(2 * levels);
    let workspace_level = usize::from(under_workspaces);
    match row {
        InteractionRow::Workspace(name) => {
            return vec![Line::from(Span::styled(
                name.to_string(),
                Style::default()
                    .fg(theme::WORKSPACE_NAME)
                    .add_modifier(Modifier::BOLD),
            ))];
        }
        InteractionRow::Directory(name) => {
            return vec![Line::from(Span::styled(
                format!(" {}{name}/", step(workspace_level)),
                Style::default().fg(theme::DIRECTORY_NAME),
            ))];
        }
        InteractionRow::Interaction { .. } => {}
    }
    let InteractionRow::Interaction {
        name,
        grouped,
        depth,
        provider,
        branch,
        status,
        selected,
        loading,
        newly_idle,
        stop_reason,
        rate_limited,
        uncommitted,
        completion,
        tags,
        last_message,
        ..
    } = row
    else {
        unreachable!()
    };
    let (marker, color) = status_marker(*status);
    // The marker sits on a faint block of its own hue at the head of the row,
    // so the state reads from the color down the left edge of the list and
    // from the shape up close. The cursor lights its row's block up, so the
    // status still reads under it.
    let marker_style = Style::default()
        .fg(color)
        .bg(status_background(*status, *selected))
        .add_modifier(Modifier::BOLD);
    let running = matches!(status, InteractionStatus::Running { .. });
    let indent = step(workspace_level + usize::from(*grouped));
    let edge = branch_indent(*depth);
    let mut main = vec![
        Span::raw(indent.clone()),
        Span::styled(edge.clone(), Style::default().fg(theme::INACTIVE)),
        // The running indicator is already as wide as the block.
        Span::styled(
            if running {
                marker.to_owned()
            } else {
                format!(" {marker} ")
            },
            marker_style,
        ),
        Span::raw(" "),
    ];
    // Work left uncommitted is flagged ahead of the prompt, where the eye
    // starts on every row, rather than at the end of a line of labels.
    if *uncommitted {
        main.push(Span::styled(
            "! ",
            Style::default()
                .fg(theme::UNCOMMITTED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    // The prompt is never dimmed, so it stays apart from the gray message line
    // beneath it; a working or idle agent's leans slightly toward its marker's
    // hue instead. The cursor's prompt is a soft cyan and bold, apart from
    // every other row's, whatever its status.
    let name_style = if *selected {
        Style::default()
            .fg(theme::SELECTED_LIVE_INTERACTION_TEXT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(match status {
            InteractionStatus::Running { .. } => theme::RUNNING_INTERACTION_TEXT,
            InteractionStatus::Idle => theme::IDLE_INTERACTION_TEXT,
            _ => theme::TEXT,
        })
    };
    main.push(Span::styled(name.to_string(), name_style));
    // Tags belong to the prompt they label, so they follow it directly.
    if !tags.is_empty() {
        main.push(Span::styled(
            format!(" #{}", tags.join(" #")),
            Style::default().fg(theme::INTERACTION_TAG),
        ));
    }
    // A grouped row's worktree heading already says where it works.
    if let Some(branch) = branch.filter(|_| !*grouped) {
        main.push(Span::styled(
            format!(" · {branch}"),
            Style::default().fg(theme::ACCENT),
        ));
    }
    main.push(Span::styled(
        format!(" · {provider}"),
        Style::default().fg(theme::INTERACTION_STATUS_INFO),
    ));
    if let Some(why) = stop_reason {
        // In the stop's own color, so how it ended is told apart down the
        // foot of the list without reading every word.
        let tone = match status {
            InteractionStatus::Stopped(tone) => tone.color(),
            _ => theme::INTERACTION_STATUS_INFO,
        };
        main.push(Span::styled(format!(" · {why}"), Style::default().fg(tone)));
    }
    if *loading {
        main.push(Span::styled(
            " · loading…",
            Style::default().fg(theme::INACTIVE),
        ));
    }
    if let Some(window) = rate_limited {
        main.push(Span::styled(
            format!(" · RATE LIMITED ({window})"),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if *newly_idle {
        main.push(Span::styled(
            " · NEWLY IDLE",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let badge = match completion {
        CompletionState::Active => None,
        CompletionState::Completed => Some((" · COMPLETED", theme::STOP_COMPLETED)),
        CompletionState::Abandoned => Some((" · ABANDONED", theme::STOP_ABANDONED)),
        CompletionState::Sealed => Some((" · SEALED", theme::STOP_SEALED)),
    };
    if let Some((badge, color)) = badge {
        main.push(Span::styled(
            badge,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![Line::from(main)];
    if let Some(text) = last_message {
        lines.push(Line::from(Span::styled(
            format!("{indent}{}    « {text}", " ".repeat(edge.chars().count())),
            Style::default().fg(theme::SUBORDINATE_TEXT),
        )));
    }
    lines
}

/// The tree edge a branch hangs from, matching the Session picker's: a child
/// starts beneath its source's content, and each further level carries that
/// indent forward.
fn branch_indent(depth: usize) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!("{}└─ ", "  ".repeat(depth))
    }
}

fn status_marker(status: InteractionStatus) -> (&'static str, ratatui::style::Color) {
    match status {
        // Each status has its own shape, so the list reads without its colors.
        InteractionStatus::Pending => ("●", theme::INFO),
        InteractionStatus::Running { events } => (running_indicator(events), theme::RUNNING),
        InteractionStatus::Idle => ("●", theme::SUCCESS),
        InteractionStatus::Background => ("◎", theme::MUTED_WARNING),
        // Not `#`, which starts the row's tags. In the hue of how it stopped,
        // as its reason is, so the reason reads from the left edge too.
        InteractionStatus::Stopped(tone) => ("■", tone.color()),
        // Not `!`, which marks work left uncommitted on the same row.
        InteractionStatus::Error => ("✗", theme::ERROR),
        InteractionStatus::Ended => ("–", theme::INACTIVE),
    }
}

/// The block behind a row's status marker: faint, or lit up under the cursor.
fn status_background(status: InteractionStatus, selected: bool) -> ratatui::style::Color {
    match (status, selected) {
        (InteractionStatus::Pending, false) => theme::PENDING_STATUS_BACKGROUND,
        (InteractionStatus::Pending, true) => theme::PENDING_STATUS_HIGHLIGHT,
        (InteractionStatus::Running { .. }, false) => theme::RUNNING_STATUS_BACKGROUND,
        (InteractionStatus::Running { .. }, true) => theme::RUNNING_STATUS_HIGHLIGHT,
        (InteractionStatus::Idle, false) => theme::IDLE_STATUS_BACKGROUND,
        (InteractionStatus::Idle, true) => theme::IDLE_STATUS_HIGHLIGHT,
        (InteractionStatus::Background, false) => theme::BACKGROUND_STATUS_BACKGROUND,
        (InteractionStatus::Background, true) => theme::BACKGROUND_STATUS_HIGHLIGHT,
        (InteractionStatus::Stopped(_), false) => theme::STOPPED_STATUS_BACKGROUND,
        (InteractionStatus::Stopped(_), true) => theme::STOPPED_STATUS_HIGHLIGHT,
        (InteractionStatus::Error, false) => theme::ERROR_STATUS_BACKGROUND,
        (InteractionStatus::Error, true) => theme::ERROR_STATUS_HIGHLIGHT,
        (InteractionStatus::Ended, false) => theme::ENDED_STATUS_BACKGROUND,
        (InteractionStatus::Ended, true) => theme::ENDED_STATUS_HIGHLIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn rendered(view: &InteractionNavigator<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
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

    fn row(
        name: &'static str,
        depth: usize,
        last_message: Option<&'static str>,
    ) -> InteractionRow<'static> {
        InteractionRow::Interaction {
            name: name.into(),
            grouped: false,
            depth,
            provider: "codex",
            branch: None,
            status: InteractionStatus::Stopped(StopTone::Paused),
            selected: false,
            loading: false,
            newly_idle: false,
            stop_reason: None,
            rate_limited: None,
            uncommitted: false,
            completion: CompletionState::Active,
            tags: &[],
            last_message,
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
        assert!(screen.contains("│ ■  source · codex"), "{screen}");
        assert!(screen.contains("│  └─  ■  branch · codex"), "{screen}");
        assert!(screen.contains("│         « forked"), "{screen}");
    }

    #[test]
    fn renders_grouping_activity_and_continuation_rows() {
        let tags = vec!["bug".into(), "urgent".into()];
        let view = InteractionNavigator {
            scope: "All".into(),
            all_workspaces: true,
            completion_filter: "completed hidden".into(),
            filter: None,
            typing_filter: false,
            requested_offset: 0,
            rows: vec![
                InteractionRow::Workspace("Payments".into()),
                InteractionRow::Interaction {
                    name: "repair checkout".into(),
                    grouped: false,
                    depth: 0,
                    provider: "codex",
                    branch: Some("fix"),
                    status: InteractionStatus::Running { events: 2 },
                    selected: true,
                    loading: false,
                    newly_idle: true,
                    stop_reason: None,
                    rate_limited: None,
                    uncommitted: false,
                    completion: CompletionState::Active,
                    tags: &tags,
                    last_message: Some("The checks are green."),
                },
            ],
        };
        let screen = rendered(&view);
        assert!(screen.contains("Payments"), "{screen}");
        assert!(
            screen.contains("repair checkout #bug #urgent · fix · codex"),
            "{screen}"
        );
        assert!(screen.contains("· NEWLY IDLE"), "{screen}");
        assert!(screen.contains("The checks are green."), "{screen}");
        assert_eq!(height(&view, 12), 5);
    }

    /// Each heading's rows are stepped in past where its name starts: under
    /// a Workspace heading, which sits against the edge, and again under a
    /// directory heading within it,
    /// where they also drop the branch the heading already gives.
    #[test]
    fn rows_are_indented_under_their_headings() {
        let row = |name: &'static str, grouped| InteractionRow::Interaction {
            name: name.into(),
            grouped,
            depth: 0,
            provider: "claude",
            branch: Some("feature"),
            status: InteractionStatus::Idle,
            selected: false,
            loading: false,
            newly_idle: false,
            stop_reason: None,
            rate_limited: None,
            uncommitted: false,
            completion: CompletionState::Active,
            tags: &[],
            last_message: None,
        };
        let lines = |view: &InteractionNavigator<'_>| {
            rendered(view)
                .chars()
                .collect::<Vec<_>>()
                .chunks(80)
                .map(|line| line.iter().collect::<String>())
                .collect::<Vec<_>>()
        };

        let all = InteractionNavigator {
            all_workspaces: true,
            rows: vec![
                InteractionRow::Workspace("Payments".into()),
                row("alone", false),
                InteractionRow::Directory("checkout".into()),
                row("first", true),
                row("second", true),
            ],
            ..many_rows(0)
        };
        let screen = lines(&all);
        assert!(screen[1].starts_with("│Payments"), "{screen:#?}");
        assert!(
            screen[2].starts_with("│   ●  alone · feature · claude"),
            "{screen:#?}"
        );
        assert!(screen[3].starts_with("│   checkout/"), "{screen:#?}");
        assert!(
            screen[4].starts_with("│     ●  first · claude "),
            "{screen:#?}"
        );
        assert!(
            screen[5].starts_with("│     ●  second · claude "),
            "{screen:#?}"
        );

        // In Workspace scope there is no Workspace heading to step in under.
        let one = InteractionNavigator {
            rows: vec![
                row("alone", false),
                InteractionRow::Directory("checkout".into()),
                row("first", true),
            ],
            ..many_rows(0)
        };
        let screen = lines(&one);
        assert!(
            screen[1].starts_with("│ ●  alone · feature · claude"),
            "{screen:#?}"
        );
        assert!(screen[2].starts_with("│ checkout/"), "{screen:#?}");
        assert!(
            screen[3].starts_with("│   ●  first · claude "),
            "{screen:#?}"
        );
    }

    /// A stopped entry is one the operator has to decide whether to resume,
    /// and the reason it stopped is what that decision turns on.
    #[test]
    fn a_stopped_interaction_says_why_it_stopped() {
        let view = InteractionNavigator {
            scope: "Payments".into(),
            all_workspaces: false,
            completion_filter: "completed hidden".into(),
            filter: None,
            typing_filter: false,
            requested_offset: 0,
            rows: vec![InteractionRow::Interaction {
                name: "repair checkout".into(),
                grouped: false,
                depth: 0,
                provider: "claude",
                branch: None,
                status: InteractionStatus::Stopped(StopTone::Paused),
                selected: false,
                loading: false,
                newly_idle: false,
                stop_reason: Some("paused".into()),
                rate_limited: None,
                uncommitted: false,
                completion: CompletionState::Active,
                tags: &[],
                last_message: None,
            }],
        };
        assert!(rendered(&view).contains("repair checkout · claude · paused"));
    }

    /// An interaction idling because a plan window refused it looks exactly
    /// like one waiting for the operator, and is the opposite situation: the
    /// row has to say which of the two it is.
    #[test]
    fn an_idle_interaction_says_when_a_plan_window_is_holding_it() {
        let view = InteractionNavigator {
            scope: "Payments".into(),
            all_workspaces: false,
            completion_filter: "completed hidden".into(),
            filter: None,
            typing_filter: false,
            requested_offset: 0,
            rows: vec![InteractionRow::Interaction {
                name: "repair checkout".into(),
                grouped: false,
                depth: 0,
                provider: "claude",
                branch: None,
                status: InteractionStatus::Idle,
                selected: false,
                loading: false,
                newly_idle: false,
                stop_reason: None,
                rate_limited: Some("five_hour".into()),
                uncommitted: false,
                completion: CompletionState::Active,
                tags: &[],
                last_message: None,
            }],
        };
        assert!(rendered(&view).contains("repair checkout · claude · RATE LIMITED (five_hour)"));
    }

    fn many_rows(selected: usize) -> InteractionNavigator<'static> {
        InteractionNavigator {
            scope: "Payments".into(),
            all_workspaces: false,
            completion_filter: "completed hidden".into(),
            filter: None,
            typing_filter: false,
            requested_offset: 0,
            rows: (0..8)
                .map(|index| InteractionRow::Interaction {
                    name: format!("task {index}").into(),
                    grouped: false,
                    depth: 0,
                    provider: "claude",
                    branch: None,
                    status: InteractionStatus::Idle,
                    selected: index == selected,
                    loading: false,
                    newly_idle: false,
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

    /// The line [`render`] draws [`many_rows`] from, in a pane ten lines tall
    /// inside its border: five two-line rows.
    fn drawn_offset(selected: usize, requested_offset: usize) -> usize {
        let view = InteractionNavigator {
            requested_offset,
            ..many_rows(selected)
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        let mut offset = None;
        terminal
            .draw(|frame| offset = Some(render(frame, &view, frame.area())))
            .unwrap();
        offset.unwrap()
    }

    /// The cursor walks the rows in view without moving them.
    #[test]
    fn the_cursor_moves_freely_inside_the_view() {
        for selected in 0..=2 {
            assert_eq!(drawn_offset(selected, 0), 0, "row {selected}");
        }
        // Rows 2 to 6 in view, the cursor anywhere between their margins.
        assert_eq!(drawn_offset(4, 4), 4);
    }

    /// Two rows are kept between the cursor and the edge it is moving toward,
    /// so the operator sees what comes next before reaching it.
    #[test]
    fn the_view_scrolls_two_rows_before_the_cursor_reaches_an_edge() {
        // Down from the top: row 3 is the first with fewer than two rows below.
        assert_eq!(drawn_offset(3, 0), 2);
        // Up from the bottom, where rows 3 to 7 are in view.
        assert_eq!(drawn_offset(5, 6), 6);
        assert_eq!(drawn_offset(4, 6), 4);
    }

    /// At the ends of the list there are no rows to keep in the margin, so
    /// the view stops there instead of running past the list.
    #[test]
    fn the_view_stops_at_the_ends_of_the_list() {
        assert_eq!(drawn_offset(0, 6), 0);
        assert_eq!(drawn_offset(7, 0), 6);
        assert_eq!(drawn_offset(7, 100), 6);
    }

    /// Each way of being finished with a Session has its own badge, so one
    /// given up on never reads as one that got done.
    #[test]
    fn each_completion_state_has_its_own_badge() {
        for (completion, badge) in [
            (CompletionState::Completed, "· COMPLETED"),
            (CompletionState::Abandoned, "· ABANDONED"),
            (CompletionState::Sealed, "· SEALED"),
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
                (CompletionState::Completed, "· COMPLETED"),
                (CompletionState::Abandoned, "· ABANDONED"),
                (CompletionState::Sealed, "· SEALED"),
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
}
