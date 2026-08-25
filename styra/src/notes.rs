//! Operator-authored notes on a Session and on its Workspace: the state they
//! live in, the keys that edit them, and the server calls that persist them.
//!
//! Everything notes-specific outside of rendering lives here, so the rest of
//! the client only has to know three things: [`App`] carries a [`Notes`], the
//! event loop hands keys to [`handle_key`] while [`Notes::is_open`], and the
//! pickers call [`edit_session_notes`]/[`edit_workspace_notes`] on `e`.
//! Rendering is the matching [`crate::ui::notes`] module.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;

use crate::app::App;
use crate::ui;
use styra_server::{Client, InteractionUpdate, LogEntry, SessionSummary, WorkspaceSummary};

/// Which of the two sets of notes the open editor is writing to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Session,
    Workspace,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Session => "Session notes",
            Self::Workspace => "Workspace notes",
        }
    }

    /// The scope `Tab` moves to, which the editor's title advertises.
    pub fn other_label(self) -> &'static str {
        match self {
            Self::Session => "Workspace",
            Self::Workspace => "Session",
        }
    }
}

/// The open notes editor, floating over whatever view the operator opened it
/// from. Both buffers are held at once so `Tab` can move between Session and
/// Workspace notes without dropping unsaved edits to the one being left, and
/// so a single save can persist both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    scope: Scope,
    session: String,
    workspace: String,
    /// False before the first message launches a Session, when there is no
    /// Session for notes to belong to and only the Workspace can be written.
    session_available: bool,
}

impl Editor {
    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn session_available(&self) -> bool {
        self.session_available
    }

    /// The text of the scope being edited.
    pub fn buffer(&self) -> &str {
        match self.scope {
            Scope::Session => &self.session,
            Scope::Workspace => &self.workspace,
        }
    }

    fn buffer_mut(&mut self) -> &mut String {
        match self.scope {
            Scope::Session => &mut self.session,
            Scope::Workspace => &mut self.workspace,
        }
    }
}

/// A Session's notes and its Workspace's, as last read from (or written to)
/// the server, plus the editor while one is open.
///
/// The last-known text is kept even when nothing is open so the view can say
/// that notes exist without the operator having to press `E` to find out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    editor: Option<Editor>,
    session: String,
    workspace: String,
    /// Whether the two fields above have been fetched for this Session yet.
    /// [`ensure_loaded`] fills them on the event loop's first pass and leaves
    /// them alone afterwards; a Session or Workspace switch builds a fresh
    /// [`App`].
    loaded: bool,
}

impl Notes {
    pub fn is_open(&self) -> bool {
        self.editor.is_some()
    }

    pub fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    /// Whether either set of notes has anything in it, which is what the
    /// view's marker reports.
    pub fn any(&self) -> bool {
        !self.session.is_empty() || !self.workspace.is_empty()
    }

    /// Seed the last-known text, as the server's answer does.
    pub fn set_known(&mut self, session: impl Into<String>, workspace: impl Into<String>) {
        self.session = session.into();
        self.workspace = workspace.into();
    }
}

/// Open the notes editor over the current view, on the Session's own notes
/// where there is a Session and on the Workspace's otherwise. It opens on what
/// the server last reported, so opening it is also how notes are read.
pub fn open(app: &mut App) {
    let session_available = !app.session_id.is_empty();
    app.notes.editor = Some(Editor {
        scope: if session_available {
            Scope::Session
        } else {
            Scope::Workspace
        },
        session: app.notes.session.clone(),
        workspace: app.notes.workspace.clone(),
        session_available,
    });
}

/// Keys for the notes editor: printable characters and `Enter` write, `Tab`
/// moves between Session and Workspace notes, `Ctrl+S` persists both, `Esc`
/// closes without saving. Deliberately not `Enter` to save: notes are prose,
/// so a newline is worth more there than a second way to save.
pub fn handle_key(app: &mut App, client: &Client, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.notes.editor = None,
        KeyCode::Tab | KeyCode::BackTab => toggle_scope(app),
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => save(app, client),
        KeyCode::Enter => edit(app, |buffer| buffer.push('\n')),
        KeyCode::Backspace => edit(app, |buffer| {
            buffer.pop();
        }),
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            edit(app, |buffer| buffer.push(ch))
        }
        _ => {}
    }
}

fn edit(app: &mut App, change: impl FnOnce(&mut String)) {
    if let Some(editor) = app.notes.editor.as_mut() {
        change(editor.buffer_mut());
    }
}

/// Move between Session and Workspace notes. Both buffers stay as typed, so
/// this is only a change of which one is on screen.
pub fn toggle_scope(app: &mut App) {
    let Some(editor) = app.notes.editor.as_mut() else {
        return;
    };
    if !editor.session_available {
        app.show_action_message("no Session yet — only Workspace notes can be written");
        return;
    }
    editor.scope = match editor.scope {
        Scope::Session => Scope::Workspace,
        Scope::Workspace => Scope::Session,
    };
}

/// Read the current Session's and Workspace's notes so the view can report
/// that they exist and the editor can open on them without a round trip.
///
/// Marked loaded whichever way it goes: a server that cannot answer this now
/// will not answer it any better once per frame, and notes are not worth
/// refusing to draw the session over.
///
/// The lists are read rather than the by-id lookups: fetching a Workspace
/// records an access on the server, which would silently reorder the Workspace
/// picker every time a Session is opened.
pub fn ensure_loaded(app: &mut App, client: &Client, workspace_id: &str) {
    if app.notes.loaded {
        return;
    }
    app.notes.loaded = true;
    let workspace = app
        .workspace_id
        .clone()
        .unwrap_or_else(|| workspace_id.to_owned());
    let mut session_notes = String::new();
    if !app.session_id.is_empty() {
        match client.list_sessions(&workspace) {
            Ok(sessions) => {
                if let Some(session) = sessions
                    .into_iter()
                    .find(|session| session.id == app.session_id)
                {
                    session_notes = session.notes;
                }
            }
            Err(error) => app.push_log(LogEntry::error(format!(
                "could not read Session notes: {error:#}"
            ))),
        }
    }
    let mut workspace_notes = String::new();
    match client.list_workspaces() {
        Ok(workspaces) => {
            if let Some(found) = workspaces.into_iter().find(|item| item.id == workspace) {
                workspace_notes = found.notes;
            }
        }
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not read Workspace notes: {error:#}"
        ))),
    }
    app.notes.set_known(session_notes, workspace_notes);
}

/// Persist what the editor holds, writing only the scopes whose text actually
/// changed. The editor stays open if a write fails, so the text is still there
/// to retry or copy out of rather than being dropped on the floor.
fn save(app: &mut App, client: &Client) {
    let Some(editor) = app.notes.editor.clone() else {
        return;
    };
    let mut failed = false;
    if editor.session_available && editor.session != app.notes.session {
        match client.update_session_notes(&app.session_id, &editor.session) {
            Ok(summary) => app.notes.session = summary.notes,
            Err(error) => {
                failed = true;
                app.push_log(LogEntry::error(format!(
                    "could not save Session notes: {error:#}"
                )));
            }
        }
    }
    if editor.workspace != app.notes.workspace {
        match app.workspace_id.clone() {
            Some(workspace_id) => {
                match client.update_workspace_notes(&workspace_id, &editor.workspace) {
                    Ok(summary) => app.notes.workspace = summary.notes,
                    Err(error) => {
                        failed = true;
                        app.push_log(LogEntry::error(format!(
                            "could not save Workspace notes: {error:#}"
                        )));
                    }
                }
            }
            None => {
                failed = true;
                app.push_log(LogEntry::warn(
                    "Workspace notes not saved: no Workspace is open",
                ));
            }
        }
    }
    if !failed {
        app.notes.editor = None;
        app.show_action_message("notes saved");
    }
}

/// Edit the highlighted Session's notes from the Session picker, replacing that
/// row with what the server stored.
pub fn edit_session_notes(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    sessions: &mut [SessionSummary],
    selected: usize,
    updates: &[InteractionUpdate],
) -> Result<()> {
    let Some(notes) = prompt(
        terminal,
        Scope::Session,
        &sessions[selected].notes,
        |frame| ui::render_picker(frame, sessions, selected, ui::Preview::Ready(updates)),
    )?
    else {
        return Ok(());
    };
    sessions[selected] = client.update_session_notes(&sessions[selected].id, &notes)?;
    Ok(())
}

/// The same from the Workspace picker. The picker's own liveness and Session
/// preview are passed through so the backdrop behind the editor stays the
/// screen the operator opened it from.
pub fn edit_workspace_notes(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    client: &Client,
    workspaces: &mut [WorkspaceSummary],
    selected: usize,
    interactions: &[styra_server::InteractionSummary],
    sessions: &[styra_server::SessionSummary],
) -> Result<()> {
    let Some(notes) = prompt(
        terminal,
        Scope::Workspace,
        &workspaces[selected].notes,
        |frame| {
            ui::render_workspace_picker(
                frame,
                workspaces,
                selected,
                interactions,
                ui::SessionsPreview::Ready(sessions),
            )
        },
    )?
    else {
        return Ok(());
    };
    workspaces[selected] = client.update_workspace_notes(&workspaces[selected].id, &notes)?;
    Ok(())
}

/// The pickers' own notes editor: the same `Ctrl+S`/`Esc` terms as the main
/// view's, drawn over the picker it was opened from. It runs its own loop
/// because the pickers are blocking loops of their own rather than [`App`]
/// state.
fn prompt<F>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    scope: Scope,
    initial: &str,
    mut render_background: F,
) -> Result<Option<String>>
where
    F: FnMut(&mut ratatui::Frame),
{
    let mut value = initial.to_owned();
    loop {
        terminal.draw(|frame| {
            render_background(frame);
            ui::render_notes_prompt(frame, scope, &value);
        })?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Some(value))
            }
            KeyCode::Enter => value.push('\n'),
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(ch) if !ch.is_control() => value.push(ch),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys;
    use crate::session::Live;
    use crossterm::event::KeyEvent;

    fn app() -> App {
        App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        )
    }

    /// No server is listening on this path, so every call through it fails —
    /// which is all these tests need, since they only exercise the state.
    fn client() -> Client {
        Client::new("/nonexistent/styra.sock")
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        handle_key(app, &client(), KeyEvent::new(code, modifiers));
    }

    /// The editor opens on what the server last reported, and typing into it
    /// only touches the scope on screen — the other buffer keeps its text so a
    /// round trip through `Tab` loses nothing.
    #[test]
    fn the_notes_editor_edits_one_scope_at_a_time_and_keeps_the_other() {
        let mut app = app();
        app.notes.set_known("session", "workspace");

        open(&mut app);
        press(&mut app, KeyCode::Char('!'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);

        let editor = app.notes.editor().expect("editor open");
        assert_eq!(editor.scope(), Scope::Workspace);
        assert_eq!(editor.session, "session!");
        assert_eq!(editor.workspace, "workspac\nx");
        // Nothing is persisted until a save lands, so the last-known text is
        // still what the view reports.
        assert_eq!(app.notes.session, "session");
        assert_eq!(app.notes.workspace, "workspace");
    }

    #[test]
    fn cancelling_the_notes_editor_discards_what_was_typed() {
        let mut app = app();
        app.notes.set_known("kept", "");
        open(&mut app);
        press(&mut app, KeyCode::Char('z'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert!(!app.notes.is_open());
        assert_eq!(app.notes.session, "kept");
    }

    /// `E` opens the editor from the main view, and printable keys typed into
    /// it are note text — including the ones that are shortcuts in the view
    /// underneath.
    #[test]
    fn e_opens_the_editor_and_printable_keys_become_note_text() {
        let mut app = app();
        let mut pending_fold = false;
        let mut live = Live::Viewing;
        keys::handle_list_key(
            &mut app,
            &client(),
            &mut live,
            KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT),
            &mut pending_fold,
            std::path::Path::new("/nonexistent/styra-preferences"),
        );
        assert!(app.notes.is_open());

        for ch in ['q', '?'] {
            press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        assert_eq!(app.notes.editor().unwrap().buffer(), "q?");
    }

    /// A save that cannot reach the server leaves the editor open, so the text
    /// is still there to retry rather than lost.
    #[test]
    fn a_failed_save_keeps_the_editor_and_its_text() {
        let mut app = app();
        open(&mut app);
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);

        assert_eq!(app.notes.editor().expect("still open").buffer(), "x");
        assert!(app.notes.session.is_empty());
    }

    /// With no Session launched there is nothing for Session notes to belong
    /// to, so the editor opens on the Workspace and stays there.
    #[test]
    fn a_pending_session_edits_workspace_notes_only() {
        let mut app = App::pending(styra_server::agent::Selection::parse("codex").unwrap());
        open(&mut app);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);

        let editor = app.notes.editor().expect("editor open");
        assert_eq!(editor.scope(), Scope::Workspace);
        assert!(!editor.session_available());
    }
}
