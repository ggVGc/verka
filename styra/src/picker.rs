use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::time::Duration;

use styra_server::{Client, InteractionSummary, InteractionUpdate, LogEntry, WorkspaceSummary};

use crate::ui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceChoice {
    Existing(WorkspaceSummary),
    CreateCurrentDirectory,
}

/// The session picker loop: j/k or arrows to move, Enter to choose a
/// session, Esc or q to back out without picking one.
pub fn run_session_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    sessions: &[styra_server::SessionSummary],
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
            _ => {}
        }
    }
}

pub fn run_workspace_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    workspaces: &[WorkspaceSummary],
) -> Result<Option<WorkspaceChoice>> {
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
            _ => {}
        }
    }
}

/// The current-interactions picker loop: j/k or arrows to move, Enter to attach to a
/// live interaction, Esc or q to back out. Mirrors [`run_session_picker`] but over the
/// server's live interactions rather than the stored-session store.
pub fn run_interactions_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    interactions: &[InteractionSummary],
    workspaces: &[WorkspaceSummary],
) -> Result<Option<InteractionSummary>> {
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
            _ => {}
        }
    }
}
