use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::time::Duration;

use styra_server::{Client, InteractionSummary, InteractionUpdate, LogEntry, WorkspaceSummary};

use crate::notes;
use crate::ui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceChoice {
    Existing(WorkspaceSummary),
    CreateCurrentDirectory,
}

/// The session picker loop: j/k or arrows to move, Enter to choose a
/// session, `e` to edit its Session notes, Esc or q to back out.
pub fn run_session_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    sessions: &mut [styra_server::SessionSummary],
) -> Result<Option<String>> {
    let mut selected = 0usize;
    let mut preview_id = String::new();
    let mut preview_cursor = 0u64;
    let mut preview_updates = Vec::new();
    let mut preview_live = false;
    loop {
        if let Some(selected_session) = sessions.get(selected) {
            if preview_id != selected_session.id {
                preview_id.clone_from(&selected_session.id);
                preview_cursor = 0;
                preview_updates.clear();
                preview_live = client
                    .list_interactions()?
                    .iter()
                    .any(|interaction| interaction.id == preview_id);
                if !preview_live {
                    match client.stored_session(&preview_id) {
                        Ok(stored) => {
                            preview_updates.extend(
                                stored
                                    .events
                                    .into_iter()
                                    .filter(|event| {
                                        !matches!(
                                            event,
                                            styra_server::event::AgentEvent::Unknown { .. }
                                        )
                                    })
                                    .map(InteractionUpdate::Event),
                            );
                        }
                        Err(error) => preview_updates.push(InteractionUpdate::Log(
                            LogEntry::error(format!("could not load session log: {error:#}")),
                        )),
                    }
                }
            }
            if preview_live {
                match client.updates(&preview_id, preview_cursor) {
                    Ok(batch) => {
                        preview_cursor = batch.next;
                        preview_updates.extend(batch.updates.into_iter().filter_map(|sequenced| {
                            match sequenced.update {
                                InteractionUpdate::Raw(_) => None,
                                update => Some(update),
                            }
                        }));
                    }
                    Err(error) => {
                        let message = format!("could not load current log: {error:#}");
                        if !preview_updates.last().is_some_and(
                            |update| matches!(update, InteractionUpdate::Log(entry) if entry.message == message),
                        ) {
                            preview_updates
                                .push(InteractionUpdate::Log(LogEntry::error(message)));
                        }
                    }
                }
            }
        }

        terminal.draw(|frame| ui::render_picker(frame, sessions, selected, &preview_updates))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(sessions.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter if !sessions.is_empty() => {
                return Ok(Some(sessions[selected].id.clone()));
            }
            KeyCode::Char('r') if !sessions.is_empty() => {
                if let Some(name) = read_session_name(
                    terminal,
                    sessions,
                    selected,
                    sessions[selected].name.as_deref().unwrap_or(""),
                )? {
                    sessions[selected] = client.rename_session(
                        &sessions[selected].id,
                        (!name.trim().is_empty()).then_some(name.as_str()),
                    )?;
                }
            }
            KeyCode::Char('e') if !sessions.is_empty() => {
                notes::edit_session_notes(terminal, client, sessions, selected, &preview_updates)?;
            }
            _ => {}
        }
    }
}

fn read_session_name(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    sessions: &[styra_server::SessionSummary],
    selected: usize,
    initial: &str,
) -> Result<Option<String>> {
    let mut value = initial.to_owned();
    loop {
        terminal.draw(|frame| {
            ui::render_picker(frame, sessions, selected, &[]);
            ui::render_name_prompt(frame, &value);
        })?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter => return Ok(Some(value)),
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(ch) if value.chars().count() < 80 && !ch.is_control() => value.push(ch),
            _ => {}
        }
    }
}

/// The Workspace picker loop: j/k or arrows to move, Enter to open a
/// Workspace, `e` to edit its Workspace notes, `c` to create one for the
/// current directory, Esc or q to back out.
///
/// The list is ordered once on entry, by [`sort_workspaces`]. A Workspace the
/// operator opens is not reordered under them while they look at it.
pub fn run_workspace_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    workspaces: &mut [WorkspaceSummary],
) -> Result<Option<WorkspaceChoice>> {
    // A server that cannot answer still leaves a useful list: without live
    // interactions to consult, the ordering falls back to recent access alone.
    sort_workspaces(workspaces, &client.list_interactions().unwrap_or_default());
    let mut selected = 0usize;
    loop {
        terminal.draw(|frame| ui::render_workspace_picker(frame, workspaces, selected))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(workspaces.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter if !workspaces.is_empty() => {
                return Ok(Some(WorkspaceChoice::Existing(
                    workspaces[selected].clone(),
                )));
            }
            KeyCode::Char('c') => return Ok(Some(WorkspaceChoice::CreateCurrentDirectory)),
            KeyCode::Char('e') if !workspaces.is_empty() => {
                notes::edit_workspace_notes(terminal, client, workspaces, selected)?;
            }
            _ => {}
        }
    }
}

/// The current-interactions picker loop: j/k or arrows to move, Enter to attach to a
/// live interaction, `d` to close the selected one, Esc or q to back out. Mirrors
/// [`run_session_picker`] but over the server's live interactions rather than the
/// stored-session store.
pub fn run_interactions_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    interactions: &mut Vec<InteractionSummary>,
    workspaces: &[WorkspaceSummary],
) -> Result<Option<InteractionSummary>> {
    sort_interactions(interactions);
    let mut selected = 0usize;
    let mut preview_id = String::new();
    let mut preview_cursor = 0u64;
    let mut preview_updates = Vec::new();
    loop {
        let selected_interaction = &interactions[selected];
        if preview_id != selected_interaction.id {
            preview_id.clone_from(&selected_interaction.id);
            preview_cursor = 0;
            preview_updates.clear();
        }

        match client.updates(&preview_id, preview_cursor) {
            Ok(batch) => {
                preview_cursor = batch.next;
                preview_updates.extend(batch.updates.into_iter().filter_map(|sequenced| {
                    match sequenced.update {
                        InteractionUpdate::Raw(_) => None,
                        update => Some(update),
                    }
                }));
            }
            Err(error) => {
                let message = format!("could not load current log: {error:#}");
                if !preview_updates
                    .last()
                    .is_some_and(
                        |update| matches!(update, InteractionUpdate::Log(entry) if entry.message == message),
                    )
                {
                    preview_updates.push(InteractionUpdate::Log(LogEntry::error(message)));
                }
            }
        }

        terminal.draw(|frame| {
            ui::render_interactions_picker(
                frame,
                interactions,
                workspaces,
                selected,
                &preview_updates,
            )
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(interactions.len() - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter => return Ok(Some(interactions[selected].clone())),
            // Close the interaction: the server stops it and forgets it, so the
            // Session drops out of this list and becomes stored history like
            // every other session on disk.
            KeyCode::Char('d') => {
                // A failure here means the server no longer knows the
                // interaction, which is the state this key asks for anyway, so
                // the row leaves the list either way.
                client.close_interaction(&interactions[selected].id).ok();
                interactions.remove(selected);
                if interactions.is_empty() {
                    return Ok(None);
                }
                selected = selected.min(interactions.len() - 1);
                preview_id.clear();
            }
            _ => {}
        }
    }
}

/// The Driva template picker. Templates layer rather than replace one another,
/// so this is a multi-select: j/k or arrows to move, Space to add or drop the
/// template under the cursor, Enter to accept the whole set, Esc or q to leave
/// it as it was.
///
/// Selection order is preserved because Driva applies templates in order and
/// later ones win on conflict, so the sequence the operator built is the
/// policy they get.
pub fn run_template_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    templates: &[styra_server::TemplateSummary],
    selected_names: &[String],
) -> Result<Option<Vec<String>>> {
    let mut chosen: Vec<String> = selected_names
        .iter()
        .filter(|name| templates.iter().any(|template| &template.name == *name))
        .cloned()
        .collect();
    let mut cursor = 0usize;
    loop {
        terminal.draw(|frame| ui::render_template_picker(frame, templates, &chosen, cursor))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('j') | KeyCode::Down => {
                cursor = (cursor + 1).min(templates.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => cursor = cursor.saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some(template) = templates.get(cursor) {
                    match chosen.iter().position(|name| name == &template.name) {
                        Some(index) => {
                            chosen.remove(index);
                        }
                        None => chosen.push(template.name.clone()),
                    }
                }
            }
            KeyCode::Enter => return Ok(Some(chosen)),
            _ => {}
        }
    }
}

/// Order Workspaces for the picker: those holding a live interaction first,
/// then the rest, and within each group the most recently accessed first.
///
/// A live interaction is one the server is still accepting input for, whether
/// it is idle and waiting on the operator or busy with a turn. Those are the
/// Workspaces the operator has work in flight in, so they belong above ones
/// only recency speaks for.
fn sort_workspaces(workspaces: &mut [WorkspaceSummary], interactions: &[InteractionSummary]) {
    workspaces.sort_by(|a, b| {
        has_live_interaction(b, interactions)
            .cmp(&has_live_interaction(a, interactions))
            .then_with(|| b.last_accessed_at_ms.cmp(&a.last_accessed_at_ms))
            .then_with(|| b.created_at_ms.cmp(&a.created_at_ms))
    });
}

fn has_live_interaction(workspace: &WorkspaceSummary, interactions: &[InteractionSummary]) -> bool {
    interactions
        .iter()
        .any(|interaction| interaction.accepting && interaction.workspace_id == workspace.id)
}

/// Put idle interactions first — they are the ones waiting on the operator —
/// then interactions still processing work, and stopped interactions last.
/// `sort_by_key` is stable, so the server's ordering is retained within each
/// status group.
fn sort_interactions(interactions: &mut [InteractionSummary]) {
    interactions.sort_by_key(|interaction| {
        if !interaction.accepting {
            2
        } else {
            match interaction.activity {
                styra_server::InteractionActivity::Pending => 0,
                styra_server::InteractionActivity::Running => 1,
                styra_server::InteractionActivity::Background => 1,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_server::{agent::Selection, DrivaOptions, InteractionActivity};

    fn interaction(id: &str, accepting: bool, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            id: id.into(),
            name: None,
            workspace_id: "workspace".into(),
            selection: Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            driva: DrivaOptions {
                isolation_backend: "none".into(),
                command: vec![],
                working_directory: PathBuf::from("/workspace"),
                network: false,
                mounts: vec![],
            },
            accepting,
            activity,
        }
    }

    fn interaction_in(
        id: &str,
        workspace_id: &str,
        accepting: bool,
        activity: InteractionActivity,
    ) -> InteractionSummary {
        InteractionSummary {
            workspace_id: workspace_id.into(),
            ..interaction(id, accepting, activity)
        }
    }

    fn workspace(id: &str, last_accessed_at_ms: u64) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: None,
            notes: String::new(),
            host_path: format!("/home/op/{id}").into(),
            path: format!("/state/workspaces/{id}").into(),
            session_count: 0,
            age: "now".into(),
            created_at_ms: 1,
            last_accessed_at_ms,
            launch: Default::default(),
        }
    }

    #[test]
    fn workspaces_sort_by_recent_access() {
        let mut workspaces = vec![
            workspace("older", 10),
            workspace("newest", 30),
            workspace("newer", 20),
        ];

        sort_workspaces(&mut workspaces, &[]);

        assert_eq!(
            workspaces
                .iter()
                .map(|workspace| workspace.id.as_str())
                .collect::<Vec<_>>(),
            ["newest", "newer", "older"]
        );
    }

    #[test]
    fn workspaces_with_live_interactions_sort_above_more_recently_accessed_ones() {
        let mut workspaces = vec![
            workspace("untouched", 40),
            workspace("running", 10),
            workspace("stopped", 30),
            workspace("idle", 20),
        ];
        let interactions = vec![
            interaction_in("a", "running", true, InteractionActivity::Running),
            interaction_in("b", "stopped", false, InteractionActivity::Pending),
            interaction_in("c", "idle", true, InteractionActivity::Pending),
        ];

        sort_workspaces(&mut workspaces, &interactions);

        // Idle and running lead, ordered by access between themselves; a
        // Workspace whose only interaction has stopped ranks with the rest.
        assert_eq!(
            workspaces
                .iter()
                .map(|workspace| workspace.id.as_str())
                .collect::<Vec<_>>(),
            ["idle", "running", "untouched", "stopped"]
        );
    }

    #[test]
    fn interactions_sort_idle_then_running_then_stopped_stably() {
        let mut interactions = vec![
            interaction("stopped-1", false, InteractionActivity::Running),
            interaction("idle-1", true, InteractionActivity::Pending),
            interaction("pending-1", true, InteractionActivity::Running),
            interaction("stopped-2", false, InteractionActivity::Pending),
            interaction("pending-2", true, InteractionActivity::Running),
            interaction("idle-2", true, InteractionActivity::Pending),
        ];

        sort_interactions(&mut interactions);

        assert_eq!(
            interactions
                .iter()
                .map(|interaction| interaction.id.as_str())
                .collect::<Vec<_>>(),
            [
                "idle-1",
                "idle-2",
                "pending-1",
                "pending-2",
                "stopped-1",
                "stopped-2",
            ]
        );
    }
}
