use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use styra_protocol::{InteractionSummary, InteractionUpdate, LogEntry, WorkspaceSummary};
use styra_server::Client;

use crate::help::Help;
use crate::keymap::Window;
use crate::launch::LaunchScope;
use crate::presentation;
use crate::session::{is_recent_session, session_tree_depths, sort_sessions_tree, SessionOrder};
use styra_ui::Ui;

/// How long the cursor must rest on a Session or Workspace before its preview
/// is loaded.
/// Short enough to feel immediate when the cursor stops, long enough that
/// scrolling through the list costs no loads at all.
const PREVIEW_SETTLE: Duration = Duration::from_millis(120);

/// How often the Workspace picker re-asks the server which Interactions are
/// live. Rare enough to leave a long-open picker idle, frequent enough that a
/// turn ending is visible without the operator moving the cursor.
const LIVENESS_REFRESH: Duration = Duration::from_secs(2);

/// Ordering remains a TUI decision because it controls keyboard navigation;
/// the UI receives only the corresponding presentation label.
fn picker_order(order: SessionOrder) -> styra_ui::picker::SessionOrder {
    match order {
        SessionOrder::LastActivity => styra_ui::picker::SessionOrder::LastActivity,
        SessionOrder::Created => styra_ui::picker::SessionOrder::Creation,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// The picker yields at most one of these per run, so the size difference never
// costs anything worth an extra allocation.
#[allow(clippy::large_enum_variant)]
pub enum WorkspaceChoice {
    Existing(WorkspaceSummary),
    CreateCurrentDirectory,
}

/// What the operator left the session picker with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionChoice {
    /// Open the stored Session with this id.
    Open(String),
    /// Start a fresh Session in the Workspace whose list was being browsed.
    New,
}

/// The session picker loop: j/k or arrows to move, Enter to choose a
/// session, `n` to start a new one instead of resuming any of them, `s` to
/// switch between ordering by last activity and by creation, `a` to toggle
/// history older than a week, `c` to toggle showing Sessions the operator has
/// marked completed (hidden by default, the same convention as the live
/// interactions navigator), `C` to mark the selected Session completed or not,
/// and `/` to filter by name or first prompt. Esc abandons an active search,
/// then backs out. When `current_id` is in the list, it opens selected even if
/// another root or branch sorts above it.
///
/// `can_start_new` says whether this picker was opened somewhere that can act
/// on [`SessionChoice::New`]: browsing to attach a shell or to view a stored
/// log has no Workspace to start work in, so there `n` is not a key at all.
///
/// `?` shows the whole list of those keys. The picker's own title says only
/// what the list cannot be read without — the filter, and the sort — because
/// a strip of shortcuts along the top can never hold all of them anyway.
pub fn run_session_picker(
    terminal: &mut dyn Ui,
    client: &Client,
    sessions: &mut [styra_protocol::SessionSummary],
    current_id: Option<&str>,
    can_start_new: bool,
) -> Result<Option<SessionChoice>> {
    let mut order = SessionOrder::LastActivity;
    let mut all_sessions = sessions.to_vec();
    let now_ms = unix_now_ms();
    let mut showing_all = false;
    let mut show_completed = false;
    let mut filter: Option<String> = None;
    let mut searching = false;
    let mut sessions = picker_sessions(
        &all_sessions,
        showing_all,
        show_completed,
        now_ms,
        order,
        None,
    );
    let mut selected = initial_session_selection(&sessions, current_id);
    let mut preview_id = String::new();
    let mut preview_cursor = 0u64;
    let mut preview_updates = Vec::new();
    let mut preview_live = false;
    // Set while the cursor has moved but its Session has not been loaded yet.
    // Loading is a blocking round-trip, so holding `j` must not queue one load
    // per row it passes over; the load waits for the cursor to settle.
    let mut settle_from: Option<Instant> = None;
    let mut help = Help::default();
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
                                            styra_protocol::event::AgentEvent::Unknown { .. }
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
            presentation::Preview::Loading
        } else {
            presentation::Preview::Ready(&preview_updates)
        };
        if help.is_open() {
            render_help(terminal, Window::SessionPicker, &mut help)?;
        } else {
            terminal.render_session_picker(
                &sessions,
                selected,
                picker_order(order),
                preview,
                filter.as_deref(),
                searching,
                show_completed,
            )?;
        }

        let Some(Event::Key(key)) = terminal.poll_event(Duration::from_millis(100))? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        // The reference is modal: it describes the list underneath, so none of
        // what it describes acts while it is up.
        if handle_help_key(&mut help, key.code) {
            continue;
        }
        if searching {
            match key.code {
                KeyCode::Esc => {
                    filter = None;
                    searching = false;
                }
                KeyCode::Enter => searching = false,
                KeyCode::Backspace => {
                    if let Some(filter) = &mut filter {
                        filter.pop();
                    }
                }
                KeyCode::Char(character) if !character.is_control() => {
                    filter.get_or_insert_with(String::new).push(character);
                }
                _ => continue,
            }
            let cursor_id = sessions.get(selected).map(|session| session.id.clone());
            sessions = picker_sessions(
                &all_sessions,
                showing_all,
                show_completed,
                now_ms,
                order,
                filter.as_deref(),
            );
            selected = cursor_id
                .and_then(|id| sessions.iter().position(|session| session.id == id))
                .unwrap_or_else(|| initial_session_selection(&sessions, current_id));
            continue;
        }
        match key.code {
            KeyCode::Esc if filter.is_some() => {
                filter = None;
                sessions = picker_sessions(
                    &all_sessions,
                    showing_all,
                    show_completed,
                    now_ms,
                    order,
                    None,
                );
                selected = initial_session_selection(&sessions, current_id);
            }
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('?') => help.open(),
            KeyCode::Char('/') => {
                filter = Some(String::new());
                searching = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(sessions.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Char('g') => selected = 0,
            KeyCode::Char('G') => selected = sessions.len().saturating_sub(1),
            KeyCode::Char('J') => {
                selected = next_top_level_session(&sessions, selected).unwrap_or(selected)
            }
            KeyCode::Char('K') => {
                selected = previous_top_level_session(&sessions, selected).unwrap_or(selected)
            }
            // Re-ordering keeps the cursor on the Session it was on: the
            // operator is changing how the list is arranged, not which
            // conversation they were looking at.
            KeyCode::Char('s') => {
                let cursor_id = sessions.get(selected).map(|session| session.id.clone());
                order = order.toggled();
                sort_sessions_tree(&mut sessions, order);
                selected = cursor_id
                    .and_then(|id| sessions.iter().position(|session| session.id == id))
                    .unwrap_or(0);
            }
            KeyCode::Char('a') => {
                let cursor_id = sessions.get(selected).map(|session| session.id.clone());
                showing_all = !showing_all;
                sessions = picker_sessions(
                    &all_sessions,
                    showing_all,
                    show_completed,
                    now_ms,
                    order,
                    filter.as_deref(),
                );
                selected = cursor_id
                    .and_then(|id| sessions.iter().position(|session| session.id == id))
                    .unwrap_or_else(|| initial_session_selection(&sessions, current_id));
            }
            KeyCode::Char('c') => {
                let cursor_id = sessions.get(selected).map(|session| session.id.clone());
                show_completed = !show_completed;
                sessions = picker_sessions(
                    &all_sessions,
                    showing_all,
                    show_completed,
                    now_ms,
                    order,
                    filter.as_deref(),
                );
                selected = cursor_id
                    .and_then(|id| sessions.iter().position(|session| session.id == id))
                    .unwrap_or_else(|| initial_session_selection(&sessions, current_id));
            }
            // Completion is stored on the Session, so the row can be marked
            // from here without the interaction being live — and unmarked the
            // same way, which is the only way back once `c` has revealed it.
            KeyCode::Char('C') if !sessions.is_empty() => {
                let completed = !sessions[selected].completed;
                let id = sessions[selected].id.clone();
                if let Err(error) = client.set_session_completed(&id, completed) {
                    show_message(
                        terminal,
                        &sessions,
                        selected,
                        order,
                        "could not change completion",
                        &format!("{error:#}"),
                    )?;
                    continue;
                }
                sessions[selected].completed = completed;
                if let Some(session) = all_sessions.iter_mut().find(|session| session.id == id) {
                    session.completed = completed;
                }
                sessions = picker_sessions(
                    &all_sessions,
                    showing_all,
                    show_completed,
                    now_ms,
                    order,
                    filter.as_deref(),
                );
                // A Session that has just left the list leaves the cursor
                // where it was, which is now the row that took its place.
                selected = sessions
                    .iter()
                    .position(|session| session.id == id)
                    .unwrap_or_else(|| selected.min(sessions.len().saturating_sub(1)));
            }
            KeyCode::Char('n') if can_start_new => return Ok(Some(SessionChoice::New)),
            KeyCode::Enter if !sessions.is_empty() => {
                return Ok(Some(SessionChoice::Open(sessions[selected].id.clone())));
            }
            KeyCode::Char('r') if !sessions.is_empty() => {
                if let Some(name) = read_session_name(
                    terminal,
                    &sessions,
                    selected,
                    order,
                    sessions[selected].name.as_deref().unwrap_or(""),
                )? {
                    let renamed = client.rename_session(
                        &sessions[selected].id,
                        (!name.trim().is_empty()).then_some(name.as_str()),
                    )?;
                    sessions[selected] = renamed.clone();
                    if let Some(index) = all_sessions
                        .iter()
                        .position(|session| session.id == renamed.id)
                    {
                        all_sessions[index] = renamed;
                    }
                }
            }
            KeyCode::Char('x') if !sessions.is_empty() => {
                match client.convert_session_provider(&sessions[selected].id) {
                    Ok(converted) => return Ok(Some(SessionChoice::Open(converted.id))),
                    Err(error) => show_message(
                        terminal,
                        &sessions,
                        selected,
                        order,
                        "could not convert session",
                        &format!("{error:#}"),
                    )?,
                }
            }
            _ => {}
        }
    }
}

/// Draw the reference for `window` over the picker, and record how far it can
/// actually scroll — the renderer is the only thing that knows the height.
fn render_help(terminal: &mut dyn Ui, window: Window, help: &mut Help) -> Result<()> {
    let rows = presentation::help_rows(window);
    let feedback = terminal.render_help(
        window.name(),
        &rows,
        crate::keymap::CLOSE_REFERENCE,
        help.offset(),
    )?;
    if let Some(scroll) = feedback
        .scroll
        .iter()
        .find(|scroll| scroll.panel == styra_ui::PanelId::Help)
    {
        help.apply_feedback(scroll.limit, scroll.effective_offset);
    }
    Ok(())
}

/// Handle a key while the reference is open, reporting whether it owned it.
/// The reference is modal, so it owns every key until it is closed.
fn handle_help_key(help: &mut Help, code: KeyCode) -> bool {
    if !help.is_open() {
        return false;
    }
    match code {
        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => help.close(),
        KeyCode::Char('j') | KeyCode::Down => help.line_down(),
        KeyCode::Char('k') | KeyCode::Up => help.line_up(),
        KeyCode::PageDown => help.page_down(),
        KeyCode::PageUp => help.page_up(),
        KeyCode::Char('g') => help.scroll_to_top(),
        _ => {}
    }
    true
}

fn picker_sessions(
    sessions: &[styra_protocol::SessionSummary],
    showing_all: bool,
    show_completed: bool,
    now_ms: u64,
    order: SessionOrder,
    filter: Option<&str>,
) -> Vec<styra_protocol::SessionSummary> {
    let filter = filter
        .map(str::to_lowercase)
        .filter(|filter| !filter.is_empty());
    let mut displayed = sessions
        .iter()
        .filter(|session| {
            (showing_all || is_recent_session(session, now_ms))
                && (show_completed || !session.completed)
                && filter.as_ref().is_none_or(|filter| {
                    session
                        .name
                        .as_deref()
                        .into_iter()
                        .chain(session.first_prompt.as_deref())
                        .any(|text| text.to_lowercase().contains(filter))
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    sort_sessions_tree(&mut displayed, order);
    displayed
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The next root conversation below `selected`, skipping every branch nested
/// beneath the current root. This is the session tree's coarse navigation;
/// j/k remain the way to walk individual branches.
fn next_top_level_session(
    sessions: &[styra_protocol::SessionSummary],
    selected: usize,
) -> Option<usize> {
    session_tree_depths(sessions)
        .into_iter()
        .enumerate()
        .find_map(|(index, depth)| (index > selected && depth == 0).then_some(index))
}

/// The root conversation above `selected`. From inside a branch this lands on
/// its root first, then a subsequent K moves to the preceding root.
fn previous_top_level_session(
    sessions: &[styra_protocol::SessionSummary],
    selected: usize,
) -> Option<usize> {
    session_tree_depths(sessions)
        .into_iter()
        .enumerate()
        .rev()
        .find_map(|(index, depth)| (index < selected && depth == 0).then_some(index))
}

fn initial_session_selection(
    sessions: &[styra_protocol::SessionSummary],
    current_id: Option<&str>,
) -> usize {
    current_id
        .and_then(|id| sessions.iter().position(|session| session.id == id))
        .unwrap_or(0)
}

/// Show a dismissable notice over the session picker and block until any key
/// dismisses it, so an error from an in-picker action (e.g. a failed
/// conversion) is seen rather than lost.
fn show_message(
    terminal: &mut dyn Ui,
    sessions: &[styra_protocol::SessionSummary],
    selected: usize,
    order: SessionOrder,
    title: &str,
    message: &str,
) -> Result<()> {
    loop {
        terminal.render_session_picker_message(
            sessions,
            selected,
            picker_order(order),
            presentation::Preview::Ready(&[]),
            None,
            false,
            false,
            title,
            message,
        )?;
        if let Some(Event::Key(key)) = terminal.poll_event(Duration::from_millis(100))? {
            if key.kind == KeyEventKind::Press {
                return Ok(());
            }
        }
    }
}

fn read_session_name(
    terminal: &mut dyn Ui,
    sessions: &[styra_protocol::SessionSummary],
    selected: usize,
    order: SessionOrder,
    initial: &str,
) -> Result<Option<String>> {
    let mut value = initial.to_owned();
    loop {
        terminal.render_session_picker_name_prompt(
            sessions,
            selected,
            picker_order(order),
            presentation::Preview::Ready(&[]),
            None,
            false,
            false,
            &value,
        )?;
        let Some(Event::Key(key)) = terminal.poll_event(Duration::from_millis(100))? else {
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
/// Workspace, `c` to create one for the current directory, `/` to filter by
/// name, Esc or q to back out, and `?` for that list on
/// screen. Esc abandons an active search, then clears the filter, then backs
/// out.
///
/// The list is ordered once on entry, by [`sort_workspaces`]. A Workspace the
/// operator opens is not reordered under them while they look at it — but its
/// liveness marker is refreshed as the picker sits open, so a Workspace whose
/// agent finishes or goes idle says so without the ordering shifting.
///
/// Filtering narrows that same ordering rather than re-deriving it, so a
/// Workspace does not move relative to its neighbours as characters are typed.
pub fn run_workspace_picker(
    terminal: &mut dyn Ui,
    client: &Client,
    workspaces: &mut [WorkspaceSummary],
) -> Result<Option<WorkspaceChoice>> {
    // A server that cannot answer still leaves a useful list: without live
    // interactions to consult, the ordering falls back to recent access alone.
    let mut interactions = client.list_interactions().unwrap_or_default();
    sort_workspaces(workspaces, &interactions);
    label_recency(workspaces, &interactions, unix_now_ms());
    let mut all_workspaces = workspaces.to_vec();
    let mut filter: Option<String> = None;
    let mut searching = false;
    let mut workspaces = picker_workspaces(&all_workspaces, None);
    let mut selected = 0usize;
    let mut refreshed = Instant::now();
    // The Session list of the row under the cursor, loaded like the session
    // picker's conversation preview: a blocking round-trip, so holding `j`
    // must not queue one load per row it passes over.
    let mut preview_id = String::new();
    let mut preview_sessions: Vec<styra_protocol::SessionSummary> = Vec::new();
    let mut settle_from: Option<Instant> = None;
    let mut help = Help::default();
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
            // The recency column ages as the picker sits open, and a turn
            // ending in a Workspace resets it. Relabelling in place leaves the
            // ordering — and so the cursor — where the operator left it.
            let now = unix_now_ms();
            label_recency(&mut all_workspaces, &interactions, now);
            label_recency(&mut workspaces, &interactions, now);
        }

        // Until the settle timer fires and the load returns, the pane says so:
        // a Workspace with no Sessions and an unread one look nothing alike.
        let preview = if settle_from.is_some() {
            presentation::SessionsPreview::Loading
        } else {
            presentation::SessionsPreview::Ready(&preview_sessions)
        };
        if help.is_open() {
            render_help(terminal, Window::WorkspacePicker, &mut help)?;
        } else {
            terminal.render_workspace_picker(
                &workspaces,
                selected,
                &interactions,
                preview,
                filter.as_deref(),
                searching,
            )?;
        }
        let Some(Event::Key(key)) = terminal.poll_event(Duration::from_millis(100))? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if handle_help_key(&mut help, key.code) {
            continue;
        }
        if searching {
            match key.code {
                KeyCode::Esc => {
                    filter = None;
                    searching = false;
                }
                KeyCode::Enter => searching = false,
                KeyCode::Backspace => {
                    if let Some(filter) = &mut filter {
                        filter.pop();
                    }
                }
                KeyCode::Char(character) if !character.is_control() => {
                    filter.get_or_insert_with(String::new).push(character);
                }
                _ => continue,
            }
            // The cursor stays on the Workspace it was on for as long as the
            // narrowing list still holds it; when it is typed away, the list
            // reads from the top.
            let cursor_id = workspaces
                .get(selected)
                .map(|workspace| workspace.id.clone());
            workspaces = picker_workspaces(&all_workspaces, filter.as_deref());
            selected = cursor_id
                .and_then(|id| workspaces.iter().position(|workspace| workspace.id == id))
                .unwrap_or(0);
            continue;
        }
        match key.code {
            KeyCode::Esc if filter.is_some() => {
                let cursor_id = workspaces
                    .get(selected)
                    .map(|workspace| workspace.id.clone());
                filter = None;
                workspaces = picker_workspaces(&all_workspaces, None);
                selected = cursor_id
                    .and_then(|id| workspaces.iter().position(|workspace| workspace.id == id))
                    .unwrap_or(0);
            }
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Char('?') => help.open(),
            KeyCode::Char('/') => {
                filter = Some(String::new());
                searching = true;
            }
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

/// The Workspaces the filter leaves on screen, in the order they were sorted
/// into on entry.
///
/// A Workspace is matched on its name alone — the operator-facing name, or the
/// host directory name standing in for it when there is none.
fn picker_workspaces(
    workspaces: &[WorkspaceSummary],
    filter: Option<&str>,
) -> Vec<WorkspaceSummary> {
    let filter = filter
        .map(str::to_lowercase)
        .filter(|filter| !filter.is_empty());
    workspaces
        .iter()
        .filter(|workspace| {
            filter
                .as_ref()
                .is_none_or(|filter| workspace_matches(workspace, filter))
        })
        .cloned()
        .collect()
}

/// Whether `filter`, already lowercased, appears in the Workspace's displayed
/// name.
fn workspace_matches(workspace: &WorkspaceSummary, filter: &str) -> bool {
    let name = workspace.name.clone().unwrap_or_else(|| {
        workspace
            .host_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned()
    });
    name.to_lowercase().contains(filter)
}

/// Root-loop-owned state for the Driva template chooser. `templates` is `None`
/// while discovery is running on the launch-effect worker; keeping that state
/// in `App` lets the ordinary frame loop remain alive throughout the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplatePicker {
    pub request_id: u64,
    pub workspace_id: String,
    pub scope: LaunchScope,
    pub templates: Option<Vec<styra_protocol::TemplateSummary>>,
    initial: Vec<String>,
    pub chosen: Vec<String>,
    pub cursor: usize,
}

impl TemplatePicker {
    pub fn loading(
        request_id: u64,
        workspace_id: String,
        scope: LaunchScope,
        chosen: Vec<String>,
    ) -> Self {
        Self {
            request_id,
            workspace_id,
            scope,
            templates: None,
            initial: chosen.clone(),
            chosen,
            cursor: 0,
        }
    }

    pub fn loaded(&mut self, templates: Vec<styra_protocol::TemplateSummary>) {
        self.chosen
            .retain(|name| templates.iter().any(|template| &template.name == name));
        self.initial.clone_from(&self.chosen);
        self.templates = Some(templates);
        self.cursor = 0;
    }

    pub fn handle_key(&mut self, code: KeyCode) -> TemplatePickerAction {
        let Some(templates) = self.templates.as_ref() else {
            return if matches!(code, KeyCode::Char('q') | KeyCode::Esc) {
                TemplatePickerAction::Cancel
            } else {
                TemplatePickerAction::None
            };
        };
        match code {
            KeyCode::Char('q') | KeyCode::Esc => TemplatePickerAction::Cancel,
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor = (self.cursor + 1).min(templates.len().saturating_sub(1));
                TemplatePickerAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                TemplatePickerAction::None
            }
            KeyCode::Char(' ') => {
                if let Some(template) = templates.get(self.cursor) {
                    match self.chosen.iter().position(|name| name == &template.name) {
                        Some(index) => {
                            self.chosen.remove(index);
                        }
                        None => self.chosen.push(template.name.clone()),
                    }
                }
                TemplatePickerAction::None
            }
            KeyCode::Enter if self.chosen == self.initial => TemplatePickerAction::Unchanged,
            KeyCode::Enter => TemplatePickerAction::Apply {
                scope: self.scope,
                chosen: self.chosen.clone(),
            },
            _ => TemplatePickerAction::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplatePickerAction {
    None,
    Cancel,
    Unchanged,
    Apply {
        scope: LaunchScope,
        chosen: Vec<String>,
    },
}

/// Order Workspaces for the picker: those holding a live interaction first,
/// then the rest, and within each group the most recently worked in first.
///
/// A live interaction is one the server is still accepting input for, whether
/// it is idle and waiting on the operator or busy with a turn. Those are the
/// Workspaces the operator has work in flight in, so they belong above ones
/// only recency speaks for.
///
/// Recency is the Workspace's most recent interaction — the latest moment any
/// of its Interactions last did something — because that is when work last
/// happened there. Opening the Workspace in a picker is a weaker signal and
/// only speaks for a Workspace whose Interactions say nothing: one with no
/// Interaction on the list at all, or whose Interactions come from a server
/// too old to date them.
fn sort_workspaces(workspaces: &mut [WorkspaceSummary], interactions: &[InteractionSummary]) {
    workspaces.sort_by(|a, b| {
        has_live_interaction(b, interactions)
            .cmp(&has_live_interaction(a, interactions))
            .then_with(|| {
                last_worked_at_ms(b, interactions).cmp(&last_worked_at_ms(a, interactions))
            })
            .then_with(|| b.created_at_ms.cmp(&a.created_at_ms))
    });
}

/// Replace the server's age — how long ago the Workspace was created — with
/// how long ago work last happened in it.
///
/// The column is read against the ordering beside it, and creation date says
/// nothing about that ordering: a Workspace made months ago and worked in this
/// morning sorts at the top reading "94d ago". The recency the rows are sorted
/// by is the one worth a column.
fn label_recency(
    workspaces: &mut [WorkspaceSummary],
    interactions: &[InteractionSummary],
    now: u64,
) {
    for workspace in workspaces {
        workspace.age = humanize_since(now, last_worked_at_ms(workspace, interactions));
    }
}

/// How long ago a moment was, in the server's wording so a row reads the same
/// whichever side of the socket phrased it.
fn humanize_since(now_ms: u64, at_ms: u64) -> String {
    let elapsed_secs = now_ms.saturating_sub(at_ms) / 1000;
    if elapsed_secs < 60 {
        "just now".into()
    } else if elapsed_secs < 3_600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3_600)
    } else {
        format!("{}d ago", elapsed_secs / 86_400)
    }
}

/// When work last happened in a Workspace: its most recent Interaction's
/// activity, falling back to the recorded access when no Interaction of its
/// own dates it. Both readings are epoch milliseconds on the server's clock,
/// so the two kinds of Workspace still compare against each other.
fn last_worked_at_ms(workspace: &WorkspaceSummary, interactions: &[InteractionSummary]) -> u64 {
    interactions
        .iter()
        .filter(|interaction| interaction.workspace_id == workspace.id)
        .map(|interaction| interaction.activity_since_ms)
        .max()
        .filter(|since_ms| *since_ms > 0)
        .unwrap_or(workspace.last_accessed_at_ms)
}

fn has_live_interaction(workspace: &WorkspaceSummary, interactions: &[InteractionSummary]) -> bool {
    interactions.iter().any(|interaction| {
        interaction.activity.accepting() && interaction.workspace_id == workspace.id
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_protocol::{agent::Selection, DrivaOptions, InteractionActivity, SessionSummary};

    fn session(id: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            name: None,
            first_prompt: None,
            tags: Vec::new(),
            workspace_id: "workspace".into(),
            path: PathBuf::from(id),
            selection: Selection::parse("codex").unwrap(),
            age: String::new(),
            created_at_ms: None,
            last_event_at_ms: None,
            last_event_age: String::new(),
            origin: None,
            completed: false,
        }
    }

    fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            auto_retry: false,
            id: id.into(),
            name: None,
            tags: Vec::new(),
            workspace_id: "workspace".into(),
            selection: Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            driva: DrivaOptions {
                isolation_backend: "none".into(),
                command: vec![],
                working_directory: PathBuf::from("/workspace"),
                network: false,
                base: Vec::new(),
                mounts: vec![],
                ..Default::default()
            },
            activity,
            activity_reason: None,
            activity_since_ms: 0,
            idle_unseen: false,
            uncommitted_changes: false,
            last_message: None,
            events: 0,
            completed: false,
        }
    }

    fn interaction_in(
        id: &str,
        workspace_id: &str,
        activity: InteractionActivity,
    ) -> InteractionSummary {
        InteractionSummary {
            workspace_id: workspace_id.into(),
            ..interaction(id, activity)
        }
    }

    fn workspace(id: &str, last_accessed_at_ms: u64) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: None,
            host_path: format!("/home/op/{id}").into(),
            git_repository: None,
            path: format!("/state/workspaces/{id}").into(),
            session_count: 0,
            age: "now".into(),
            created_at_ms: 1,
            last_accessed_at_ms,
            launch: Default::default(),
        }
    }

    fn template(name: &str) -> styra_protocol::TemplateSummary {
        styra_protocol::TemplateSummary {
            name: name.into(),
            description: String::new(),
        }
    }

    #[test]
    fn template_picker_keeps_loading_and_choice_in_root_loop_state() {
        let mut picker = TemplatePicker::loading(
            1,
            "workspace".into(),
            LaunchScope::Workspace,
            vec!["rust".into(), "removed".into()],
        );
        assert_eq!(
            picker.handle_key(KeyCode::Enter),
            TemplatePickerAction::None
        );

        picker.loaded(vec![template("rust"), template("browser")]);
        assert_eq!(picker.chosen, vec!["rust"]);
        assert_eq!(
            picker.handle_key(KeyCode::Enter),
            TemplatePickerAction::Unchanged
        );
        picker.handle_key(KeyCode::Down);
        picker.handle_key(KeyCode::Char(' '));
        assert_eq!(
            picker.handle_key(KeyCode::Enter),
            TemplatePickerAction::Apply {
                scope: LaunchScope::Workspace,
                chosen: vec!["rust".into(), "browser".into()],
            }
        );
    }

    #[test]
    fn session_picker_opens_on_the_current_interaction_when_it_is_listed() {
        let sessions = vec![session("first"), session("current"), session("last")];
        assert_eq!(initial_session_selection(&sessions, Some("current")), 1);
        assert_eq!(initial_session_selection(&sessions, Some("gone")), 0);
    }

    #[test]
    fn session_picker_history_filter_can_be_toggled() {
        let now_ms = 10 * crate::session::RECENT_SESSION_WINDOW_MS;
        let mut fresh = session("fresh");
        fresh.last_event_at_ms = Some(now_ms - crate::session::RECENT_SESSION_WINDOW_MS);
        let mut old = session("old");
        old.last_event_at_ms = Some(now_ms - crate::session::RECENT_SESSION_WINDOW_MS - 1);
        let unknown_age = session("unknown-age");
        let sessions = vec![old, unknown_age, fresh];

        let recent = picker_sessions(
            &sessions,
            false,
            false,
            now_ms,
            SessionOrder::LastActivity,
            None,
        );
        assert_eq!(
            recent
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["fresh", "unknown-age"]
        );
        let all = picker_sessions(
            &sessions,
            true,
            false,
            now_ms,
            SessionOrder::LastActivity,
            None,
        );
        assert_eq!(all.len(), 3);
        let recent_again = picker_sessions(
            &sessions,
            false,
            false,
            now_ms,
            SessionOrder::LastActivity,
            None,
        );
        assert_eq!(recent_again, recent);
    }

    #[test]
    fn session_picker_filter_matches_name_or_first_prompt_case_insensitively() {
        let mut named = session("named");
        named.name = Some("Payments migration".into());
        let mut prompted = session("prompted");
        prompted.first_prompt = Some("Investigate checkout timeout".into());
        let sessions = vec![named, prompted];

        let matches = picker_sessions(
            &sessions,
            true,
            false,
            unix_now_ms(),
            SessionOrder::LastActivity,
            Some("TIMEOUT"),
        );

        assert_eq!(
            matches
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["prompted"]
        );
    }

    #[test]
    fn capital_j_and_k_jump_between_root_conversations() {
        let root = session("root");
        let mut branch = session("branch");
        branch.origin = Some(styra_protocol::SessionOrigin {
            session_id: "root".into(),
            provider: styra_protocol::agent::Provider::Codex,
            at_ms: Some(1),
            history: styra_protocol::BranchHistory::ThroughSelected,
        });
        let other_root = session("other-root");
        let sessions = vec![root, branch, other_root];

        assert_eq!(next_top_level_session(&sessions, 0), Some(2));
        assert_eq!(next_top_level_session(&sessions, 1), Some(2));
        assert_eq!(previous_top_level_session(&sessions, 1), Some(0));
        assert_eq!(previous_top_level_session(&sessions, 2), Some(0));
    }

    #[test]
    fn workspace_filter_matches_the_name_case_insensitively() {
        let mut named = workspace("w-1", 3);
        named.name = Some("Payments API".into());
        let workspaces = vec![named, workspace("billing", 2), workspace("quiet", 1)];

        let by_name = picker_workspaces(&workspaces, Some("payments"));
        assert_eq!(
            by_name.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            vec!["w-1"]
        );

        // An unnamed Workspace is matched on the host directory name its row
        // shows in place of a name.
        let by_directory = picker_workspaces(&workspaces, Some("bill"));
        assert_eq!(
            by_directory
                .iter()
                .map(|w| w.id.as_str())
                .collect::<Vec<_>>(),
            vec!["billing"]
        );

        // The host path beside the name is not searched: only the name is.
        assert!(picker_workspaces(&workspaces, Some("/home/op")).is_empty());
    }

    #[test]
    fn workspace_filter_keeps_the_order_it_narrows() {
        let workspaces = vec![workspace("work-a", 3), workspace("work-b", 2)];

        let filtered = picker_workspaces(&workspaces, Some("work"));

        assert_eq!(
            filtered.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            vec!["work-a", "work-b"]
        );
    }

    #[test]
    fn empty_workspace_filter_shows_every_workspace() {
        let workspaces = vec![workspace("w-1", 2), workspace("w-2", 1)];

        assert_eq!(picker_workspaces(&workspaces, Some("")).len(), 2);
        assert_eq!(picker_workspaces(&workspaces, None).len(), 2);
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
            interaction_in("a", "running", InteractionActivity::Running),
            interaction_in("b", "stopped", InteractionActivity::Stopped),
            interaction_in("c", "idle", InteractionActivity::Pending),
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

    /// What orders the list is when work last happened in a Workspace, not
    /// when the operator last opened one: a Workspace whose agent said
    /// something an hour after it was last opened has to lead the one that was
    /// opened since and then left alone.
    #[test]
    fn workspaces_sort_by_their_most_recent_interaction() {
        let mut workspaces = vec![
            workspace("opened-since", 500),
            workspace("worked-in", 100),
            workspace("never-touched", 300),
        ];
        let interactions = vec![
            InteractionSummary {
                activity_since_ms: 900,
                ..interaction_in("recent", "worked-in", InteractionActivity::Stopped)
            },
            // The Workspace is dated by its *most recent* interaction, so an
            // older sibling does not drag it back down the list.
            InteractionSummary {
                activity_since_ms: 200,
                ..interaction_in("stale", "worked-in", InteractionActivity::Stopped)
            },
            // A server too old to date its interactions says nothing about
            // when this Workspace was worked in; its access still does.
            InteractionSummary {
                activity_since_ms: 0,
                ..interaction_in("undated", "opened-since", InteractionActivity::Stopped)
            },
        ];

        sort_workspaces(&mut workspaces, &interactions);

        assert_eq!(
            workspaces
                .iter()
                .map(|workspace| workspace.id.as_str())
                .collect::<Vec<_>>(),
            ["worked-in", "opened-since", "never-touched"]
        );
    }

    /// The age column is read against the ordering beside it, so it has to
    /// date a Workspace the same way the ordering does: by its most recent
    /// interaction, not by when it was created.
    #[test]
    fn the_age_column_says_how_long_since_work_last_happened() {
        let hour_ms = 3_600_000;
        let mut workspaces = vec![
            WorkspaceSummary {
                created_at_ms: 1,
                ..workspace("worked-in", hour_ms)
            },
            // No interaction dates this one, so its recorded access is all
            // there is to say when it was last used.
            WorkspaceSummary {
                created_at_ms: 1,
                ..workspace("only-opened", 20 * hour_ms)
            },
        ];
        let interactions = vec![InteractionSummary {
            activity_since_ms: 23 * hour_ms,
            ..interaction_in("recent", "worked-in", InteractionActivity::Stopped)
        }];

        label_recency(&mut workspaces, &interactions, 24 * hour_ms);

        assert_eq!(workspaces[0].age, "1h ago");
        assert_eq!(workspaces[1].age, "4h ago");
    }

    /// A Workspace worked in seconds ago should not read "0m ago", and one
    /// dated ahead of this client's clock should not read as a negative age.
    #[test]
    fn work_just_done_and_work_dated_ahead_both_read_as_just_now() {
        let mut workspaces = vec![workspace("fresh", 9_000), workspace("ahead", 60_000)];

        label_recency(&mut workspaces, &[], 10_000);

        assert_eq!(workspaces[0].age, "just now");
        assert_eq!(workspaces[1].age, "just now");
    }
}
