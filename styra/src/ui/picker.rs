//! The session, Workspace, and current-interactions picker screens. These
//! stand apart from [`crate::app::App`] because each overlays before (or
//! instead of) any loaded session, so they render from their own borrowed
//! data rather than app state.

use super::{log_line, message_text_color, tag_color, SELECTION_BG};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use styra_server::{InteractionSummary, InteractionUpdate, SessionSummary, WorkspaceSummary};

/// Render the session picker screen: every stored session, newest first,
/// with `selected` highlighted. Standalone from [`crate::app::App`] — the
/// picker runs before any session is loaded, so it has no state of its own
/// to render.
pub fn render_picker(frame: &mut Frame, sessions: &[SessionSummary], selected: usize) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" styra · choose a session · Enter open · q cancel ");

    if sessions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no sessions found",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = sessions.iter().map(session_item).collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected.min(sessions.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Render the top-level Workspace picker. Entering a Workspace leads to its
/// separate Session picker.
pub fn render_workspace_picker(
    frame: &mut Frame,
    workspaces: &[WorkspaceSummary],
    selected: usize,
) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" styra · choose a Workspace · Enter open · q cancel ");
    if workspaces.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no Workspaces found",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }
    let items: Vec<ListItem> = workspaces
        .iter()
        .map(|workspace| {
            let name = workspace.name.clone().unwrap_or_else(|| {
                workspace
                    .host_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workspace")
                    .to_owned()
            });
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{name:<20} "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "{:>3} sessions  {:<10} ",
                        workspace.session_count, workspace.age
                    ),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    workspace.host_path.display().to_string(),
                    Style::default().fg(Color::White),
                ),
            ]))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected.min(workspaces.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Render the current-interactions picker: interactions on the left and the selected
/// interaction's live diagnostic/stderr log on the right.
///
/// Like [`render_picker`], it stands apart from [`crate::app::App`] because it
/// overlays whichever session is currently loaded. The picker loop owns and
/// refreshes `updates`, while this function remains a pure renderer.
pub fn render_interactions_picker(
    frame: &mut Frame,
    interactions: &[InteractionSummary],
    selected: usize,
    updates: &[InteractionUpdate],
) {
    let area = frame.area();
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);
    let interactions_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" styra · current interactions · Enter attach · q cancel ");

    if interactions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no live interactions on the server",
            Style::default().fg(Color::Gray),
        )))
        .block(interactions_block);
        frame.render_widget(empty, panes[0]);
        render_interaction_log_preview(frame, None, updates, panes[1]);
        return;
    }

    let items: Vec<ListItem> = interactions.iter().map(interaction_item).collect();
    let list = List::new(items).block(interactions_block).highlight_style(
        Style::default()
            .bg(SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    let selected = selected.min(interactions.len() - 1);
    state.select(Some(selected));
    frame.render_stateful_widget(list, panes[0], &mut state);
    render_interaction_log_preview(frame, interactions.get(selected), updates, panes[1]);
}

fn render_interaction_log_preview(
    frame: &mut Frame,
    interaction: Option<&InteractionSummary>,
    updates: &[InteractionUpdate],
    area: Rect,
) {
    let title = interaction
        .map(|interaction| format!(" current log · {} ", interaction.id))
        .unwrap_or_else(|| " current log ".into());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title);

    if updates.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no log entries yet",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    // The preview follows the tail, combining decoded activity with Styra's
    // diagnostic/stderr log. Raw wire lines are filtered by the picker loop
    // because they duplicate decoded events in this compact view.
    let viewport = area.height.saturating_sub(2) as usize;
    let start = updates.len().saturating_sub(viewport);
    let lines: Vec<Line<'static>> = updates[start..]
        .iter()
        .map(|update| {
            interaction_preview_line(
                update,
                interaction.map(|item| item.selection.provider.protocol()),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn interaction_preview_line(
    update: &InteractionUpdate,
    protocol: Option<styra_server::event::Protocol>,
) -> Line<'static> {
    match update {
        InteractionUpdate::Event(event) => {
            let tag = event.tag();
            let display_tag = match event {
                styra_server::event::AgentEvent::UserMessage { .. } => "»",
                styra_server::event::AgentEvent::AgentMessage { .. } => "«",
                _ => tag,
            };
            let indent = if matches!(
                event,
                styra_server::event::AgentEvent::UserMessage { .. }
                    | styra_server::event::AgentEvent::AgentMessage { .. }
            ) {
                ""
            } else {
                "  "
            };
            Line::from(vec![
                Span::styled(
                    format!("{indent}{display_tag:<8} "),
                    Style::default()
                        .fg(tag_color(tag))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    protocol
                        .map(|protocol| {
                            protocol.presented_summary(
                                event,
                                styra_server::event::PresentationMode::Pretty,
                            )
                        })
                        .unwrap_or_else(|| event.summary()),
                    Style::default().fg(message_text_color(tag)),
                ),
            ])
        }
        InteractionUpdate::Log(entry) => log_line(entry),
        InteractionUpdate::Ended(end) => {
            let (message, color) = match (&end.error, end.exit_code) {
                (Some(error), _) => (format!("failed: {error}"), Color::Red),
                (None, Some(code)) => (format!("ended with exit code {code}"), Color::DarkGray),
                (None, None) => ("ended".into(), Color::DarkGray),
            };
            Line::from(Span::styled(message, Style::default().fg(color)))
        }
        InteractionUpdate::Raw(_) => Line::default(),
    }
}

fn interaction_item(interaction: &InteractionSummary) -> ListItem<'static> {
    let (label, color) = if interaction.accepting {
        ("live", Color::Green)
    } else {
        ("ended", Color::DarkGray)
    };
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{:<14} ", interaction.selection.provider.as_str()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{label:<6} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} · ", interaction.workspace_id),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(interaction.id.clone(), Style::default().fg(Color::White)),
    ]))
}

fn session_item(session: &SessionSummary) -> ListItem<'static> {
    let provider = session.selection.provider.as_str();
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{provider:<14} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<10} ", session.age),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(session.id.clone(), Style::default().fg(Color::White)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use styra_server::event::AgentEvent;

    fn picker_summary(id: &str, selection: &str, age: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            workspace_id: "w-1".into(),
            path: std::path::PathBuf::from(id),
            selection: styra_server::agent::Selection::parse(selection).unwrap(),
            age: age.into(),
            created_at_ms: None,
        }
    }

    fn rendered_picker(sessions: &[SessionSummary], selected: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, sessions, selected))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn picker_lists_sessions_with_provider_and_age() {
        let sessions = vec![
            picker_summary("s-1", "codex", "2m ago"),
            picker_summary("s-2", "claude", "3h ago"),
        ];
        let screen = rendered_picker(&sessions, 0);
        assert!(screen.contains("choose a session"));
        assert!(screen.contains("codex"));
        assert!(screen.contains("2m ago"));
        assert!(screen.contains("s-1"));
        assert!(screen.contains("claude"));
        assert!(screen.contains("3h ago"));
        assert!(screen.contains("s-2"));
    }

    #[test]
    fn picker_shows_a_placeholder_when_there_are_no_sessions() {
        let screen = rendered_picker(&[], 0);
        assert!(screen.contains("no sessions found"));
    }

    #[test]
    fn workspace_picker_lists_workspaces_before_sessions() {
        let workspaces = vec![WorkspaceSummary {
            id: "w-1".into(),
            name: Some("retry work".into()),
            host_path: PathBuf::from("/home/op/retry"),
            path: PathBuf::from("/state/workspaces/w-1"),
            session_count: 3,
            age: "2h ago".into(),
            created_at_ms: 1,
        }];
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
        terminal
            .draw(|frame| render_workspace_picker(frame, &workspaces, 0))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("choose a Workspace"), "{screen}");
        assert!(screen.contains("retry work"), "{screen}");
        assert!(screen.contains("3 sessions"), "{screen}");
        assert!(screen.contains("/home/op/retry"), "{screen}");
    }

    fn interaction_summary(id: &str, selection: &str, accepting: bool) -> InteractionSummary {
        InteractionSummary {
            id: id.into(),
            workspace_id: "w-1".into(),
            selection: styra_server::agent::Selection::parse(selection).unwrap(),
            workspace: std::path::PathBuf::from("/home/op/project"),
            driva: styra_server::DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec![selection.into()],
                working_directory: std::path::PathBuf::from("/tmp/styra/workspace"),
                network: false,
                mounts: Vec::new(),
            },
            accepting,
        }
    }

    fn rendered_interactions_picker(
        interactions: &[InteractionSummary],
        selected: usize,
        updates: &[InteractionUpdate],
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_interactions_picker(frame, interactions, selected, updates))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn interactions_picker_lists_interactions_with_provider_and_live_state() {
        let interactions = vec![
            interaction_summary("s-1", "codex", true),
            interaction_summary("s-2", "claude", false),
        ];
        let screen = rendered_interactions_picker(&interactions, 0, &[]);
        assert!(screen.contains("current interactions"));
        assert!(screen.contains("current log"));
        assert!(screen.contains("codex"));
        assert!(screen.contains("live"));
        assert!(screen.contains("s-1"));
        assert!(screen.contains("claude"));
        assert!(screen.contains("ended"));
        assert!(screen.contains("s-2"));
    }

    #[test]
    fn interactions_picker_shows_a_placeholder_when_there_are_no_live_interactions() {
        let screen = rendered_interactions_picker(&[], 0, &[]);
        assert!(screen.contains("no live interactions"));
    }

    #[test]
    fn interactions_picker_previews_the_selected_interactions_log() {
        let interactions = vec![
            interaction_summary("s-1", "codex", true),
            interaction_summary("s-2", "claude", true),
        ];
        let updates = vec![
            InteractionUpdate::Event(AgentEvent::CommandStarted {
                command: "cargo test".into(),
            }),
            InteractionUpdate::Log(styra_server::LogEntry::warn("waiting for response")),
            InteractionUpdate::Event(AgentEvent::AgentMessage {
                text: "Tests pass.".into(),
            }),
        ];

        let screen = rendered_interactions_picker(&interactions, 1, &updates);
        assert!(screen.contains("current log · s-2"));
        assert!(screen.contains("cargo test"));
        assert!(screen.contains("waiting for response"));
        assert!(screen.contains("Tests pass."));
    }

    #[test]
    fn picker_highlights_the_selected_session() {
        let sessions = vec![
            picker_summary("s-1", "codex", "2m ago"),
            picker_summary("s-2", "codex", "3h ago"),
        ];

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, &sessions, 1))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let row_containing = |text: &str| -> u16 {
            (0..buffer.area.height)
                .find(|&y| {
                    let row: String = (0..buffer.area.width)
                        .map(|x| buffer.cell((x, y)).unwrap().symbol())
                        .collect();
                    row.contains(text)
                })
                .unwrap_or_else(|| panic!("no row contains {text:?}"))
        };
        let row_has_gray_backdrop = |y: u16| {
            (0..buffer.area.width)
                .any(|x| buffer.cell((x, y)).unwrap().style().bg == Some(SELECTION_BG))
        };

        assert!(!row_has_gray_backdrop(row_containing("s-1")));
        assert!(row_has_gray_backdrop(row_containing("s-2")));
    }
}
