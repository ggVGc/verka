//! The session, Workspace, and current-interactions picker screens. These
//! stand apart from [`crate::app::App`] because each overlays before (or
//! instead of) any loaded session, so they render from their own borrowed
//! data rather than app state.

use super::notes::render_notes_pane;
use super::{
    log_line, message_text_color, render_placeholder, status_color, tag_color, SELECTION_BG,
    SELECTION_MARKER,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::notes;
use styra_server::{InteractionSummary, InteractionUpdate, SessionSummary, WorkspaceSummary};

/// Render the session picker screen: every stored session, newest first,
/// with `selected` highlighted. Standalone from [`crate::app::App`] — the
/// picker runs before any session is loaded, so it has no state of its own
/// to render.
pub fn render_picker(
    frame: &mut Frame,
    sessions: &[SessionSummary],
    selected: usize,
    updates: &[InteractionUpdate],
) {
    let area = frame.area();
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" styra · choose a session · Enter open · r rename · e Session notes · q cancel ");

    if sessions.is_empty() {
        render_placeholder(frame, block, panes[0], "  no sessions found");
        render_session_preview(frame, None, updates, panes[1]);
        return;
    }

    let selected = selected.min(sessions.len() - 1);
    let items: Vec<ListItem> = sessions
        .iter()
        .enumerate()
        .map(|(index, session)| session_item(session, index == selected))
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, panes[0], &mut state);
    let session = sessions.get(selected);
    render_session_preview(frame, session, updates, panes[1]);
}

fn render_session_preview(
    frame: &mut Frame,
    session: Option<&SessionSummary>,
    updates: &[InteractionUpdate],
    area: Rect,
) {
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(area);
    render_notes_pane(
        frame,
        notes::Scope::Session,
        session.map(|item| item.notes.as_str()),
        panes[0],
    );
    render_log_preview(
        frame,
        session.map(|item| item.id.as_str()),
        session.map(|item| item.selection.provider.protocol()),
        updates,
        panes[1],
    );
}

pub fn render_name_prompt(frame: &mut Frame, value: &str) {
    let area = frame.area();
    let width = area.width.saturating_sub(8).min(72);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + area.height.saturating_sub(3) / 2,
        width,
        3,
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(value.to_owned()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Session name · Enter save · Esc cancel "),
        ),
        popup,
    );
}

/// Render the top-level Workspace picker. Entering a Workspace leads to its
/// separate Session picker.
pub fn render_workspace_picker(
    frame: &mut Frame,
    workspaces: &[WorkspaceSummary],
    selected: usize,
) {
    let area = frame.area();
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(
            " styra · choose a Workspace · Enter open · e Workspace notes · c create · q cancel ",
        );
    if workspaces.is_empty() {
        render_placeholder(
            frame,
            block,
            panes[0],
            "  no Workspaces found · press c to create one in the current directory",
        );
        render_notes_pane(frame, notes::Scope::Workspace, None, panes[1]);
        return;
    }
    let items: Vec<ListItem> = workspaces
        .iter()
        .enumerate()
        .map(|(index, workspace)| {
            let name = crate::session::workspace_display_name(workspace);
            ListItem::new(Line::from(vec![
                Span::styled(
                    if index == selected.min(workspaces.len() - 1) {
                        "• "
                    } else {
                        "  "
                    },
                    Style::default().fg(if index == selected.min(workspaces.len() - 1) {
                        SELECTION_MARKER
                    } else {
                        Color::Cyan
                    }),
                ),
                Span::styled(
                    format!("{name:<19} "),
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
    frame.render_stateful_widget(list, panes[0], &mut state);
    render_notes_pane(
        frame,
        notes::Scope::Workspace,
        workspaces.get(selected).map(|item| item.notes.as_str()),
        panes[1],
    );
}

/// Render the Driva template picker: every template the Workspace could name,
/// with the chosen ones marked by the position they hold in the layering.
///
/// The number, rather than a plain check, is the point: templates are applied
/// in order and a later one wins on conflict, so which template is third
/// changes the resulting policy.
pub fn render_template_picker(
    frame: &mut Frame,
    templates: &[styra_server::TemplateSummary],
    chosen: &[String],
    cursor: usize,
) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" styra · Driva templates · Space toggle · Enter apply · q cancel ")
        .title_bottom(Line::from(Span::styled(
            " layered in the order chosen; a later template wins on conflict ",
            Style::default().fg(Color::Gray),
        )));
    if templates.is_empty() {
        render_placeholder(frame, block, area, "  no Driva templates are available");
        return;
    }
    let cursor = cursor.min(templates.len() - 1);
    let items: Vec<ListItem> = templates
        .iter()
        .enumerate()
        .map(|(index, template)| {
            let position = chosen.iter().position(|name| name == &template.name);
            let marker = match position {
                Some(order) => format!("{:>2} ", order + 1),
                None => "   ".to_owned(),
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    if index == cursor { "• " } else { "  " },
                    Style::default().fg(if index == cursor {
                        SELECTION_MARKER
                    } else {
                        Color::Cyan
                    }),
                ),
                Span::styled(
                    marker,
                    Style::default()
                        .fg(if position.is_some() {
                            Color::Yellow
                        } else {
                            Color::DarkGray
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<16} ", template.name),
                    Style::default()
                        .fg(if position.is_some() {
                            Color::Cyan
                        } else {
                            Color::Gray
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    template.description.clone(),
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
    state.select(Some(cursor));
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
    workspaces: &[WorkspaceSummary],
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
        .title(" styra · current interactions · Enter attach · d close · q cancel ");

    if interactions.is_empty() {
        render_placeholder(
            frame,
            interactions_block,
            panes[0],
            "  no live interactions on the server",
        );
        render_log_preview(frame, None, None, updates, panes[1]);
        return;
    }

    let items: Vec<ListItem> = interactions
        .iter()
        .enumerate()
        .map(|(index, interaction)| {
            interaction_item(
                interaction,
                workspaces,
                index == selected.min(interactions.len() - 1),
            )
        })
        .collect();
    let list = List::new(items).block(interactions_block).highlight_style(
        Style::default()
            .bg(SELECTION_BG)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    let selected = selected.min(interactions.len() - 1);
    state.select(Some(selected));
    frame.render_stateful_widget(list, panes[0], &mut state);
    let interaction = interactions.get(selected);
    render_log_preview(
        frame,
        interaction.map(|item| item.id.as_str()),
        interaction.map(|item| item.selection.provider.protocol()),
        updates,
        panes[1],
    );
}

fn render_log_preview(
    frame: &mut Frame,
    id: Option<&str>,
    protocol: Option<styra_server::event::Protocol>,
    updates: &[InteractionUpdate],
    area: Rect,
) {
    let title = id
        .map(|id| format!(" conversation · {id} "))
        .unwrap_or_else(|| " conversation ".into());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title);

    // The preview follows the tail of the conversation only — the same filter
    // the main list applies in conversation-only mode — so a glance down the
    // picker reads as an exchange rather than as tool traffic.
    let entries: Vec<&InteractionUpdate> = updates
        .iter()
        .filter(|update| is_interaction_entry(update))
        .collect();
    if entries.is_empty() {
        render_placeholder(frame, block, area, "  no messages yet");
        return;
    }
    let viewport = area.height.saturating_sub(2) as usize;
    let start = entries.len().saturating_sub(viewport);
    let lines: Vec<Line<'static>> = entries[start..]
        .iter()
        .map(|update| interaction_preview_line(update, protocol))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Whether an update is a conversational entry of the interaction: the same
/// filter the main list applies in conversation-only mode. Tool activity,
/// Styra-side diagnostics, and raw wire lines are left out of the previews.
fn is_interaction_entry(update: &InteractionUpdate) -> bool {
    matches!(
        update,
        InteractionUpdate::Event(
            styra_server::event::AgentEvent::UserMessage { .. }
                | styra_server::event::AgentEvent::AgentMessage { .. }
        )
    )
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
        InteractionUpdate::WorkingDirectoryChanged(directory) => Line::from(Span::styled(
            format!("working directory: {}", directory.display()),
            Style::default().fg(Color::Cyan),
        )),
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

fn interaction_item(
    interaction: &InteractionSummary,
    workspaces: &[WorkspaceSummary],
    selected: bool,
) -> ListItem<'static> {
    // Status colors come from the same table the main interaction view uses,
    // so a given state reads identically in both places.
    let status = if interaction.accepting {
        crate::app::Status::from(interaction.activity)
    } else {
        crate::app::Status::Ended {
            exit_code: None,
            error: None,
        }
    };
    let color = status_color(&status);
    let state = status.label();
    let workspace_name = workspaces
        .iter()
        .find(|workspace| workspace.id == interaction.workspace_id)
        .map(crate::session::workspace_display_name)
        .unwrap_or_else(|| interaction.workspace_id.clone());
    ListItem::new(Line::from(vec![
        Span::styled(
            if selected { "•" } else { " " },
            Style::default().fg(if selected {
                SELECTION_MARKER
            } else {
                Color::Cyan
            }),
        ),
        Span::styled(
            format!("{:<6} ", interaction.selection.provider.as_str()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{state:<8} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{workspace_name} · "),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            interaction
                .name
                .clone()
                .unwrap_or_else(|| interaction.id.clone()),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            interaction
                .name
                .as_ref()
                .map(|_| format!(" · {}", short_id(&interaction.id)))
                .unwrap_or_default(),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
}

fn session_item(session: &SessionSummary, selected: bool) -> ListItem<'static> {
    let provider = session.selection.provider.as_str();
    let display_name = session.name.as_deref().unwrap_or(&session.id);
    ListItem::new(vec![
        Line::from(vec![
            Span::styled(
                if selected { "• " } else { "  " },
                Style::default().fg(if selected {
                    SELECTION_MARKER
                } else {
                    Color::White
                }),
            ),
            Span::styled(
                display_name.to_owned(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{provider:<14} "), Style::default().fg(Color::Cyan)),
            Span::styled(session.age.clone(), Style::default().fg(Color::Gray)),
            Span::styled(
                session
                    .name
                    .as_ref()
                    .map(|_| format!(" · {}", short_id(&session.id)))
                    .unwrap_or_default(),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ])
}

fn short_id(id: &str) -> &str {
    id.get(id.len().saturating_sub(12)..).unwrap_or(id)
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
            name: None,
            notes: String::new(),
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
            .draw(|frame| render_picker(frame, sessions, selected, &[]))
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
    fn picker_prefers_a_session_name_but_retains_a_short_identity() {
        let mut session = picker_summary("0000000123456-42-7", "codex", "2m ago");
        session.name = Some("Fix session picker".into());
        let screen = rendered_picker(&[session], 0);
        assert!(screen.contains("Fix session picker"), "{screen}");
        assert!(screen.contains("123456-42-7"), "{screen}");
    }

    #[test]
    fn picker_shows_a_placeholder_when_there_are_no_sessions() {
        let screen = rendered_picker(&[], 0);
        assert!(screen.contains("no sessions found"));
    }

    #[test]
    fn session_picker_previews_the_selected_sessions_log() {
        let sessions = vec![picker_summary("s-1", "codex", "2m ago")];
        let updates = vec![
            InteractionUpdate::Event(AgentEvent::CommandStarted {
                command: "cargo test".into(),
            }),
            InteractionUpdate::Event(AgentEvent::AgentMessage {
                text: "Tests pass.".into(),
            }),
        ];
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, &sessions, 0, &updates))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(screen.contains("conversation · s-1"), "{screen}");
        assert!(screen.contains("Tests pass."), "{screen}");
        // Tool activity is not part of the conversation.
        assert!(!screen.contains("cargo test"), "{screen}");
    }

    #[test]
    fn workspace_picker_lists_workspaces_before_sessions() {
        let workspaces = vec![WorkspaceSummary {
            id: "w-1".into(),
            name: Some("retry work".into()),
            notes: String::new(),
            host_path: PathBuf::from("/home/op/retry"),
            path: PathBuf::from("/state/workspaces/w-1"),
            session_count: 3,
            age: "2h ago".into(),
            created_at_ms: 1,
            last_accessed_at_ms: 1,
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
            name: None,
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
            activity: styra_server::InteractionActivity::Pending,
        }
    }

    fn rendered_interactions_picker(
        interactions: &[InteractionSummary],
        workspaces: &[WorkspaceSummary],
        selected: usize,
        updates: &[InteractionUpdate],
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| {
                render_interactions_picker(frame, interactions, workspaces, selected, updates)
            })
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
    fn interactions_picker_lists_interactions_with_liveness_and_activity() {
        let mut running = interaction_summary("s-1", "codex", true);
        running.activity = styra_server::InteractionActivity::Running;
        let interactions = vec![running, interaction_summary("s-2", "claude", false)];
        let workspaces = vec![WorkspaceSummary {
            id: "w-1".into(),
            name: Some("payments".into()),
            notes: String::new(),
            host_path: PathBuf::from("/home/op/project"),
            path: PathBuf::from("/state/workspaces/w-1"),
            session_count: 2,
            age: "2m ago".into(),
            created_at_ms: 1,
            last_accessed_at_ms: 1,
        }];
        let screen = rendered_interactions_picker(&interactions, &workspaces, 0, &[]);
        assert!(screen.contains("current interactions"));
        assert!(screen.contains("conversation"));
        assert!(screen.contains("codex"));
        assert!(screen.contains("running"));
        assert!(screen.contains("s-1"));
        assert!(screen.contains("payments"));
        assert!(screen.contains("claude"));
        assert!(screen.contains("ended"));
        assert!(screen.contains("s-2"));

        let pending = rendered_interactions_picker(
            &[interaction_summary("s-3", "codex", true)],
            &workspaces,
            0,
            &[],
        );
        assert!(pending.contains("idle"));
    }

    #[test]
    fn interactions_picker_shows_a_placeholder_when_there_are_no_live_interactions() {
        let screen = rendered_interactions_picker(&[], &[], 0, &[]);
        assert!(screen.contains("no live interactions"));
    }

    #[test]
    fn preview_shows_a_placeholder_when_there_are_no_messages_yet() {
        let interactions = vec![interaction_summary("s-1", "codex", true)];
        let updates = vec![
            InteractionUpdate::Event(AgentEvent::CommandStarted {
                command: "cargo test".into(),
            }),
            InteractionUpdate::Log(styra_server::LogEntry::error("could not load current log")),
        ];
        let screen = rendered_interactions_picker(&interactions, &[], 0, &updates);
        assert!(screen.contains("no messages yet"), "{screen}");
        assert!(!screen.contains("could not load current log"), "{screen}");
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

        let screen = rendered_interactions_picker(&interactions, &[], 1, &updates);
        assert!(screen.contains("conversation · s-2"));
        assert!(screen.contains("Tests pass."));
        // Tool activity and Styra's own log entries are not conversation.
        assert!(!screen.contains("cargo test"));
        assert!(!screen.contains("waiting for response"));
    }

    #[test]
    fn picker_highlights_the_selected_session() {
        let sessions = vec![
            picker_summary("s-1", "codex", "2m ago"),
            picker_summary("s-2", "codex", "3h ago"),
        ];

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, &sessions, 1, &[]))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        // Scan only the session pane: the log-preview pane's title spells out
        // the selected session's id too, so searching the full row width would
        // match that border row instead of the list row it is describing.
        let session_pane_width = buffer.area.width * 42 / 100;
        let row_containing = |text: &str| -> u16 {
            (0..buffer.area.height)
                .find(|&y| {
                    let row: String = (0..session_pane_width)
                        .map(|x| buffer.cell((x, y)).unwrap().symbol())
                        .collect();
                    row.contains(text)
                })
                .unwrap_or_else(|| panic!("no row contains {text:?}"))
        };
        let row_has_selection_backdrop = |y: u16| {
            (0..buffer.area.width)
                .any(|x| buffer.cell((x, y)).unwrap().style().bg == Some(SELECTION_BG))
        };

        assert!(!row_has_selection_backdrop(row_containing("s-1")));
        assert!(row_has_selection_backdrop(row_containing("s-2")));
    }
}
