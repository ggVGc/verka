//! The session and Workspace picker screens. These stand apart from
//! [`crate::app::App`] because each overlays before (or instead of) any loaded
//! session, so they render from their own borrowed data rather than app state.

use super::{message_text_color, palette, render_placeholder, tag_color};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use styra_server::{InteractionSummary, InteractionUpdate, SessionSummary, WorkspaceSummary};

/// Whether the picker has the selected session's conversation yet. Loading is
/// a round-trip to the server, so "nothing to show" and "nothing yet" are
/// genuinely different states and must not read the same.
#[derive(Debug, Clone, Copy)]
pub enum Preview<'a> {
    Loading,
    Ready(&'a [InteractionUpdate]),
}

/// Render the session picker screen: every stored session, newest first,
/// with `selected` highlighted. Standalone from [`crate::app::App`] — the
/// picker runs before any session is loaded, so it has no state of its own
/// to render.
pub fn render_picker(
    frame: &mut Frame,
    sessions: &[SessionSummary],
    selected: usize,
    preview: Preview<'_>,
) {
    let area = frame.area();
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(
            " styra · choose a session · Enter open · r rename · x convert provider · q cancel ",
        );

    if sessions.is_empty() {
        render_placeholder(frame, block, panes[0], "  no sessions found");
        render_session_preview(frame, None, preview, panes[1]);
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
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, panes[0], &mut state);
    let session = sessions.get(selected);
    render_session_preview(frame, session, preview, panes[1]);
}

fn render_session_preview(
    frame: &mut Frame,
    session: Option<&SessionSummary>,
    preview: Preview<'_>,
    area: Rect,
) {
    render_session_log_preview(
        frame,
        session.map(|item| item.id.as_str()),
        session.map(|item| item.selection.provider.protocol()),
        preview,
        area,
    );
}

/// Render the conversation preview retained by the stored-Session picker.
/// Live-interaction navigation uses the main event list directly instead.
fn render_session_log_preview(
    frame: &mut Frame,
    id: Option<&str>,
    protocol: Option<styra_server::event::Protocol>,
    preview: Preview<'_>,
    area: Rect,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(
            id.map(|id| format!(" conversation · {id} "))
                .unwrap_or_else(|| " conversation ".into()),
        );
    let updates = match preview {
        Preview::Loading => {
            render_placeholder(frame, block, area, "  loading…");
            return;
        }
        Preview::Ready(updates) => updates,
    };
    let viewport = area.height.saturating_sub(2) as usize;
    let entries = updates
        .iter()
        .rev()
        .filter_map(|update| match update {
            InteractionUpdate::Event(
                event @ (styra_server::event::AgentEvent::UserMessage { .. }
                | styra_server::event::AgentEvent::AgentMessage { .. }),
            ) => Some(event),
            _ => None,
        })
        .take(viewport)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        render_placeholder(frame, block, area, "  no messages yet");
        return;
    }
    let lines = entries
        .into_iter()
        .rev()
        .map(|event| {
            let tag = event.tag();
            let marker = match event {
                styra_server::event::AgentEvent::UserMessage { .. } => "»",
                _ => "«",
            };
            Line::from(vec![
                Span::styled(
                    format!("{marker:<8} "),
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
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// A dismissable one-line notice (e.g. a conversion failure), overlaid on top
/// of the session picker it interrupted.
pub fn render_message_popup(frame: &mut Frame, title: &str, message: &str) {
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
        Paragraph::new(message.to_owned()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(palette::ERROR))
                .title(format!(" {title} · press any key ")),
        ),
        popup,
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
                .border_style(Style::default().fg(palette::ACCENT))
                .title(" Session name · Enter save · Esc cancel "),
        ),
        popup,
    );
}

/// Whether the picker has the selected Workspace's Session list yet. Loading
/// is a round-trip to the server, so an unread Workspace and an empty one
/// must not read the same.
#[derive(Debug, Clone, Copy)]
pub enum SessionsPreview<'a> {
    Loading,
    Ready(&'a [SessionSummary]),
}

/// Render the top-level Workspace picker. Entering a Workspace leads to its
/// separate Session picker, so the right-hand pane previews that next screen
/// for the row under the cursor: the Workspace's Sessions.
pub fn render_workspace_picker(
    frame: &mut Frame,
    workspaces: &[WorkspaceSummary],
    selected: usize,
    interactions: &[InteractionSummary],
    preview: SessionsPreview<'_>,
) {
    let area = frame.area();
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::ACCENT))
        .title(
            " styra \u{b7} choose a Workspace \u{b7} Enter open \u{b7} c create \u{b7} q cancel ",
        );
    if workspaces.is_empty() {
        render_placeholder(
            frame,
            block,
            panes[0],
            "  no Workspaces found \u{b7} press c to create one in the current directory",
        );
        render_sessions_preview(frame, None, preview, interactions, panes[1]);
        return;
    }
    let selected = selected.min(workspaces.len() - 1);
    let items: Vec<ListItem> = workspaces
        .iter()
        .enumerate()
        .map(|(index, workspace)| workspace_item(workspace, index == selected, interactions))
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, panes[0], &mut state);
    let workspace = workspaces.get(selected);
    render_sessions_preview(frame, workspace, preview, interactions, panes[1]);
}

/// One Workspace row. A Workspace holding an Interaction the server still
/// accepts input for is marked and counted: that is where the operator has
/// work in flight, and it is why the row sorts where it does.
fn workspace_item(
    workspace: &WorkspaceSummary,
    selected: bool,
    interactions: &[InteractionSummary],
) -> ListItem<'static> {
    let name = crate::workspace::display_name(workspace);
    let live = live_interactions(&workspace.id, interactions);
    ListItem::new(Line::from(vec![
        Span::styled(
            if selected { "\u{2022} " } else { "  " },
            Style::default().fg(if selected {
                palette::SELECTION_MARKER
            } else {
                palette::ACCENT
            }),
        ),
        Span::styled(
            format!("{name:<19} "),
            Style::default()
                .fg(palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>3} sessions  ", workspace.session_count),
            Style::default().fg(palette::MUTED_TEXT),
        ),
        Span::styled(
            format!("{:<9}", live_label(live)),
            Style::default()
                .fg(palette::LIVE_MARKER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<10} ", workspace.age),
            Style::default().fg(palette::MUTED_TEXT),
        ),
        Span::styled(
            workspace.host_path.display().to_string(),
            Style::default().fg(palette::TEXT),
        ),
    ]))
}

/// The liveness column: a dot and a count, or nothing at all. Dot and count
/// share one column so a Workspace with no work in flight costs the row only
/// the blank width, leaving the host path room to be read.
fn live_label(live: usize) -> String {
    match live {
        0 => String::new(),
        n => format!("\u{25cf} {n} live"),
    }
}

/// Interactions in a Workspace the server still accepts input for, whether
/// idle and waiting on the operator or busy with a turn.
fn live_interactions(workspace_id: &str, interactions: &[InteractionSummary]) -> usize {
    interactions
        .iter()
        .filter(|interaction| interaction.accepting && interaction.workspace_id == workspace_id)
        .count()
}

/// The Session-picker preview: the Sessions `Enter` would open this Workspace
/// on, in the same order that screen shows them, with the live ones marked.
fn render_sessions_preview(
    frame: &mut Frame,
    workspace: Option<&WorkspaceSummary>,
    preview: SessionsPreview<'_>,
    interactions: &[InteractionSummary],
    area: Rect,
) {
    let title = match workspace {
        Some(workspace) => format!(
            " sessions \u{b7} {} ",
            crate::workspace::display_name(workspace)
        ),
        None => " sessions ".to_owned(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(title);
    let sessions = match preview {
        SessionsPreview::Loading => {
            render_placeholder(frame, block, area, "  loading\u{2026}");
            return;
        }
        SessionsPreview::Ready(sessions) => sessions,
    };
    if sessions.is_empty() {
        render_placeholder(frame, block, area, "  no sessions yet");
        return;
    }
    let items: Vec<ListItem> = sessions
        .iter()
        .map(|session| {
            let live = interactions
                .iter()
                .any(|interaction| interaction.accepting && interaction.id == session.id);
            preview_session_item(session, live)
        })
        .collect();
    frame.render_widget(List::new(items).block(block), area);
}

/// A Session as the preview shows it: one line, since this pane stands beside
/// the Workspace list rather than replacing it, and the full two-line form is
/// what the Session picker itself draws once the Workspace is open.
fn preview_session_item(session: &SessionSummary, live: bool) -> ListItem<'static> {
    let display_name = session
        .name
        .as_deref()
        .unwrap_or_else(|| short_id(&session.id));
    ListItem::new(Line::from(vec![
        Span::styled(
            if live { "\u{25cf} " } else { "  " },
            Style::default().fg(palette::LIVE_MARKER),
        ),
        Span::styled(
            format!("{:<8} ", session.selection.provider.as_str()),
            Style::default().fg(palette::ACCENT),
        ),
        Span::styled(
            format!("{display_name:<20} "),
            Style::default().fg(palette::TEXT),
        ),
        Span::styled(
            session.age.clone(),
            Style::default().fg(palette::MUTED_TEXT),
        ),
    ]))
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
        .border_style(Style::default().fg(palette::ACCENT))
        .title(" styra · Driva templates · Space toggle · Enter apply · q cancel ")
        .title_bottom(Line::from(Span::styled(
            " layered in the order chosen; a later template wins on conflict ",
            Style::default().fg(palette::MUTED_TEXT),
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
                        palette::SELECTION_MARKER
                    } else {
                        palette::ACCENT
                    }),
                ),
                Span::styled(
                    marker,
                    Style::default()
                        .fg(if position.is_some() {
                            palette::WARNING
                        } else {
                            palette::INACTIVE
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<16} ", template.name),
                    Style::default()
                        .fg(if position.is_some() {
                            palette::ACCENT
                        } else {
                            palette::MUTED_TEXT
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    template.description.clone(),
                    Style::default().fg(palette::TEXT),
                ),
            ]))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

fn session_item(session: &SessionSummary, selected: bool) -> ListItem<'static> {
    let provider = session.selection.provider.as_str();
    let display_name = session.name.as_deref().unwrap_or(&session.id);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                if selected { "• " } else { "  " },
                Style::default().fg(if selected {
                    palette::SELECTION_MARKER
                } else {
                    palette::TEXT
                }),
            ),
            Span::styled(
                display_name.to_owned(),
                Style::default()
                    .fg(palette::TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{provider:<14} "),
                Style::default().fg(palette::ACCENT),
            ),
            Span::styled(
                session.age.clone(),
                Style::default().fg(palette::MUTED_TEXT),
            ),
            Span::styled(
                session
                    .name
                    .as_ref()
                    .map(|_| format!(" · {}", short_id(&session.id)))
                    .unwrap_or_default(),
                Style::default().fg(palette::ADDITIONAL_INFO),
            ),
        ]),
    ];
    if let Some(origin) = &session.origin {
        let kind = if origin.provider == session.selection.provider {
            "branched"
        } else {
            "converted"
        };
        lines.push(Line::from(Span::styled(
            format!(
                "  ⤷ {kind} from {} ({})",
                short_id(&origin.session_id),
                origin.provider.as_str()
            ),
            Style::default().fg(palette::ADDITIONAL_INFO),
        )));
    }
    ListItem::new(lines)
}

pub(crate) fn short_id(id: &str) -> &str {
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
            workspace_id: "w-1".into(),
            path: std::path::PathBuf::from(id),
            selection: styra_server::agent::Selection::parse(selection).unwrap(),
            age: age.into(),
            created_at_ms: None,
            origin: None,
        }
    }

    fn rendered_picker(sessions: &[SessionSummary], selected: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, sessions, selected, Preview::Ready(&[])))
            .unwrap();
        screen_text(terminal.backend().buffer())
    }

    /// Split a flattened screen back into rows, so an assertion can say which
    /// row a marker landed on rather than only that it is somewhere on screen.
    fn screen_lines(screen: &str, width: usize) -> Vec<String> {
        screen
            .chars()
            .collect::<Vec<_>>()
            .chunks(width)
            .map(|row| row.iter().collect())
            .collect()
    }

    fn screen_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer
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
    fn a_branched_session_shows_where_it_came_from() {
        let mut session = picker_summary("s-2", "claude", "2m ago");
        session.origin = Some(styra_server::SessionOrigin {
            session_id: "s-1".into(),
            provider: styra_server::agent::Provider::Codex,
            at_ms: None,
        });
        let screen = rendered_picker(&[session], 0);
        assert!(screen.contains("converted from"), "{screen}");
        assert!(screen.contains("s-1"), "{screen}");
        assert!(screen.contains("codex"), "{screen}");

        let mut checkpoint = picker_summary("s-3", "codex", "2m ago");
        checkpoint.origin = Some(styra_server::SessionOrigin {
            session_id: "s-1".into(),
            provider: styra_server::agent::Provider::Codex,
            at_ms: Some(1000),
        });
        let screen = rendered_picker(&[checkpoint], 0);
        assert!(screen.contains("branched from"), "{screen}");
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
            .draw(|frame| render_picker(frame, &sessions, 0, Preview::Ready(&updates)))
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

    fn workspace_summary(id: &str, name: &str, session_count: usize) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: Some(name.into()),
            host_path: PathBuf::from(format!("/home/op/{id}")),
            git_repository: None,
            worktrees_enabled: false,
            path: PathBuf::from(format!("/state/workspaces/{id}")),
            session_count,
            age: "2h ago".into(),
            created_at_ms: 1,
            last_accessed_at_ms: 1,
            launch: Default::default(),
        }
    }

    /// Wide enough that the left pane holds a whole row — name, counts,
    /// liveness, age, host path — so an assertion about the row is not really
    /// an assertion about where it was truncated.
    const WORKSPACE_PICKER_WIDTH: usize = 130;

    /// The Workspace list occupies the left 58% of the picker. The preview on
    /// the right carries the selected Workspace's name in its own border title,
    /// so a row has to be matched against this pane alone to be the list's.
    fn workspace_rows(screen: &str) -> Vec<String> {
        let width = WORKSPACE_PICKER_WIDTH * 58 / 100;
        screen_lines(screen, WORKSPACE_PICKER_WIDTH)
            .into_iter()
            .map(|line| line.chars().take(width).collect())
            .collect()
    }

    fn rendered_workspace_picker(
        workspaces: &[WorkspaceSummary],
        selected: usize,
        interactions: &[InteractionSummary],
        preview: SessionsPreview<'_>,
    ) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(WORKSPACE_PICKER_WIDTH as u16, 14)).unwrap();
        terminal
            .draw(|frame| {
                render_workspace_picker(frame, workspaces, selected, interactions, preview)
            })
            .unwrap();
        screen_text(terminal.backend().buffer())
    }

    #[test]
    fn workspace_picker_lists_workspaces_before_sessions() {
        let mut workspace = workspace_summary("retry", "retry work", 3);
        workspace.host_path = PathBuf::from("/home/op/retry");
        let screen = rendered_workspace_picker(&[workspace], 0, &[], SessionsPreview::Ready(&[]));
        assert!(screen.contains("choose a Workspace"), "{screen}");
        assert!(screen.contains("retry work"), "{screen}");
        assert!(screen.contains("3 sessions"), "{screen}");
        assert!(screen.contains("/home/op/retry"), "{screen}");
    }

    #[test]
    fn workspace_picker_marks_workspaces_holding_a_live_interaction() {
        let workspaces = vec![
            workspace_summary("w-1", "payments", 2),
            workspace_summary("w-2", "quiet", 1),
        ];
        let interactions = vec![
            interaction_summary("s-1", "codex", true),
            interaction_summary("s-2", "claude", true),
            // Stopped: the server no longer accepts input, so it is not work
            // in flight and must not mark the Workspace.
            InteractionSummary {
                workspace_id: "w-2".into(),
                ..interaction_summary("s-3", "codex", false)
            },
        ];

        let screen =
            rendered_workspace_picker(&workspaces, 0, &interactions, SessionsPreview::Ready(&[]));

        let payments = workspace_rows(&screen)
            .into_iter()
            .find(|line| line.contains("payments"))
            .unwrap();
        let quiet = workspace_rows(&screen)
            .into_iter()
            .find(|line| line.contains("quiet"))
            .unwrap();
        assert!(payments.contains('\u{25cf}'), "{payments}");
        assert!(payments.contains("2 live"), "{payments}");
        assert!(!quiet.contains('\u{25cf}'), "{quiet}");
        assert!(!quiet.contains("live"), "{quiet}");
    }

    #[test]
    fn workspace_picker_previews_the_selected_workspaces_sessions() {
        let workspaces = vec![
            workspace_summary("w-1", "payments", 2),
            workspace_summary("w-2", "quiet", 0),
        ];
        let mut named = picker_summary("s-1", "codex", "2m ago");
        named.name = Some("retry backoff".into());
        let sessions = vec![named, picker_summary("s-9", "claude", "1h ago")];

        let screen = rendered_workspace_picker(
            &workspaces,
            0,
            &[interaction_summary("s-1", "codex", true)],
            SessionsPreview::Ready(&sessions),
        );

        assert!(screen.contains("sessions \u{b7} payments"), "{screen}");
        assert!(screen.contains("retry backoff"), "{screen}");
        assert!(screen.contains("2m ago"), "{screen}");
        assert!(screen.contains("1h ago"), "{screen}");
        // The live Session carries the same dot its Workspace row does.
        let live_row = screen_lines(&screen, WORKSPACE_PICKER_WIDTH)
            .into_iter()
            .find(|line| line.contains("retry backoff"))
            .unwrap();
        assert!(live_row.contains('\u{25cf}'), "{live_row}");
    }

    #[test]
    fn an_unloaded_workspace_preview_reads_as_loading_rather_than_as_empty() {
        let workspaces = vec![workspace_summary("w-1", "payments", 2)];

        let loading = rendered_workspace_picker(&workspaces, 0, &[], SessionsPreview::Loading);
        assert!(loading.contains("loading"), "{loading}");

        let empty = rendered_workspace_picker(&workspaces, 0, &[], SessionsPreview::Ready(&[]));
        assert!(empty.contains("no sessions yet"), "{empty}");
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
            last_message: None,
        }
    }

    #[test]
    fn picker_highlights_the_selected_session() {
        let sessions = vec![
            picker_summary("s-1", "codex", "2m ago"),
            picker_summary("s-2", "codex", "3h ago"),
        ];

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, &sessions, 1, Preview::Ready(&[])))
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
            (0..buffer.area.width).any(|x| {
                buffer.cell((x, y)).unwrap().style().bg == Some(palette::SELECTION_BACKGROUND)
            })
        };

        assert!(!row_has_selection_backdrop(row_containing("s-1")));
        assert!(row_has_selection_backdrop(row_containing("s-2")));
    }
}
