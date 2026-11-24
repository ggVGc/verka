use super::*;
use crate::text_prompt::{self, TextPrompt};
use crate::theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::path::Path;

fn session_tree_depths(sessions: &[SessionSummary]) -> Vec<usize> {
    use std::collections::HashMap;
    let positions = sessions
        .iter()
        .enumerate()
        .map(|(index, session)| (session.id.as_str(), index))
        .collect::<HashMap<_, _>>();
    sessions
        .iter()
        .enumerate()
        .map(|(index, session)| {
            let mut depth = 0;
            let mut seen = vec![index];
            let mut parent = session
                .origin
                .as_ref()
                .and_then(|origin| positions.get(origin.session_id.as_str()).copied());
            while let Some(parent_index) = parent {
                if seen.contains(&parent_index) {
                    return 0;
                }
                seen.push(parent_index);
                depth += 1;
                parent = sessions[parent_index]
                    .origin
                    .as_ref()
                    .and_then(|origin| positions.get(origin.session_id.as_str()).copied());
            }
            depth
        })
        .collect()
}

fn render_placeholder(frame: &mut Frame, block: Block<'static>, area: Rect, text: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_owned(),
            Style::default().fg(theme::MUTED_TEXT),
        )))
        .block(block),
        area,
    );
}

fn tag_color(tag: &str) -> ratatui::style::Color {
    match tag {
        "agent" => theme::AGENT_TAG,
        "user" => theme::USER_TAG,
        "shell" => theme::SHELL_TAG,
        "tool" => theme::SPECIAL,
        "plan" | "files" => theme::INFO,
        "branch" => theme::ACCENT,
        "error" | "malformed" => theme::ERROR,
        _ => theme::ADDITIONAL_INFO,
    }
}

fn message_text_color(tag: &str) -> ratatui::style::Color {
    match tag {
        "agent" => theme::AGENT_TEXT,
        "user" => theme::USER_TEXT,
        "error" | "malformed" => theme::ERROR,
        _ => theme::TEXT,
    }
}

fn workspace_display_name(workspace: &WorkspaceSummary) -> String {
    workspace.name.clone().unwrap_or_else(|| {
        workspace
            .host_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_owned()
    })
}
use styra_protocol::{InteractionSummary, InteractionUpdate, SessionSummary, WorkspaceSummary};

/// Render the session picker screen: every stored session in `order`, newest
/// first, with `selected` highlighted. Each row shows the age `order` sorts
/// on, so the list never looks unsorted. Standalone from [`crate::app::App`] — the
/// picker runs before any session is loaded, so it has no state of its own
/// to render.
// Picker rendering keeps its borrowed display state explicit, like render_workspace_picker.
#[allow(clippy::too_many_arguments)]
pub fn render_picker(
    frame: &mut Frame,
    scope: &str,
    sessions: &[SessionSummary],
    selected: usize,
    order: SessionOrder,
    preview: Preview<'_>,
    filter: Option<&str>,
    searching: bool,
    show_completed: bool,
) {
    let area = frame.area();
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(session_picker_title(
            scope,
            order,
            filter,
            searching,
            show_completed,
        ));

    if sessions.is_empty() {
        render_placeholder(frame, block, panes[0], "  no sessions found");
        render_session_preview(frame, None, preview, panes[1]);
        return;
    }

    let selected = selected.min(sessions.len() - 1);
    let depths = session_tree_depths(sessions);
    let items: Vec<ListItem> = sessions
        .iter()
        .zip(depths)
        .enumerate()
        .map(|(index, (session, depth))| session_item(session, depth, index == selected, order))
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, panes[0], &mut state);
    let session = sessions.get(selected);
    render_session_preview(frame, session, preview, panes[1]);
}

fn session_picker_title(
    scope: &str,
    order: SessionOrder,
    filter: Option<&str>,
    searching: bool,
    show_completed: bool,
) -> String {
    let filter = filter
        .filter(|filter| !filter.is_empty())
        .map(|filter| format!(" · filter: {filter}"))
        .unwrap_or_default();
    let searching = if searching { " · searching" } else { "" };
    let completed = if show_completed {
        "completed shown"
    } else {
        "completed hidden"
    };
    // The shortcuts live behind `?` rather than along this line: only a few
    // ever fit, and the ones that did crowded out the state — the filter,
    // which way the list is sorted, and whether completed sessions are shown
    // — that the operator cannot get anywhere else.
    format!(
        " {scope} · sessions{filter}{searching} · sort: {} · {completed} · ? keys ",
        order.label(),
    )
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
    protocol: Option<styra_protocol::event::Protocol>,
    preview: Preview<'_>,
    area: Rect,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::INACTIVE))
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
                event @ (styra_protocol::event::AgentEvent::UserMessage { .. }
                | styra_protocol::event::AgentEvent::AgentMessage { .. }),
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
        .filter_map(|event| conversation_line(event, protocol))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The last `count` messages of a conversation, oldest first, one line each,
/// or a line saying there are none yet.
pub fn latest_conversation<'e>(
    events: impl DoubleEndedIterator<Item = &'e styra_protocol::event::AgentEvent>,
    protocol: styra_protocol::event::Protocol,
    count: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<_> = events
        .rev()
        .filter_map(|event| conversation_line(event, Some(protocol)))
        .take(count)
        .collect();
    if lines.is_empty() {
        return vec![Line::from(Span::styled(
            "  no messages yet",
            Style::default().fg(theme::MUTED_TEXT),
        ))];
    }
    lines.reverse();
    lines
}

/// One message of another Session's conversation, as a single line: who
/// spoke, and the gist of what they said. `None` for anything that is not a
/// message — a conversation previewed from outside is read by its exchange.
pub(crate) fn conversation_line(
    event: &styra_protocol::event::AgentEvent,
    protocol: Option<styra_protocol::event::Protocol>,
) -> Option<Line<'static>> {
    let marker = match event {
        styra_protocol::event::AgentEvent::UserMessage { .. } => "»",
        styra_protocol::event::AgentEvent::AgentMessage { .. } => "«",
        _ => return None,
    };
    let tag = event.tag();
    Some(Line::from(vec![
        Span::styled(
            format!("{marker:<8} "),
            Style::default()
                .fg(tag_color(tag))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            protocol
                .map(|protocol| {
                    protocol
                        .presented_summary(event, styra_protocol::event::PresentationMode::Pretty)
                })
                .unwrap_or_else(|| event.summary()),
            Style::default().fg(message_text_color(tag)),
        ),
    ]))
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
        Paragraph::new(message.to_owned())
            .style(Style::default().fg(theme::ERROR))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::ERROR))
                    .title(format!(" {title} · press any key ")),
            ),
        popup,
    );
}

pub fn render_name_prompt(frame: &mut Frame, value: &str) {
    let prompt = TextPrompt::new(" Session name · Enter save · Esc cancel ", value);
    text_prompt::render(frame, &prompt, frame.area());
}

/// A one-line text field over the Workspace picker: its name, or its directory.
pub fn render_workspace_prompt(frame: &mut Frame, title: &str, value: &str) {
    let prompt = TextPrompt::new(format!(" {title} · Enter save · Esc cancel "), value);
    text_prompt::render(frame, &prompt, frame.area());
}

/// Render the top-level Workspace picker. Entering a Workspace leads to its
/// separate Session picker, so the right-hand pane previews that next screen
/// for the row under the cursor: the Workspace's Sessions.
#[allow(clippy::too_many_arguments)]
pub fn render_workspace_picker(
    frame: &mut Frame,
    workspaces: &[WorkspaceSummary],
    selected: usize,
    interactions: &[InteractionSummary],
    preview: SessionsPreview<'_>,
    filter: Option<&str>,
    show_deleted: bool,
) {
    let panes = workspace_picker_panes(frame.area());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(workspace_picker_title(filter, show_deleted));
    if workspaces.is_empty() {
        let empty = if filter.is_some_and(|filter| !filter.is_empty()) {
            "  no Workspaces match the filter \u{b7} Esc clears it"
        } else if show_deleted {
            "  no deleted Workspaces · Ctrl+T returns to active Workspaces"
        } else {
            "  no Workspaces found \u{b7} press Ctrl+C to create one in the current directory"
        };
        render_placeholder(frame, block, panes[0], empty);
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
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, panes[0], &mut state);
    let workspace = workspaces.get(selected);
    if show_deleted {
        render_placeholder(
            frame,
            Block::default()
                .borders(Borders::ALL)
                .title(" Preserved Sessions "),
            panes[1],
            "  Restore this Workspace with Ctrl+U to access its Sessions.",
        );
    } else {
        render_sessions_preview(frame, workspace, preview, interactions, panes[1]);
    }
}

/// The Workspace picker's two panes: the list on the left, the selected
/// Workspace's Sessions on the right.
fn workspace_picker_panes(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area)
}

/// The question Styra asks when started in a directory no Workspace covers,
/// overlaid on the Workspace list it falls back to if the answer is no. The
/// screen beneath is washed down as it is behind the message box, so the
/// question is plainly what has the keys.
pub fn render_workspace_offer(frame: &mut Frame, directory: &Path) {
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(theme::MODAL_BACKDROP)
                .add_modifier(Modifier::DIM),
        ),
        frame.area(),
    );
    let area = workspace_picker_panes(frame.area())[0];
    let width = area.width.saturating_sub(4).min(72);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        // Pinned near the top, where the eye starts, however tall the screen:
        // clear of the list's title, with a row of air above it.
        area.y + 2.min(area.height.saturating_sub(5)),
        width,
        5.min(area.height),
    );
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Create a Workspace for this directory?",
                Style::default().fg(theme::TEXT),
            )),
            Line::from(Span::styled(
                directory.display().to_string(),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "Enter/y create \u{b7} Esc/n pick an existing one",
                Style::default().fg(theme::MUTED_TEXT),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::ACCENT))
                .title(" No Workspace here "),
        ),
        popup,
    );
}

/// The Workspace picker's title says what the list cannot be read without:
/// the filter narrowing it. Every letter typed is part of that filter, so the
/// rest of the keys live behind `?`, as they do for the session picker.
fn workspace_picker_title(filter: Option<&str>, show_deleted: bool) -> String {
    let filter = filter
        .filter(|filter| !filter.is_empty())
        .map(|filter| format!(" \u{b7} filter: {filter}"))
        .unwrap_or_default();
    let scope = if show_deleted {
        "Deleted Workspaces"
    } else {
        "Workspaces"
    };
    let action = if show_deleted {
        "Ctrl+U restore"
    } else {
        "Ctrl+X delete"
    };
    format!(" styra \u{b7} {scope}{filter} \u{b7} type to filter \u{b7} Ctrl+T deleted \u{b7} {action} \u{b7} ? keys ")
}

/// One Workspace row. A Workspace holding an Interaction the server still
/// accepts input for is marked and counted: that is where the operator has
/// work in flight, and it is why the row sorts where it does.
fn workspace_item(
    workspace: &WorkspaceSummary,
    selected: bool,
    interactions: &[InteractionSummary],
) -> ListItem<'static> {
    let name = workspace_display_name(workspace);
    let live = live_interactions(&workspace.id, interactions);
    ListItem::new(Line::from(vec![
        Span::styled(
            if selected { "\u{2022} " } else { "  " },
            Style::default().fg(if selected {
                theme::SELECTION_MARKER
            } else {
                theme::ACCENT
            }),
        ),
        Span::styled(
            format!("{name:<19} "),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:>3} sessions  ", workspace.session_count),
            Style::default().fg(theme::MUTED_TEXT),
        ),
        Span::styled(
            format!("{:<9}", live_label(live)),
            Style::default()
                .fg(theme::LIVE_MARKER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<10} ", workspace.age),
            Style::default().fg(theme::MUTED_TEXT),
        ),
        Span::styled(
            workspace.host_path.display().to_string(),
            Style::default().fg(theme::TEXT),
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

/// Interactions in a Workspace that are still open work: idle and waiting on
/// the operator, busy with a turn, or dormant until the next message.
fn live_interactions(workspace_id: &str, interactions: &[InteractionSummary]) -> usize {
    interactions
        .iter()
        .filter(|interaction| interaction.open() && interaction.workspace_id == workspace_id)
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
        Some(workspace) => format!(" sessions \u{b7} {} ", workspace_display_name(workspace)),
        None => " sessions ".to_owned(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::INACTIVE))
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
                .any(|interaction| interaction.open() && interaction.id == session.id);
            preview_session_item(session, live)
        })
        .collect();
    frame.render_widget(List::new(items).block(block), area);
}

/// A Session as the preview shows it: one line, matching the compact tree in
/// the full Session picker.
fn preview_session_item(session: &SessionSummary, live: bool) -> ListItem<'static> {
    let display_name = session
        .name
        .as_deref()
        .unwrap_or_else(|| short_id(&session.id));
    ListItem::new(Line::from(vec![
        Span::styled(
            if live { "\u{25cf} " } else { "  " },
            Style::default().fg(theme::LIVE_MARKER),
        ),
        Span::styled(
            format!("{:<8} ", session.selection.provider.as_str()),
            Style::default().fg(theme::ACCENT),
        ),
        Span::styled(
            format!("{display_name:<20} "),
            Style::default().fg(theme::TEXT),
        ),
        Span::styled(session.age.clone(), Style::default().fg(theme::MUTED_TEXT)),
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
    templates: &[styra_protocol::TemplateSummary],
    chosen: &[String],
    cursor: usize,
) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(" styra · Driva templates · ? keys ")
        .title_bottom(Line::from(Span::styled(
            " layered in the order chosen; a later template wins on conflict ",
            Style::default().fg(theme::MUTED_TEXT),
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
                        theme::SELECTION_MARKER
                    } else {
                        theme::ACCENT
                    }),
                ),
                Span::styled(
                    marker,
                    Style::default()
                        .fg(if position.is_some() {
                            theme::WARNING
                        } else {
                            theme::INACTIVE
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<16} ", template.name),
                    Style::default()
                        .fg(if position.is_some() {
                            theme::ACCENT
                        } else {
                            theme::MUTED_TEXT
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    template.description.clone(),
                    Style::default().fg(theme::TEXT),
                ),
            ]))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(cursor));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Keep the root frame visibly alive while template discovery runs on the
/// launch-effect worker. The picker replaces this screen in place when the
/// response arrives.
pub fn render_template_picker_loading(frame: &mut Frame) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(" styra · Driva templates · Esc cancel ");
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  loading Driva templates…",
            Style::default().fg(theme::MUTED_TEXT),
        )))
        .block(block),
        frame.area(),
    );
}

fn session_item(
    session: &SessionSummary,
    depth: usize,
    selected: bool,
    order: SessionOrder,
) -> ListItem<'static> {
    let provider = session.selection.provider.as_str();
    let display_name = session.name.as_deref().unwrap_or(&session.id);
    let age = match order {
        SessionOrder::LastActivity if !session.last_event_age.is_empty() => {
            session.last_event_age.clone()
        }
        _ => session.age.clone(),
    };
    let mut spans = vec![
        Span::styled(
            tree_marker(depth, selected),
            Style::default().fg(if selected {
                theme::SELECTION_MARKER
            } else {
                theme::TEXT
            }),
        ),
        Span::styled(
            display_name.to_owned(),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {provider}"), Style::default().fg(theme::ACCENT)),
        Span::styled(
            format!(" · {age}"),
            Style::default().fg(theme::ADDITIONAL_INFO),
        ),
    ];
    if session.completed == styra_protocol::CompletionState::Sealed {
        spans.push(Span::styled(
            " · SEALED",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    } else if session.completed == styra_protocol::CompletionState::Completed {
        spans.push(Span::styled(
            " · COMPLETED",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    } else if session.completed == styra_protocol::CompletionState::Abandoned {
        spans.push(Span::styled(
            " · ABANDONED",
            Style::default()
                .fg(theme::STOP_ABANDONED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    ListItem::new(Line::from(spans))
}

fn tree_marker(depth: usize, selected: bool) -> String {
    let cursor = if selected { "• " } else { "  " };
    if depth == 0 {
        cursor.to_owned()
    } else {
        // A child starts beneath its parent's content, not at the same left
        // edge as the root marker. Each further branch carries that indent
        // forward, so the hierarchy reads as a tree instead of a gutter.
        format!("{}└─ {cursor}", "  ".repeat(depth))
    }
}

#[cfg(test)]
mod tests;
