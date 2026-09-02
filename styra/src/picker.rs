use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::HashMap;
use std::io::Stdout;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use styra_server::protocol::ResumeSession;
use styra_server::{Client, InteractionSummary, InteractionUpdate, LogEntry, WorkspaceSummary};

use crate::notes;
use crate::ui;

/// How long the cursor must rest on a Session or Workspace before its preview
/// is loaded.
/// Short enough to feel immediate when the cursor stops, long enough that
/// scrolling through the list costs no loads at all.
const PREVIEW_SETTLE: Duration = Duration::from_millis(120);

/// How often the Workspace picker re-asks the server which Interactions are
/// live. Rare enough to leave a long-open picker idle, frequent enough that a
/// turn ending is visible without the operator moving the cursor.
const LIVENESS_REFRESH: Duration = Duration::from_secs(2);

/// How often the open interactions picker replaces its summaries. Unlike the
/// conversation preview, status, selection, name, and last response all come
/// from `list_interactions`, so they need their own short refresh cadence.
const INTERACTIONS_REFRESH: Duration = Duration::from_millis(250);

struct PreviewBatch {
    updates: Vec<InteractionUpdate>,
    next: u64,
}

type PreviewLoad = (String, Result<PreviewBatch, String>);
type InteractionsRefresh = Result<Vec<InteractionSummary>, String>;

/// A live-interaction preview retained for the lifetime of its picker. The
/// summary records which server state the cursor covers; when that changes, a
/// new incremental request starts without throwing the cached conversation
/// away.
struct CachedInteractionPreview {
    summary: InteractionSummary,
    cursor: u64,
    updates: Vec<InteractionUpdate>,
    loading: bool,
}

#[derive(Default)]
struct InteractionPreviewCache {
    entries: HashMap<String, CachedInteractionPreview>,
}

impl InteractionPreviewCache {
    /// Mark a missing or stale preview as loading and return the cursor its
    /// background request should start from. One request per interaction may
    /// be in flight; a summary that changes during it remains stale and causes
    /// another incremental request as soon as this one completes.
    fn start_load(&mut self, interaction: &InteractionSummary) -> Option<u64> {
        match self.entries.get_mut(&interaction.id) {
            Some(entry) if entry.loading || entry.summary == *interaction => None,
            Some(entry) => {
                entry.summary = interaction.clone();
                entry.loading = true;
                Some(entry.cursor)
            }
            None => {
                self.entries.insert(
                    interaction.id.clone(),
                    CachedInteractionPreview {
                        summary: interaction.clone(),
                        cursor: 0,
                        updates: Vec::new(),
                        loading: true,
                    },
                );
                Some(0)
            }
        }
    }

    fn finish_load(&mut self, id: &str, result: Result<PreviewBatch, String>) {
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        entry.loading = false;
        match result {
            Ok(batch) => {
                entry.cursor = batch.next;
                if entry.updates.is_empty() {
                    entry.updates = batch.updates;
                } else {
                    entry.updates.extend(batch.updates);
                }
            }
            Err(message) => {
                let message = format!("could not load current log: {message}");
                let already_reported = entry.updates.last().is_some_and(
                    |update| matches!(update, InteractionUpdate::Log(log) if log.message == message),
                );
                if !already_reported {
                    entry
                        .updates
                        .push(InteractionUpdate::Log(LogEntry::error(message)));
                }
            }
        }
    }

    fn preview(&self, id: &str) -> ui::Preview<'_> {
        match self.entries.get(id) {
            Some(entry) if !entry.updates.is_empty() || !entry.loading => {
                ui::Preview::Ready(&entry.updates)
            }
            _ => ui::Preview::Loading,
        }
    }
}

fn load_interaction_preview(client: Client, id: String, after: u64, sender: Sender<PreviewLoad>) {
    thread::spawn(move || {
        let result = client
            .updates_without_raw(&id, after)
            .map(conversation_preview_batch)
            .map_err(|error| format!("{error:#}"));
        sender.send((id, result)).ok();
    });
}

fn conversation_preview_batch(batch: styra_server::protocol::Updates) -> PreviewBatch {
    PreviewBatch {
        updates: batch
            .updates
            .into_iter()
            .filter_map(|sequenced| match sequenced.update {
                update @ InteractionUpdate::Event(
                    styra_server::event::AgentEvent::UserMessage { .. }
                    | styra_server::event::AgentEvent::AgentMessage { .. },
                ) => Some(update),
                _ => None,
            })
            .collect(),
        next: batch.next,
    }
}

fn refresh_interactions(client: Client, sender: Sender<InteractionsRefresh>) {
    thread::spawn(move || {
        let result = client
            .list_interactions()
            .map_err(|error| format!("{error:#}"));
        sender.send(result).ok();
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
// The picker yields at most one of these per run, so the size difference never
// costs anything worth an extra allocation.
#[allow(clippy::large_enum_variant)]
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
    // Set while the cursor has moved but its Session has not been loaded yet.
    // Loading is a blocking round-trip, so holding `j` must not queue one load
    // per row it passes over; the load waits for the cursor to settle.
    let mut settle_from: Option<Instant> = None;
    loop {
        if let Some(selected_session) = sessions.get(selected) {
            if preview_id != selected_session.id {
                preview_id.clone_from(&selected_session.id);
                preview_cursor = 0;
                preview_updates.clear();
                preview_live = false;
                settle_from = Some(Instant::now());
            }
            if settle_from.is_some_and(|since| since.elapsed() >= PREVIEW_SETTLE) {
                settle_from = None;
                preview_live = client
                    .list_interactions()?
                    .iter()
                    .any(|interaction| interaction.id == preview_id);
                if !preview_live {
                    // The preview renders decoded events only, so the raw wire
                    // lines are left on the server rather than shipped here to
                    // be dropped.
                    match client.stored_session_events(&preview_id) {
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
                match client.updates_without_raw(&preview_id, preview_cursor) {
                    Ok(batch) => {
                        preview_cursor = batch.next;
                        preview_updates
                            .extend(batch.updates.into_iter().map(|sequenced| sequenced.update));
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

        // Until the settle timer fires and the load returns, the pane says so:
        // an empty conversation and an unread one look nothing alike.
        let preview = if settle_from.is_some() {
            ui::Preview::Loading
        } else {
            ui::Preview::Ready(&preview_updates)
        };
        terminal.draw(|frame| ui::render_picker(frame, sessions, selected, preview))?;

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
            KeyCode::Char('x') if !sessions.is_empty() => {
                match client.convert_session_provider(&sessions[selected].id) {
                    Ok(converted) => return Ok(Some(converted.id)),
                    Err(error) => show_message(
                        terminal,
                        sessions,
                        selected,
                        "could not convert session",
                        &format!("{error:#}"),
                    )?,
                }
            }
            _ => {}
        }
    }
}

/// Show a dismissable notice over the session picker and block until any key
/// dismisses it, so an error from an in-picker action (e.g. a failed
/// conversion) is seen rather than lost.
fn show_message(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    sessions: &[styra_server::SessionSummary],
    selected: usize,
    title: &str,
    message: &str,
) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            ui::render_picker(frame, sessions, selected, ui::Preview::Ready(&[]));
            ui::render_message_popup(frame, title, message);
        })?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                return Ok(());
            }
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
            ui::render_picker(frame, sessions, selected, ui::Preview::Ready(&[]));
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
/// operator opens is not reordered under them while they look at it — but its
/// liveness marker is refreshed as the picker sits open, so a Workspace whose
/// agent finishes or goes idle says so without the ordering shifting.
pub fn run_workspace_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    workspaces: &mut [WorkspaceSummary],
) -> Result<Option<WorkspaceChoice>> {
    // A server that cannot answer still leaves a useful list: without live
    // interactions to consult, the ordering falls back to recent access alone.
    let mut interactions = client.list_interactions().unwrap_or_default();
    sort_workspaces(workspaces, &interactions);
    let mut selected = 0usize;
    let mut refreshed = Instant::now();
    // The Session list of the row under the cursor, loaded like the session
    // picker's conversation preview: a blocking round-trip, so holding `j`
    // must not queue one load per row it passes over.
    let mut preview_id = String::new();
    let mut preview_sessions: Vec<styra_server::SessionSummary> = Vec::new();
    let mut settle_from: Option<Instant> = None;
    loop {
        if let Some(workspace) = workspaces.get(selected) {
            if preview_id != workspace.id {
                preview_id.clone_from(&workspace.id);
                preview_sessions.clear();
                settle_from = Some(Instant::now());
            }
            if settle_from.is_some_and(|since| since.elapsed() >= PREVIEW_SETTLE) {
                settle_from = None;
                preview_sessions = client.list_sessions(&preview_id).unwrap_or_default();
            }
        }
        if refreshed.elapsed() >= LIVENESS_REFRESH {
            refreshed = Instant::now();
            if let Ok(current) = client.list_interactions() {
                interactions = current;
            }
        }

        // Until the settle timer fires and the load returns, the pane says so:
        // a Workspace with no Sessions and an unread one look nothing alike.
        let preview = if settle_from.is_some() {
            ui::SessionsPreview::Loading
        } else {
            ui::SessionsPreview::Ready(&preview_sessions)
        };
        terminal.draw(|frame| {
            ui::render_workspace_picker(frame, workspaces, selected, &interactions, preview)
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
                notes::edit_workspace_notes(
                    terminal,
                    client,
                    workspaces,
                    selected,
                    &interactions,
                    &preview_sessions,
                )?;
            }
            _ => {}
        }
    }
}

/// How the interactions picker is showing the server's live interactions: over
/// every Workspace or only the one this client is attached to, in one flat list
/// or split under a heading per Workspace. Both are toggled from inside the
/// picker, so this is only where it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionsView {
    pub only_current_workspace: bool,
    pub grouped: bool,
}

impl Default for InteractionsView {
    /// Grouping is on by default: over every Workspace the headings are what
    /// keep a long list readable.
    fn default() -> Self {
        Self {
            only_current_workspace: false,
            grouped: true,
        }
    }
}

impl InteractionsView {
    /// Whether the list is actually laid out under Workspace headings. A view
    /// restricted to one Workspace has a single group, so a heading over it
    /// would only repeat what the corner already says.
    pub fn groups_by_workspace(self) -> bool {
        self.grouped && !self.only_current_workspace
    }
}

/// Everything the interactions picker draws besides the conversation preview:
/// the interactions themselves, the Workspaces they are named by, and the rows
/// and selection the current [`InteractionsView`] lays them out as.
pub struct InteractionsList<'a> {
    pub interactions: &'a [InteractionSummary],
    pub workspaces: &'a [WorkspaceSummary],
    pub current_workspace_id: Option<&'a str>,
    pub rows: &'a [InteractionRow],
    pub selected_row: Option<usize>,
    pub view: InteractionsView,
}

/// One row of the interactions picker: a Workspace heading in grouped mode, or
/// one of the interactions, by its index in the picker's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionRow {
    Workspace(String),
    Interaction(usize),
}

/// The live-interactions picker loop: j/k or arrows to move, Enter to attach
/// to a live interaction, `i` to attach and enter input mode, `X` to convert
/// the selected active interaction to the other provider, `S` to stop the
/// selected one, `D` to delete a stopped
/// one, `w` to restrict the list to the current Workspace, `g` to group it per
/// Workspace, Esc or q to back out. Mirrors [`run_session_picker`] but over the
/// server's live interactions rather than the stored-session store.
///
/// Deleting is only offered for an interaction that has already stopped: a
/// running agent is asked to stop first, so `D` never discards work in flight.
pub fn run_interactions_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    interactions: &mut Vec<InteractionSummary>,
    workspaces: &[WorkspaceSummary],
    current_workspace_id: Option<&str>,
    view: InteractionsView,
) -> Result<Option<(InteractionSummary, Option<String>, bool)>> {
    sort_interactions(interactions);
    // Without a Workspace to compare against, the restricted view has nothing
    // to restrict to, so the picker stays on the whole list.
    let mut view = InteractionsView {
        only_current_workspace: view.only_current_workspace && current_workspace_id.is_some(),
        ..view
    };
    let mut selected = 0usize;
    // Set when a toggle reorders or refilters the list under the cursor: the
    // row the operator was looking at is found again by id rather than by
    // position, so `w` and `g` do not move the selection off it.
    let mut refocus: Option<String> = None;
    let mut preview_id = String::new();
    let mut rendered_preview_id = String::new();
    let mut preview_settle_from: Option<Instant> = None;
    let mut preview_cache = InteractionPreviewCache::default();
    let (preview_sender, preview_receiver): (Sender<PreviewLoad>, Receiver<PreviewLoad>) =
        mpsc::channel();
    let (refresh_sender, refresh_receiver): (
        Sender<InteractionsRefresh>,
        Receiver<InteractionsRefresh>,
    ) = mpsc::channel();
    let mut refresh_in_flight = false;
    let mut refreshed = Instant::now();
    loop {
        let rows = interaction_rows(interactions, workspaces, current_workspace_id, view);
        let visible: Vec<usize> = rows
            .iter()
            .filter_map(|row| match row {
                InteractionRow::Interaction(index) => Some(*index),
                InteractionRow::Workspace(_) => None,
            })
            .collect();
        if let Some(id) = refocus.take() {
            if let Some(position) = visible
                .iter()
                .position(|index| interactions[*index].id == id)
            {
                selected = position;
            }
        }
        selected = selected.min(visible.len().saturating_sub(1));
        let selected_index = visible.get(selected).copied();

        match selected_index {
            Some(index) => {
                if preview_id != interactions[index].id {
                    preview_id.clone_from(&interactions[index].id);
                    preview_settle_from = Some(Instant::now());
                }
                if let Some(after) = preview_cache.start_load(&interactions[index]) {
                    load_interaction_preview(
                        client.clone(),
                        preview_id.clone(),
                        after,
                        preview_sender.clone(),
                    );
                }
            }
            None => {
                preview_id.clear();
                rendered_preview_id.clear();
                preview_settle_from = None;
            }
        }

        if preview_settle_from.is_some_and(|since| since.elapsed() >= PREVIEW_SETTLE) {
            rendered_preview_id.clone_from(&preview_id);
            preview_settle_from = None;
        }

        let selected_row = selected_row(&rows, selected_index);
        let preview = if preview_id.is_empty() {
            ui::Preview::Ready(&[])
        } else if preview_settle_from.is_some() {
            ui::Preview::Loading
        } else {
            preview_cache.preview(&rendered_preview_id)
        };
        terminal.draw(|frame| {
            ui::render_interactions_picker(
                frame,
                &InteractionsList {
                    interactions,
                    workspaces,
                    current_workspace_id,
                    rows: &rows,
                    selected_row,
                    view,
                },
                preview,
            )
        })?;

        if !event::poll(Duration::from_millis(100))? {
            // Completed background work is deliberately applied only once
            // input is idle. A queued movement key always gets a redraw before
            // any cache or summary maintenance happens on the UI thread.
            for (id, result) in preview_receiver.try_iter() {
                preview_cache.finish_load(&id, result);
            }
            if let Ok(result) = refresh_receiver.try_recv() {
                refresh_in_flight = false;
                refreshed = Instant::now();
                if let Ok(mut current) = result {
                    // Refreshing can change the status sort order. Keep the
                    // cursor on the same interaction by identity rather than
                    // letting a moving row silently change the selection.
                    if !preview_id.is_empty() {
                        refocus = Some(preview_id.clone());
                    }
                    sort_interactions(&mut current);
                    *interactions = current;
                }
            }
            if !refresh_in_flight && refreshed.elapsed() >= INTERACTIONS_REFRESH {
                refresh_in_flight = true;
                refresh_interactions(client.clone(), refresh_sender.clone());
            }
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
                selected = (selected + 1).min(visible.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(index) = selected_index {
                    return Ok(Some((interactions[index].clone(), None, false)));
                }
            }
            KeyCode::Char('i') => {
                if let Some(index) = selected_index {
                    return Ok(Some((interactions[index].clone(), None, true)));
                }
            }
            // Resume the converted sibling before stopping the source, so a
            // resume failure never stops the operator's active interaction.
            KeyCode::Char('X') => {
                let Some(index) = selected_index else {
                    continue;
                };
                let source = interactions[index].clone();
                if !source.accepting {
                    show_interactions_message(
                        terminal,
                        &InteractionsList {
                            interactions,
                            workspaces,
                            current_workspace_id,
                            rows: &rows,
                            selected_row,
                            view,
                        },
                        "interaction is not active",
                        "Only an active interaction can be converted.",
                    )?;
                    continue;
                }
                let converted = match client.convert_session_provider(&source.id) {
                    Ok(converted) => converted,
                    Err(error) => {
                        show_interactions_message(
                            terminal,
                            &InteractionsList {
                                interactions,
                                workspaces,
                                current_workspace_id,
                                rows: &rows,
                                selected_row,
                                view,
                            },
                            "could not convert interaction",
                            &format!("{error:#}"),
                        )?;
                        continue;
                    }
                };
                if let Err(error) = client.resume_session(&ResumeSession {
                    id: converted.id.clone(),
                    launch: Default::default(),
                }) {
                    show_interactions_message(
                        terminal,
                        &InteractionsList {
                            interactions,
                            workspaces,
                            current_workspace_id,
                            rows: &rows,
                            selected_row,
                            view,
                        },
                        "could not activate converted interaction",
                        &format!("{error:#}"),
                    )?;
                    continue;
                }
                if let Err(error) = client.stop_interaction(&source.id) {
                    // Do not leave two active conversations after a failed
                    // handoff. The converted Session remains available for a
                    // later retry, while the source continues untouched.
                    let _ = client.stop_interaction(&converted.id);
                    show_interactions_message(
                        terminal,
                        &InteractionsList {
                            interactions,
                            workspaces,
                            current_workspace_id,
                            rows: &rows,
                            selected_row,
                            view,
                        },
                        "could not stop original interaction",
                        &format!("{error:#}"),
                    )?;
                    continue;
                }
                match client.list_interactions().and_then(|interactions| {
                    interactions
                        .into_iter()
                        .find(|interaction| interaction.id == converted.id)
                        .ok_or_else(|| {
                            anyhow::anyhow!("the converted interaction did not become active")
                        })
                }) {
                    Ok(interaction) => {
                        return Ok(Some((
                            interaction,
                            Some(format!(
                                "converted {} session to {}",
                                source.selection.provider.as_str(),
                                converted.selection.provider.as_str(),
                            )),
                            false,
                        )));
                    }
                    Err(error) => show_interactions_message(
                        terminal,
                        &InteractionsList {
                            interactions,
                            workspaces,
                            current_workspace_id,
                            rows: &rows,
                            selected_row,
                            view,
                        },
                        "could not open converted interaction",
                        &format!("{error:#}"),
                    )?,
                }
            }
            KeyCode::Char('w') if current_workspace_id.is_some() => {
                refocus = selected_index.map(|index| interactions[index].id.clone());
                view.only_current_workspace = !view.only_current_workspace;
            }
            KeyCode::Char('g') => {
                refocus = selected_index.map(|index| interactions[index].id.clone());
                view.grouped = !view.grouped;
            }
            // Stopping ends the agent but leaves the row in place: the operator
            // can still read its log, and the interaction is only forgotten
            // when they delete it.
            KeyCode::Char('S') => {
                let Some(index) = selected_index else {
                    continue;
                };
                if !interactions[index].accepting {
                    continue;
                }
                match client.stop_interaction(&interactions[index].id) {
                    Ok(()) => interactions[index].accepting = false,
                    Err(error) => show_interactions_message(
                        terminal,
                        &InteractionsList {
                            interactions,
                            workspaces,
                            current_workspace_id,
                            rows: &rows,
                            selected_row,
                            view,
                        },
                        "could not stop interaction",
                        &format!("{error:#}"),
                    )?,
                }
            }
            // Delete the interaction: the server forgets it, so the Session
            // drops out of this list and becomes stored history like every
            // other session on disk. Only offered once it has stopped.
            KeyCode::Char('D') => {
                let Some(index) = selected_index else {
                    continue;
                };
                if interactions[index].accepting {
                    show_interactions_message(
                        terminal,
                        &InteractionsList {
                            interactions,
                            workspaces,
                            current_workspace_id,
                            rows: &rows,
                            selected_row,
                            view,
                        },
                        "interaction is still running",
                        "Stop it with S before deleting it.",
                    )?;
                    continue;
                }
                // A failure here means the server no longer knows the
                // interaction, which is the state this key asks for anyway, so
                // the row leaves the list either way.
                client.close_interaction(&interactions[index].id).ok();
                interactions.remove(index);
                if interactions.is_empty() {
                    return Ok(None);
                }
                preview_id.clear();
            }
            _ => {}
        }
    }
}

/// Which row of the rendered list the cursor sits on: the row carrying the
/// selected interaction, or none when the view is empty.
fn selected_row(rows: &[InteractionRow], selected_index: Option<usize>) -> Option<usize> {
    let index = selected_index?;
    rows.iter()
        .position(|row| matches!(row, InteractionRow::Interaction(other) if *other == index))
}

/// Lay the interactions out as picker rows, honouring the current view: the
/// restricted view drops every interaction outside the current Workspace, and
/// the grouped view gathers what is left under a heading per Workspace.
///
/// Grouping preserves the order [`sort_interactions`] established — Workspaces
/// appear in the order their first interaction does, and so do the rows within
/// each — so the Workspace holding the most urgent interaction leads.
fn interaction_rows(
    interactions: &[InteractionSummary],
    workspaces: &[WorkspaceSummary],
    current_workspace_id: Option<&str>,
    view: InteractionsView,
) -> Vec<InteractionRow> {
    let included: Vec<usize> = interactions
        .iter()
        .enumerate()
        .filter(|(_, interaction)| match current_workspace_id {
            Some(id) if view.only_current_workspace => interaction.workspace_id == id,
            _ => true,
        })
        .map(|(index, _)| index)
        .collect();
    if !view.groups_by_workspace() {
        return included
            .into_iter()
            .map(InteractionRow::Interaction)
            .collect();
    }

    let mut rows = Vec::new();
    let mut placed = vec![false; interactions.len()];
    for leader in &included {
        if placed[*leader] {
            continue;
        }
        let workspace_id = &interactions[*leader].workspace_id;
        rows.push(InteractionRow::Workspace(workspace_name(
            workspaces,
            workspace_id,
        )));
        for index in &included {
            if !placed[*index] && &interactions[*index].workspace_id == workspace_id {
                placed[*index] = true;
                rows.push(InteractionRow::Interaction(*index));
            }
        }
    }
    rows
}

/// The Workspace's display name, falling back to its id when this client has
/// no summary for it — the server may know Workspaces this list does not.
pub fn workspace_name(workspaces: &[WorkspaceSummary], workspace_id: &str) -> String {
    workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .map(crate::session::workspace_display_name)
        .unwrap_or_else(|| workspace_id.to_owned())
}

/// Show a dismissable notice over the interactions picker, so an error or a
/// refused action is seen rather than lost.
fn show_interactions_message(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    list: &InteractionsList<'_>,
    title: &str,
    message: &str,
) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            ui::render_interactions_picker(frame, list, ui::Preview::Ready(&[]));
            ui::render_message_popup(frame, title, message);
        })?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                return Ok(());
            }
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
            last_message: None,
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

    #[test]
    fn interaction_previews_load_immediately_then_stay_cached() {
        let interaction = interaction("one", true, InteractionActivity::Pending);
        let mut cache = InteractionPreviewCache::default();

        assert_eq!(cache.start_load(&interaction), Some(0));
        assert_eq!(cache.start_load(&interaction), None);
        assert!(matches!(
            cache.preview(&interaction.id),
            ui::Preview::Loading
        ));

        cache.finish_load(
            &interaction.id,
            Ok(conversation_preview_batch(
                styra_server::protocol::Updates {
                    updates: vec![
                        styra_server::protocol::SequencedUpdate {
                            sequence: 1,
                            update: InteractionUpdate::Event(
                                styra_server::event::AgentEvent::CommandStarted {
                                    command: "cargo test".into(),
                                },
                            ),
                        },
                        styra_server::protocol::SequencedUpdate {
                            sequence: 2,
                            update: InteractionUpdate::Event(
                                styra_server::event::AgentEvent::AgentMessage {
                                    text: "ready".into(),
                                },
                            ),
                        },
                    ],
                    next: 2,
                },
            )),
        );
        assert_eq!(cache.start_load(&interaction), None);
        assert!(matches!(
            cache.preview(&interaction.id),
            ui::Preview::Ready(updates) if updates.len() == 1
        ));
    }

    #[test]
    fn new_interaction_information_refreshes_a_cached_preview_incrementally() {
        let interaction = interaction("one", true, InteractionActivity::Running);
        let mut cache = InteractionPreviewCache::default();
        assert_eq!(cache.start_load(&interaction), Some(0));
        cache.finish_load(
            &interaction.id,
            Ok(conversation_preview_batch(
                styra_server::protocol::Updates {
                    updates: vec![],
                    next: 7,
                },
            )),
        );

        let changed = InteractionSummary {
            last_message: Some("new response".into()),
            ..interaction
        };
        assert_eq!(cache.start_load(&changed), Some(7));
        assert_eq!(cache.start_load(&changed), None);
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
