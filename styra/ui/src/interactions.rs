//! The live-interaction navigator embedded above the event timeline.

use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;
use std::borrow::Cow;

const RUNNING_INDICATOR: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
        status: InteractionStatus,
        current: bool,
        selected: bool,
        loading: bool,
        newly_idle: bool,
        /// The interaction has stopped and left work uncommitted in its
        /// repository — see [`crate::footer::FooterView::uncommitted_changes`].
        uncommitted: bool,
        completed: bool,
        tags: &'a [String],
        last_message: Option<&'a str>,
    },
}

pub struct InteractionNavigator<'a> {
    pub scope: Cow<'a, str>,
    pub all_workspaces: bool,
    pub completion_filter: Cow<'a, str>,
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
    let width = area.width.saturating_sub(2);
    let items = view.rows.iter().map(|row| row_item(row, width));
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

fn row_item(row: &InteractionRow<'_>, width: u16) -> ListItem<'static> {
    if let InteractionRow::Workspace(name) = row {
        return ListItem::new(Line::from(Span::styled(
            format!(" {name}"),
            Style::default()
                .fg(palette::WARNING)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let InteractionRow::Interaction {
        name,
        provider,
        status,
        current,
        loading,
        newly_idle,
        uncommitted,
        completed,
        tags,
        last_message,
        ..
    } = row
    else {
        unreachable!()
    };
    let (marker, color) = status_marker(*status);
    let mut main = vec![
        Span::styled(
            if *current { "• " } else { "  " },
            Style::default().fg(if *current {
                palette::SELECTION_MARKER
            } else {
                palette::INACTIVE
            }),
        ),
        Span::styled(
            format!("{marker} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(name.to_string(), Style::default().fg(palette::TEXT)),
        Span::styled(
            format!(" · {provider}"),
            Style::default().fg(palette::ACCENT),
        ),
    ];
    if *loading {
        main.push(Span::styled(
            " · loading…",
            Style::default().fg(palette::INACTIVE),
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
    if *uncommitted {
        main.push(Span::styled(
            " · UNCOMMITTED",
            Style::default()
                .fg(palette::WARNING)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if *completed {
        main.push(Span::styled(
            " · COMPLETED",
            Style::default()
                .fg(palette::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !tags.is_empty() {
        main.push(Span::styled(
            format!(" · #{}", tags.join(" #")),
            Style::default().fg(palette::WARNING),
        ));
    }
    let mut lines = vec![Line::from(main)];
    if let Some(text) = last_message {
        let body = format!("    « {text}");
        let padding = usize::from(width).saturating_sub(body.chars().count());
        lines.push(Line::from(Span::styled(
            format!("{body}{}", " ".repeat(padding)),
            Style::default()
                .fg(palette::SUBORDINATE_TEXT)
                .bg(palette::SUBORDINATE_BACKGROUND),
        )));
    }
    ListItem::new(lines)
}

fn status_marker(status: InteractionStatus) -> (&'static str, ratatui::style::Color) {
    match status {
        InteractionStatus::Pending => (".", palette::INFO),
        InteractionStatus::Running { events } => (
            RUNNING_INDICATOR[events % RUNNING_INDICATOR.len()],
            palette::WARNING,
        ),
        InteractionStatus::Idle => ("o", palette::SUCCESS),
        InteractionStatus::Background => ("*", palette::MUTED_WARNING),
        InteractionStatus::Stopped => ("#", palette::INACTIVE),
        InteractionStatus::Error => ("!", palette::ERROR),
        InteractionStatus::Ended => ("x", palette::INACTIVE),
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
            rows: vec![
                InteractionRow::Workspace("Payments".into()),
                InteractionRow::Interaction {
                    name: "repair checkout".into(),
                    provider: "codex",
                    status: InteractionStatus::Running { events: 3 },
                    current: true,
                    selected: true,
                    loading: false,
                    newly_idle: true,
                    uncommitted: false,
                    completed: false,
                    tags: &tags,
                    last_message: Some("The checks are green."),
                },
            ],
        };
        let screen = rendered(&view);
        assert!(screen.contains("Payments"), "{screen}");
        assert!(screen.contains("⠸ repair checkout · codex"), "{screen}");
        assert!(screen.contains("NEWLY IDLE · #bug #urgent"), "{screen}");
        assert!(screen.contains("« The checks are green."), "{screen}");
        assert_eq!(height(&view, 12), 5);
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
            rows: vec![InteractionRow::Interaction {
                name: "repair checkout".into(),
                provider: "claude",
                status: InteractionStatus::Idle,
                current: false,
                selected: false,
                loading: false,
                newly_idle: false,
                uncommitted: true,
                completed: false,
                tags: &[],
                last_message: None,
            }],
        };
        assert!(rendered(&view).contains("repair checkout · claude · UNCOMMITTED"));
    }

    #[test]
    fn current_and_cursor_are_independent() {
        let view = InteractionNavigator {
            scope: "Payments".into(),
            all_workspaces: false,
            completion_filter: "completed hidden".into(),
            rows: vec![
                InteractionRow::Interaction {
                    name: "shown below".into(),
                    provider: "claude",
                    status: InteractionStatus::Idle,
                    current: true,
                    selected: false,
                    loading: false,
                    newly_idle: false,
                    uncommitted: false,
                    completed: false,
                    tags: &[],
                    last_message: None,
                },
                InteractionRow::Interaction {
                    name: "being loaded".into(),
                    provider: "codex",
                    status: InteractionStatus::Pending,
                    current: false,
                    selected: true,
                    loading: true,
                    newly_idle: false,
                    uncommitted: false,
                    completed: false,
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
