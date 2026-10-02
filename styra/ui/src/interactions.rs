//! The live-interaction navigator embedded above the event timeline.

use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;
use std::borrow::Cow;

/// A quarter-filled circle turning one step per event: solid enough to catch
/// the eye in a long list, where a single braille dot is easily missed.
const RUNNING_INDICATOR: [&str; 4] = ["◐", "◓", "◑", "◒"];

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
    Stopped,
    Error,
    Ended,
}

pub enum InteractionRow<'a> {
    Workspace(Cow<'a, str>),
    Interaction {
        name: Cow<'a, str>,
        provider: &'a str,
        /// The Git branch checked out for this interaction. `None` means the
        /// interaction is not associated with a Git checkout; a detached
        /// checkout is passed as the descriptive text `detached head`.
        branch: Option<&'a str>,
        status: InteractionStatus,
        current: bool,
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
        completed: bool,
        /// Set only when [`Self::completed`] is true because the operator
        /// sealed the Session, not merely completed it — the row's badge
        /// reads "SEALED" instead of "COMPLETED" so the irreversible state
        /// reads differently from the reversible one.
        sealed: bool,
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
    pub rows: Vec<InteractionRow<'a>>,
}

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

pub fn render(frame: &mut Frame, view: &InteractionNavigator<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        // Scope and completion filter are state the list cannot be read
        // without; the commands that change them are behind `?`.
        .title(format!(
            " {} · interactions · {} · ? keys ",
            view.scope, view.completion_filter
        ));
    let block = match view.filter {
        Some(filter) if view.typing_filter || !filter.is_empty() => {
            block.title_bottom(Line::from(vec![
                Span::styled(" /", Style::default().fg(palette::MUTED_TEXT)),
                Span::styled(
                    filter.to_owned(),
                    Style::default()
                        .fg(palette::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if view.typing_filter { "▏ " } else { " " },
                    Style::default().fg(palette::ACCENT),
                ),
            ]))
        }
        _ => block,
    };
    let mut items = view.rows.iter().map(row_item).collect::<Vec<_>>();
    if items.is_empty() {
        if let Some(filter) = view.filter.filter(|filter| !filter.is_empty()) {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("  no interaction matches {filter}"),
                Style::default()
                    .fg(palette::MUTED_TEXT)
                    .add_modifier(Modifier::DIM),
            ))));
        }
    }
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(
        view.rows
            .iter()
            .position(|row| matches!(row, InteractionRow::Interaction { selected: true, .. })),
    );
    frame.render_stateful_widget(list, area, &mut state);
}

fn row_item(row: &InteractionRow<'_>) -> ListItem<'static> {
    if let InteractionRow::Workspace(name) = row {
        return ListItem::new(Line::from(Span::styled(
            format!(" {name}"),
            Style::default()
                .fg(palette::WORKSPACE_NAME)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let InteractionRow::Interaction {
        name,
        provider,
        branch,
        status,
        current,
        loading,
        newly_idle,
        stop_reason,
        rate_limited,
        uncommitted,
        completed,
        sealed,
        tags,
        last_message,
        ..
    } = row
    else {
        unreachable!()
    };
    let (marker, color) = status_marker(*status);
    // The marker sits on the row's own background; a running spinner is told
    // apart by its glyph and color, not by a patch behind it.
    let marker_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let running = matches!(status, InteractionStatus::Running { .. });
    let mut main = vec![
        Span::styled(
            if *current { "• " } else { "  " },
            Style::default().fg(if *current {
                palette::SELECTION_MARKER
            } else {
                palette::INACTIVE
            }),
        ),
        Span::styled(marker.to_string(), marker_style),
        Span::raw(" "),
    ];
    // Work left uncommitted is flagged ahead of the prompt, where the eye
    // starts on every row, rather than at the end of a line of labels.
    if *uncommitted {
        main.push(Span::styled(
            "! ",
            Style::default()
                .fg(palette::UNCOMMITTED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    // A working agent's prompt is bold too, so its row still stands out when
    // the eye lands between spinner steps.
    let name_style = Style::default().fg(palette::TEXT);
    main.push(Span::styled(
        name.to_string(),
        if running {
            name_style.add_modifier(Modifier::BOLD)
        } else {
            name_style
        },
    ));
    // Tags belong to the prompt they label, so they follow it directly.
    if !tags.is_empty() {
        main.push(Span::styled(
            format!(" #{}", tags.join(" #")),
            Style::default().fg(palette::INTERACTION_TAG),
        ));
    }
    if let Some(branch) = branch {
        main.push(Span::styled(
            format!(" · {branch}"),
            Style::default().fg(palette::ACCENT),
        ));
    }
    main.push(Span::styled(
        format!(" · {provider}"),
        Style::default().fg(palette::INTERACTION_STATUS_INFO),
    ));
    if let Some(why) = stop_reason {
        main.push(Span::styled(
            format!(" · {why}"),
            Style::default().fg(palette::INTERACTION_STATUS_INFO),
        ));
    }
    if *loading {
        main.push(Span::styled(
            " · loading…",
            Style::default().fg(palette::INACTIVE),
        ));
    }
    if let Some(window) = rate_limited {
        main.push(Span::styled(
            format!(" · RATE LIMITED ({window})"),
            Style::default()
                .fg(palette::ERROR)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if *newly_idle {
        main.push(Span::styled(
            " · NEWLY IDLE",
            Style::default()
                .fg(palette::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if *sealed {
        main.push(Span::styled(
            " · SEALED",
            Style::default()
                .fg(palette::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    } else if *completed {
        main.push(Span::styled(
            " · COMPLETED",
            Style::default()
                .fg(palette::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    // The tint is the line's own style, so it runs the full width of the row.
    let mut lines =
        vec![Line::from(main).style(Style::default().bg(palette::INTERACTION_ROW_BACKGROUND))];
    if let Some(text) = last_message {
        lines.push(Line::from(Span::styled(
            format!("    « {text}"),
            Style::default().fg(palette::SUBORDINATE_TEXT),
        )));
    }
    ListItem::new(lines)
}

fn status_marker(status: InteractionStatus) -> (&'static str, ratatui::style::Color) {
    match status {
        InteractionStatus::Pending => (".", palette::INFO),
        InteractionStatus::Running { events } => (running_indicator(events), palette::RUNNING),
        InteractionStatus::Idle => ("o", palette::SUCCESS),
        InteractionStatus::Background => ("*", palette::MUTED_WARNING),
        InteractionStatus::Stopped => ("#", palette::INACTIVE),
        // Not `!`, which marks work left uncommitted on the same row.
        InteractionStatus::Error => ("x", palette::ERROR),
        // Not `x`, which an error is drawn as.
        InteractionStatus::Ended => ("-", palette::INACTIVE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn rendered(view: &InteractionNavigator<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| render(frame, view, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
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
            rows: vec![
                InteractionRow::Workspace("Payments".into()),
                InteractionRow::Interaction {
                    name: "repair checkout".into(),
                    provider: "codex",
                    branch: Some("fix"),
                    status: InteractionStatus::Running { events: 2 },
                    current: true,
                    selected: true,
                    loading: false,
                    newly_idle: true,
                    stop_reason: None,
                    rate_limited: None,
                    uncommitted: false,
                    completed: false,
                    sealed: false,
                    tags: &tags,
                    last_message: Some("The checks are green."),
                },
            ],
        };
        let screen = rendered(&view);
        assert!(screen.contains("Payments"), "{screen}");
        assert!(
            screen.contains("◑ repair checkout #bug #urgent · fix · codex"),
            "{screen}"
        );
        assert!(screen.contains("· NEWLY IDLE"), "{screen}");
        assert!(screen.contains("« The checks are green."), "{screen}");
        assert_eq!(height(&view, 12), 5);
    }

    #[test]
    fn the_interaction_line_is_tinted_across_the_row_and_its_message_is_not() {
        let tags = Vec::new();
        let view = InteractionNavigator {
            scope: "All".into(),
            all_workspaces: true,
            completion_filter: "completed hidden".into(),
            filter: None,
            typing_filter: false,
            rows: vec![InteractionRow::Interaction {
                name: "repair checkout".into(),
                provider: "codex",
                branch: None,
                status: InteractionStatus::Idle,
                current: false,
                selected: false,
                loading: false,
                newly_idle: false,
                stop_reason: None,
                rate_limited: None,
                uncommitted: false,
                completed: false,
                sealed: false,
                tags: &tags,
                last_message: Some("The checks are green."),
            }],
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
        terminal
            .draw(|frame| render(frame, &view, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(buffer[(78, 1)].bg, palette::INTERACTION_ROW_BACKGROUND);
        assert_eq!(buffer[(78, 2)].bg, palette::RESET);
    }

    /// The list is where an operator scanning several stopped agents decides
    /// which one to go back to, so the checkout each of them left behind has
    /// to be readable from the row.
    #[test]
    fn a_stopped_interaction_says_it_left_work_uncommitted() {
        let view = InteractionNavigator {
            scope: "Payments".into(),
            all_workspaces: false,
            completion_filter: "completed hidden".into(),
            filter: None,
            typing_filter: false,
            rows: vec![InteractionRow::Interaction {
                name: "repair checkout".into(),
                provider: "claude",
                branch: None,
                status: InteractionStatus::Idle,
                current: false,
                selected: false,
                loading: false,
                newly_idle: false,
                stop_reason: None,
                rate_limited: None,
                uncommitted: true,
                completed: false,
                sealed: false,
                tags: &[],
                last_message: None,
            }],
        };
        assert!(rendered(&view).contains("! repair checkout · claude"));
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
            rows: vec![InteractionRow::Interaction {
                name: "repair checkout".into(),
                provider: "claude",
                branch: None,
                status: InteractionStatus::Stopped,
                current: false,
                selected: false,
                loading: false,
                newly_idle: false,
                stop_reason: Some("paused".into()),
                rate_limited: None,
                uncommitted: false,
                completed: false,
                sealed: false,
                tags: &[],
                last_message: None,
            }],
        };
        assert!(rendered(&view).contains("# repair checkout · claude · paused"));
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
            rows: vec![InteractionRow::Interaction {
                name: "repair checkout".into(),
                provider: "claude",
                branch: None,
                status: InteractionStatus::Idle,
                current: false,
                selected: false,
                loading: false,
                newly_idle: false,
                stop_reason: None,
                rate_limited: Some("five_hour".into()),
                uncommitted: false,
                completed: false,
                sealed: false,
                tags: &[],
                last_message: None,
            }],
        };
        assert!(rendered(&view).contains("repair checkout · claude · RATE LIMITED (five_hour)"));
    }

    #[test]
    fn current_and_cursor_are_independent() {
        let view = InteractionNavigator {
            scope: "Payments".into(),
            all_workspaces: false,
            completion_filter: "completed hidden".into(),
            filter: None,
            typing_filter: false,
            rows: vec![
                InteractionRow::Interaction {
                    name: "shown below".into(),
                    provider: "claude",
                    branch: None,
                    status: InteractionStatus::Idle,
                    current: true,
                    selected: false,
                    loading: false,
                    newly_idle: false,
                    stop_reason: None,
                    rate_limited: None,
                    uncommitted: false,
                    completed: false,
                    sealed: false,
                    tags: &[],
                    last_message: None,
                },
                InteractionRow::Interaction {
                    name: "being loaded".into(),
                    provider: "codex",
                    branch: None,
                    status: InteractionStatus::Pending,
                    current: false,
                    selected: true,
                    loading: true,
                    newly_idle: false,
                    stop_reason: None,
                    rate_limited: None,
                    uncommitted: false,
                    completed: false,
                    sealed: false,
                    tags: &[],
                    last_message: None,
                },
            ],
        };
        let screen = rendered(&view);
        assert!(screen.contains("• o shown below"), "{screen}");
        assert!(
            screen.contains(". being loaded · codex · loading…"),
            "{screen}"
        );
    }
}
