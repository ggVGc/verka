//! Handing a key press to whichever window owns it.
//!
//! Modal windows are asked first, in order of which can be opened over
//! which; the first that is open takes the key. Only once none has does the
//! key reach the interaction navigator, and through it the focused window.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use super::navigation::{
    narrow_navigator_to_jump, open_interactions_in_workspace, preview_workspace_jump,
    select_workspace_in_grid,
};
use super::screen::blocked_on;
use super::tags::handle_tag_picker_key;
use super::{Loop, RunOutcome};
use crate::action_menu::{self, ActionMenu};
use crate::app::{App, Focus, Request, View};
use crate::input;
use crate::keybindings as keys;
use crate::launch;
use crate::launcher;
use crate::picker;
use crate::preferences;
use crate::presentation;
use crate::session;
use crate::workspace_jump;
use styra_protocol::CompletionState;
use styra_ui::query::Edit;

/// What became of a key a window was offered.
enum Handled {
    /// The window took it; the round is over.
    Done,
    /// The window took it, and it ends the loop.
    Exit(RunOutcome),
    /// The window left it for the ones beneath.
    Pass,
}

/// The message box owns paste only when no inner prompt or chooser owns input.
pub(super) fn message_paste_active(app: &App) -> bool {
    app.focus == Focus::Input
        && !app.help.is_open()
        && app.rebase_status.is_none()
        && app.launcher.is_none()
        && app.template_picker.is_none()
        && app.retarget_prompt.is_none()
        && app.integration_prompt.is_none()
        && app.branch_prompt.is_none()
        && app.link_menu.is_none()
        && app.action_menu.is_none()
        && app.workspace_jump.is_none()
        && app.insert.is_none()
        && app.git_repository_prompt.is_none()
        && app.launch.prompt.is_none()
        && app.launch.command_prompt.is_none()
        && app.tag_picker.is_none()
        && !app.diff_focused()
        && !app.search.typing()
}

/// Whether Space opens the action menu: on the list of one of the session's
/// windows, with nothing modal over it and nothing being typed. A highlighted
/// link keeps Space for its own menu.
fn action_menu_available(app: &App) -> bool {
    app.focus == Focus::List
        && app.launcher.is_none()
        && app.template_picker.is_none()
        && app.retarget_prompt.is_none()
        && app.integration_prompt.is_none()
        && app.branch_prompt.is_none()
        && app.link_menu.is_none()
        && app.link_highlight.is_none()
        && app.workspace_jump.is_none()
        && app.insert.is_none()
        && app.git_repository_prompt.is_none()
        && app.launch.prompt.is_none()
        && app.launch.command_prompt.is_none()
        && app.tag_picker.is_none()
        && !app.diff_focused()
        && !app.search.typing()
        && !app.interactions.typing_filter()
        && presentation::current_window(app).has_action_menu()
}

/// `?` describes whichever window is showing, so every key-driven one
/// answers it — the launcher, the template chooser and the modal overlays
/// included. The prompts that take typed text are excluded: there a `?` is a
/// character of what is being typed.
fn reference_available(app: &App) -> bool {
    app.insert.is_none()
        && app.git_repository_prompt.is_none()
        && app.retarget_prompt.is_none()
        && app.integration_prompt.is_none()
        && app.launch.prompt.is_none()
        && app.launch.command_prompt.is_none()
        && !app.search.typing()
        // The diff owns the keyboard; `?` is not one of its keys.
        && !app.diff_focused()
        && app
            .tag_picker
            .as_ref()
            .is_none_or(|picker| picker.new_tag.is_none())
        && (app.focus == Focus::List || app.launcher.is_some() || app.template_picker.is_some())
}

/// Global actions which operate on the current interaction without dismissing
/// its navigator. They fall through to the ordinary list-key handler below.
/// `n` and Ctrl-N are among them: stepping keeps the list open on the
/// interaction it lands on, while Ctrl-N starts a new interaction.
pub(super) fn interaction_navigator_passthrough(key: KeyEvent) -> bool {
    keys::GLOBAL_FOCUS_MESSAGE.matches(key)
        || keys::GLOBAL_DIFF.matches(key)
        || keys::GLOBAL_REBASE.matches(key)
        || keys::GLOBAL_STOP.matches(key)
        || keys::GLOBAL_NEXT_LIVE.matches(key)
        || keys::GLOBAL_NEW_SESSION.matches(key)
        || keys::GLOBAL_INTERACTION_HISTORY.matches(key)
        // The jump narrows the navigator to the Workspaces it offers, so the
        // navigator stays open under it.
        || keys::GLOBAL_WORKSPACE_JUMP.matches(key)
        || keys::GLOBAL_WORKSPACE_JUMP_LISTED.matches(key)
}

impl Loop<'_> {
    pub(super) fn handle_key(
        &mut self,
        key: KeyEvent,
        workspace_id: &str,
    ) -> Result<Option<RunOutcome>> {
        match self.modal_key(key, workspace_id)? {
            Handled::Done => {}
            Handled::Exit(outcome) => return Ok(Some(outcome)),
            Handled::Pass => self.window_key(key, workspace_id),
        }
        Ok(None)
    }

    /// Offer the key to every modal window, in order of precedence, and then
    /// to the interaction navigator.
    fn modal_key(&mut self, key: KeyEvent, workspace_id: &str) -> Result<Handled> {
        let app = &mut *self.app;
        let client = self.client;
        // While the reference is open it is modal, so none of the commands
        // described by it can accidentally act on the window underneath. It
        // comes before every other modal because it can be opened over them.
        if app.help.is_open() {
            reference_key(app, key);
            return Ok(Handled::Done);
        }
        if let Some(status) = &app.rebase_status {
            if status.finished && matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                app.rebase_status = None;
            }
            return Ok(Handled::Done);
        }
        if keys::HELP.matches(key) && reference_available(app) {
            app.help.open();
            return Ok(Handled::Done);
        }
        // The comparison picker owns typed text and navigation while open.
        if app.diff_branch_picker.is_some() {
            self.diff_branch_picker_key(key);
            return Ok(Handled::Done);
        }
        // The action menu is typed at, so while it is open it owns every key
        // but the `?` above. What it runs is pressed back through this loop,
        // so a command chosen from it takes the same path as its own key.
        if let Some(menu) = app.action_menu.as_mut() {
            match menu.handle_key(key) {
                action_menu::Outcome::Stay => {}
                action_menu::Outcome::Cancel => app.action_menu = None,
                action_menu::Outcome::Run(presses) => {
                    app.action_menu = None;
                    self.replay.extend(presses);
                }
            }
            return Ok(Handled::Done);
        }
        // Typed at too, so it owns the keys the same way.
        if app.workspace_jump.is_some() {
            return self.workspace_jump_key(key);
        }
        if keys::GLOBAL_ACTION_MENU.matches(key) && action_menu_available(app) {
            // A `z` pressed before Space would otherwise swallow the first key
            // the menu runs.
            self.pending_fold = false;
            app.action_menu = Some(ActionMenu::new(presentation::current_window(app)));
            return Ok(Handled::Done);
        }
        // Template discovery and choice are part of this root loop. Even the
        // loading state is cancellable, and no nested event loop can defer the
        // edit produced by Enter until some unrelated future keypress.
        if app.template_picker.is_some() {
            self.template_picker_key(key, workspace_id);
            return Ok(Handled::Done);
        }
        // The branching choice is modal and can replace the current Session
        // on confirmation, so it owns the key before every underlying view.
        if app.retarget_prompt.is_some() {
            input::handle_retarget_prompt_key(app, client, key);
            return Ok(Handled::Done);
        }
        if app.integration_prompt.is_some() {
            self.integration_prompt_key(key)?;
            return Ok(Handled::Done);
        }
        if app.branch_prompt.is_some() {
            input::handle_branch_prompt_key(app, client, key);
            // Confirming has already branched; open the result on this key
            // rather than leaving the switch queued behind the next one.
            return Ok(match app.take_open_session_request() {
                Some(Request::OpenSession(id)) => Handled::Exit(RunOutcome::OpenSession(id)),
                Some(Request::OpenBranch(id)) => Handled::Exit(RunOutcome::OpenBranch(id)),
                _ => Handled::Done,
            });
        }
        // The link menu is modal over link navigation, so it owns the keys
        // that would otherwise move the highlight or open the link directly.
        if app.link_menu.is_some() {
            input::handle_link_menu_key(app, key);
            return Ok(Handled::Done);
        }
        // The message editor's path prompt is modal, and its second question is
        // answered by a bare letter that means something else everywhere else,
        // so it is handled ahead of the reference.
        if app.insert.is_some() {
            input::handle_insert_key(app, key);
            return Ok(Handled::Done);
        }
        // The Git checkout is Workspace metadata. Its prompt is modal so a
        // path such as `git@host:group/project` is input rather than keys.
        if app.git_repository_prompt.is_some() {
            input::handle_git_repository_prompt_key(app, client, key);
            return Ok(Handled::Done);
        }
        // The project command prompt owns typed characters too.
        if app.launch.command_prompt.is_some() {
            input::handle_command_prompt_key(app, key);
            return Ok(Handled::Done);
        }
        // So is the Driva view's mount prompt: what is typed into it is part
        // of a path, including the characters that are shortcuts elsewhere.
        if app.launch.prompt.is_some() {
            input::handle_mount_prompt_key(app, key);
            // Confirming a Workspace mount closes the prompt and emits an
            // effect. It must leave on this key, not sit behind the next one.
            self.submit_workspace_launch_now(workspace_id);
            return Ok(Handled::Done);
        }
        if app.tag_picker.is_some() {
            handle_tag_picker_key(app, client, key);
            return Ok(Handled::Done);
        }
        // The diff owns reading keys in either layout, including its modal
        // file filter, before the event list and navigator handle them.
        if input::handle_diff_focus_key(app, key) {
            return Ok(Handled::Done);
        }
        if app.diff_focused() {
            self.diff_key(key);
            return Ok(Handled::Done);
        }
        if app.search.typing() {
            input::handle_search_key(app, key);
            return Ok(Handled::Done);
        }
        if app.interactions.open && app.focus == Focus::List {
            return self.navigator_key(key);
        }
        Ok(Handled::Pass)
    }

    /// A key no modal window took goes to the launcher, or else to whichever
    /// of the list and the message box has focus.
    ///
    /// The launcher raises requests of its own (applying a model change to
    /// the live session), which the next round carries out.
    fn window_key(&mut self, key: KeyEvent, workspace_id: &str) {
        if self.app.launcher.is_some() {
            launcher::handle_key(self.app, key, self.preferences_path);
            return;
        }
        if keys::GLOBAL_LAUNCHER.matches(key) || keys::EDITOR_LAUNCHER.matches(key) {
            self.app.recent_models = preferences::load_recent_models(self.preferences_path);
        }
        // Grid navigation shares the navigator's pending cursor. Finish
        // its move before a command can act on the previous interaction.
        if self.app.focus == Focus::List
            && self.app.view == View::Overview
            && !input::overview_navigation_key(key)
        {
            self.load_cursored_interaction();
        }
        match self.app.focus {
            Focus::List => input::handle_list_key(
                self.app,
                self.client,
                self.live,
                key,
                &mut self.pending_fold,
                self.preferences_path,
            ),
            Focus::Input => self.message_key(key, workspace_id),
        }
    }

    fn message_key(&mut self, key: KeyEvent, workspace_id: &str) {
        let app = &mut *self.app;
        let client = self.client;
        // A running capture owns the message box and its keys: there is no
        // text being typed for them to mean anything else to, and the
        // operator is speaking rather than looking for a modifier.
        if self.audio.is_recording() {
            match key {
                k if keys::EDITOR_RECORD_FINISH.matches(k) || keys::EDITOR_RECORD.matches(k) => {
                    self.audio.finish(app, client.clone())
                }
                k if keys::EDITOR_RECORD_CANCEL.matches(k) => {
                    self.audio.cancel(app, client.clone())
                }
                k if keys::EDITOR_RECORD_LOUDER.matches(k) => self.audio.boost(app),
                k if keys::EDITOR_RECORD_QUIETER.matches(k) => self.audio.quieten(app),
                _ => {}
            }
            return;
        }
        if keys::EDITOR_RECORD.matches(key) {
            self.audio.toggle(app, client.clone());
            return;
        }
        input::handle_input_key(app, client, workspace_id, self.live, key)
    }

    /// A Workspace launch edit a modal produced is sent now, before the
    /// round goes back to drawing, rather than waiting for another key.
    fn submit_workspace_launch_now(&mut self, workspace_id: &str) {
        if let Some(Request::ChangeWorkspaceLaunch {
            change,
            clear_interaction,
        }) = self.app.take_workspace_launch_request()
        {
            self.launch_effects.submit_workspace_launch(
                self.app,
                workspace_id.to_owned(),
                change,
                clear_interaction,
            );
        }
    }

    fn diff_branch_picker_key(&mut self, key: KeyEvent) {
        let app = &mut *self.app;
        let client = self.client;
        let Some(picker) = app.diff_branch_picker.as_mut() else {
            return;
        };
        match picker.handle_key(key) {
            crate::checkout_diff::BranchChoice::Stay => {}
            crate::checkout_diff::BranchChoice::Cancel => app.diff_branch_picker = None,
            crate::checkout_diff::BranchChoice::Select(branch) => {
                let id = picker.id.clone();
                let result = client.set_diff_branch(&id, branch).and_then(|()| {
                    let interaction = app
                        .interactions
                        .items
                        .iter()
                        .find(|row| row.id == id)
                        .ok_or_else(|| anyhow::anyhow!("no interaction to diff"))?;
                    crate::checkout_diff::target(client, interaction)
                });
                match result {
                    Ok(target) => {
                        app.checkout_diff = Some(styra_diff::CheckoutDiff::read(target));
                        app.diff_turn = false;
                        app.diff_branch_picker = None;
                        self.edit_reports.load(app, client);
                    }
                    Err(error) => {
                        app.show_error(format!("could not change comparison branch: {error:#}"))
                    }
                }
            }
        }
    }

    fn workspace_jump_key(&mut self, key: KeyEvent) -> Result<Handled> {
        let app = &mut *self.app;
        let Some(jump) = app.workspace_jump.as_mut() else {
            return Ok(Handled::Pass);
        };
        let outcome = jump.handle_key(key);
        let grid = match outcome {
            workspace_jump::Outcome::Stay => {
                preview_workspace_jump(app);
                narrow_navigator_to_jump(app);
                return Ok(Handled::Done);
            }
            _ => app.workspace_jump.take().and_then(|jump| jump.grid),
        };
        narrow_navigator_to_jump(app);
        match (outcome, grid) {
            (workspace_jump::Outcome::Cancel, Some((overview, only_current_workspace))) => {
                app.overview = overview;
                app.interactions.only_current_workspace = only_current_workspace;
            }
            // Over the grid the choice stays on the grid: its cursor is
            // already on the Workspace, and loads the tile as a step would.
            (workspace_jump::Outcome::Open(id), Some(_)) => {
                if !select_workspace_in_grid(app, &id) {
                    app.show_warning("the grid's filters hide that Workspace's interactions");
                }
                input::follow_overview_cursor(app);
            }
            // The chosen Workspace's first interaction is loaded outright,
            // so the list can be narrowed to it: Workspace scope follows
            // the Workspace on screen. A load that fails leaves the list in
            // All, where the cursor's rest is still listed.
            (workspace_jump::Outcome::Open(id), None) => {
                open_interactions_in_workspace(app, self.client, &id)?;
                self.load_cursored_interaction();
                if self.app.workspace.id.as_deref() == Some(id.as_str()) {
                    self.app.interactions.only_current_workspace = true;
                }
            }
            // A fresh interaction is asked for outright, as the Workspace
            // picker's Ctrl-N asks for one. The summary the jump was built
            // from stands in if the server cannot answer.
            (workspace_jump::Outcome::New(id), _) => {
                let workspace = self.client.workspace(&id).or_else(|error| {
                    app.interactions
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.id == id)
                        .cloned()
                        .ok_or(error)
                })?;
                return Ok(Handled::Exit(RunOutcome::OpenWorkspace {
                    workspace: Box::new(workspace),
                    session_id: None,
                    open_interactions: false,
                }));
            }
            _ => {}
        }
        Ok(Handled::Done)
    }

    fn template_picker_key(&mut self, key: KeyEvent, workspace_id: &str) {
        let app = &mut *self.app;
        let Some(template_picker) = app.template_picker.as_mut() else {
            return;
        };
        match template_picker.handle_key(key) {
            picker::TemplatePickerAction::None => {}
            picker::TemplatePickerAction::Cancel => app.template_picker = None,
            picker::TemplatePickerAction::Unchanged => {
                app.template_picker = None;
                app.show_action_message(
                    "templates unchanged — Space toggles the highlighted template",
                );
            }
            picker::TemplatePickerAction::Apply { scope, chosen } => {
                app.template_picker = None;
                launch::set_templates_for(app, scope, chosen);
            }
        }
        // An Apply can have queued a Workspace effect.
        self.submit_workspace_launch_now(workspace_id);
    }

    fn integration_prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        let client = self.client;
        input::handle_integration_prompt_key(self.app, key);
        let Some(request) = self.app.take_request() else {
            return Ok(());
        };
        let id = self.app.session_id.clone();
        match request {
            Request::GenerateIntegrationMessage => {
                let generated = blocked_on(
                    self.terminal,
                    self.app,
                    "generating commit message…",
                    |_| client.generate_integration_message(&id),
                )?;
                match generated {
                    Ok(message) => self.app.integration_prompt = Some(message),
                    Err(error) => self
                        .app
                        .show_error(format!("could not generate commit message: {error:#}")),
                }
            }
            Request::Integrate(message) => match client.integrate_session(&id, &message) {
                Ok(()) => {
                    self.app.close_integration_prompt();
                    self.app.pending_integration = Some(id.clone());
                    self.app.show_action_message(
                        "integration started — progress appears in the interaction log",
                    );
                }
                Err(error) => self
                    .app
                    .show_error(format!("could not integrate: {error:#}")),
            },
            _ => {}
        }
        Ok(())
    }

    /// A key for the focused diff, which owns the keyboard.
    fn diff_key(&mut self, key: KeyEvent) {
        let app = &mut *self.app;
        if keys::DIFF_BRANCH.matches(key) && !app.shown_diff().is_some_and(|diff| diff.typing()) {
            match self.client.diff_branches(&app.session_id) {
                Ok(branches) => {
                    app.diff_branch_picker = Some(crate::checkout_diff::BranchPicker::new(
                        app.session_id.clone(),
                        branches,
                    ))
                }
                Err(error) => {
                    app.show_error(format!("could not list comparison branches: {error:#}"))
                }
            }
            return;
        }
        input::handle_diff_key(app, key);
    }

    /// The embedded interaction list owns navigation while it is open.
    /// Moving its cursor makes that interaction current once the cursor
    /// rests, so the list can be walked across faster than interactions can
    /// be loaded. Enter only closes the navigator; there is no preview.
    fn navigator_key(&mut self, key: KeyEvent) -> Result<Handled> {
        if navigator_cursor_key(self.app, key) {
            return Ok(Handled::Done);
        }
        // Unbound keys leave the navigator and its pending cursor alone.
        // Events and global shortcuts still act on the selected row.
        let mapped = [keys::Window::Interactions, keys::Window::Events]
            .into_iter()
            .flat_map(keys::Window::actions)
            .any(|binding| binding.matches(key))
            || keys::EVENTS_FOLD_PREFIX.matches(key);
        if !mapped {
            return Ok(Handled::Done);
        }
        // Every other mapped key acts on the row under the cursor, so a move
        // still waiting out its settle is completed first rather than
        // abandoned.
        let chosen = self
            .app
            .interactions
            .cursor(&self.app.session_id)
            .to_owned();
        self.load_cursored_interaction();
        let app = &mut *self.app;
        match key {
            k if keys::GLOBAL_DIFF.matches(k) && app.diff_visible() => {
                app.diff_preview = false;
                app.diff_interaction_focus = false;
                app.diff_turn = false;
            }
            k if keys::GLOBAL_DIFF.matches(k) => {
                // Opening the pane is a display choice even if loading this
                // interaction's diff fails. Keep it for the next row.
                app.diff_preview = true;
                app.diff_interaction_focus = true;
                return Ok(Handled::Pass);
            }
            k if keys::INTERACTIONS_CLOSE.matches(k) => {
                if k.code == KeyCode::Enter && chosen == app.session_id {
                    self.history.record(&app.session_id);
                }
                app.interactions.close();
            }
            k if keys::INTERACTIONS_SCOPE.matches(k) => app.interactions.toggle_workspace_scope(),
            k if keys::INTERACTIONS_COMPLETED.matches(k) => app.interactions.toggle_completed(),
            k if keys::INTERACTIONS_STOPPED.matches(k) => app.interactions.toggle_stopped(),
            k if keys::INTERACTIONS_ABANDON.matches(k) => {
                return self.finish_from_navigator(CompletionState::Abandoned)
            }
            k if keys::INTERACTIONS_COMPLETE.matches(k) => {
                return self.finish_from_navigator(CompletionState::Completed)
            }
            k if keys::INTERACTIONS_TAGS.matches(k) => {
                let Some(interaction) = app.interactions.current(&app.session_id) else {
                    return Ok(Handled::Done);
                };
                match self.client.list_tags() {
                    Ok(tags) => {
                        app.tag_picker = Some(crate::tag_picker::TagPicker::new(
                            tags,
                            interaction.tags.clone(),
                        ))
                    }
                    Err(error) => app.show_error(format!("could not list tags: {error:#}")),
                }
            }
            k if interaction_navigator_passthrough(k) => return Ok(Handled::Pass),
            _ => {
                app.interactions.close();
                return Ok(Handled::Pass);
            }
        }
        Ok(Handled::Done)
    }

    /// Complete or abandon the interaction under the navigator's cursor, then
    /// move on to the row the list shows next.
    fn finish_from_navigator(&mut self, finished: CompletionState) -> Result<Handled> {
        let Some(interaction) = self.app.interactions.current(&self.app.session_id).cloned() else {
            return Ok(Handled::Done);
        };
        // The cursor settle already made this row the one on screen, so
        // finishing it is the same action — and the same local status update
        // — as the `C` and `Z` bound directly on the Events view.
        if !session::finish_interaction(self.app, self.client, self.live, finished) {
            return Ok(Handled::Done);
        }
        // Completion is a Session property the server owns, so the row is
        // still listed and the server is what says so: take its word for the
        // new state rather than writing one here.
        if let Ok(interactions) = self
            .client
            .list_interactions(&self.app.interactions_query())
        {
            self.app.interactions.refresh(interactions);
        }
        let workspace_id = self.app.workspace.id.clone();
        let Some(next) = self
            .app
            .interactions
            .select_past_hidden(&interaction.id, workspace_id.as_deref())
        else {
            self.app.interactions.close();
            return Ok(Handled::Exit(RunOutcome::Reset));
        };
        if next.id != interaction.id {
            self.make_interaction_current(next);
        }
        Ok(Handled::Done)
    }
}

fn reference_key(app: &mut App, key: KeyEvent) {
    match key {
        k if keys::CLOSE_REFERENCE.matches(k) => app.help.close(),
        // The reference is taller than a short terminal, so the
        // sections at the end have to be reachable.
        k if keys::REFERENCE_DOWN.matches(k) => app.help.line_down(),
        k if keys::REFERENCE_UP.matches(k) => app.help.line_up(),
        k if keys::REFERENCE_PAGE_DOWN.matches(k) => app.help.page_down(),
        k if keys::REFERENCE_PAGE_UP.matches(k) => app.help.page_up(),
        k if keys::REFERENCE_TOP.matches(k) => app.help.scroll_to_top(),
        _ => {}
    }
}

/// The navigator's keys that only move its cursor, filter it or change how
/// it is drawn — none of which loads anything. Reports whether `key` was one.
fn navigator_cursor_key(app: &mut App, key: KeyEvent) -> bool {
    let session_id = app.session_id.clone();
    let workspace_id = app.workspace.id.clone();
    let workspace_id = workspace_id.as_deref();
    let interactions = &mut app.interactions;
    // The `/` filter is modal while it is being typed, as the event list's
    // search is: every printable key is part of the term. The arrows still
    // walk what it leaves standing. Enter ends the typing and goes on to open
    // the row under the cursor, as Enter on the list does, rather than
    // needing a second press.
    if interactions.typing_filter() && key.code == KeyCode::Enter {
        interactions.finish_filter();
    } else if interactions.typing_filter() {
        match key.code {
            KeyCode::Esc => interactions.clear_filter(),
            KeyCode::Down => interactions.cursor_next(&session_id, workspace_id),
            KeyCode::Up => interactions.cursor_previous(&session_id, workspace_id),
            KeyCode::PageDown => interactions.cursor_page(&session_id, workspace_id, true),
            KeyCode::PageUp => interactions.cursor_page(&session_id, workspace_id, false),
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    interactions.type_filter(edit, &session_id, workspace_id)
                }
            }
        }
        return true;
    }
    match key {
        k if keys::INTERACTIONS_FILTER.matches(k) => interactions.start_filter(),
        // Chosen from tags carried by the current live snapshot, with the
        // filter in force already marked.
        k if keys::INTERACTIONS_TAG_FILTER.matches(k) => {
            app.tag_picker = Some(crate::tag_picker::TagPicker::filter(
                interactions.live_tags(),
                interactions.tag_filter().to_vec(),
            ))
        }
        // Esc widens a filtered list back out before it closes it.
        k if k.code == KeyCode::Esc && interactions.filter().is_some() => {
            interactions.clear_filter()
        }
        // In All scope the entries are grouped under Workspace headings, and
        // J/K skip whole groups: one press per Workspace rather than one per
        // interaction. They move the cursor like j/k, so a skip across
        // several groups costs no more loads than a step across one row.
        // This skips idle work, so it only ever lands on an interaction
        // currently doing work.
        k if keys::INTERACTIONS_NEXT_WORKING.matches(k) => {
            if interactions
                .cursor_to_next_active(&session_id, workspace_id)
                .is_none()
            {
                app.show_warning("no other interaction is actively working");
            }
        }
        k if keys::INTERACTIONS_PREV_WORKING.matches(k) => {
            if interactions
                .cursor_to_previous_active(&session_id, workspace_id)
                .is_none()
            {
                app.show_warning("no other interaction is actively working");
            }
        }
        // Only how the rows are drawn changes, so the cursor's pending move
        // need not be loaded first.
        k if keys::INTERACTIONS_RESPONSES.matches(k) => interactions.toggle_responses(),
        k if keys::INTERACTIONS_NEXT_WORKSPACE.matches(k) => {
            interactions.cursor_next_workspace(&session_id, workspace_id)
        }
        k if keys::INTERACTIONS_PREV_WORKSPACE.matches(k) => {
            interactions.cursor_previous_workspace(&session_id, workspace_id)
        }
        k if keys::INTERACTIONS_NEXT.matches(k) => {
            interactions.cursor_next(&session_id, workspace_id)
        }
        k if keys::INTERACTIONS_PREV.matches(k) => {
            interactions.cursor_previous(&session_id, workspace_id)
        }
        k if keys::INTERACTIONS_FIRST.matches(k) => {
            interactions.cursor_first(&session_id, workspace_id)
        }
        k if keys::INTERACTIONS_PAGE_DOWN.matches(k) => {
            interactions.cursor_page(&session_id, workspace_id, true)
        }
        k if keys::INTERACTIONS_PAGE_UP.matches(k) => {
            interactions.cursor_page(&session_id, workspace_id, false)
        }
        k if keys::INTERACTIONS_LAST.matches(k) => {
            interactions.cursor_last(&session_id, workspace_id)
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn message_paste_follows_editor_focus_and_modal_ownership() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.focus = Focus::List;
        assert!(!message_paste_active(&app));
        app.focus = Focus::Input;
        assert!(message_paste_active(&app));
        app.retarget_prompt = Some((String::new(), Vec::new()));
        assert!(!message_paste_active(&app));
        app.retarget_prompt = None;
        app.integration_prompt = Some(String::new());
        assert!(!message_paste_active(&app));
        app.integration_prompt = None;
        app.insert = Some(crate::insert::Prompt::new(None, None, false));
        assert!(!message_paste_active(&app));
        app.insert = None;
        app.help.open();
        assert!(!message_paste_active(&app));
        app.help.close();
        app.composer.add_part();
        app.composer.start_choosing();
        assert!(message_paste_active(&app));
        app.diff_preview = true;
        assert!(app.diff_visible());
        assert!(message_paste_active(&app));
        app.enter_list();
        assert!(!message_paste_active(&app));
    }

    /// Space is a letter wherever something is being typed and a link's own
    /// menu while one is highlighted, so the action menu answers it only on a
    /// window's list.
    #[test]
    fn the_action_menu_opens_only_from_a_windows_list() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.focus = Focus::List;
        assert!(action_menu_available(&app));
        app.focus = Focus::Input;
        assert!(!action_menu_available(&app));
        app.focus = Focus::List;
        app.search.open();
        assert!(!action_menu_available(&app));
        app.search.cancel();
        app.view = View::CheckoutDiff;
        assert!(!action_menu_available(&app), "the diff owns the keyboard");
        app.view = View::Overview;
        assert!(action_menu_available(&app));
        app.integration_prompt = Some(String::new());
        assert!(!action_menu_available(&app));
    }

    #[test]
    fn project_command_prompts_own_spaces_and_question_marks() {
        for action in styra_protocol::ProjectAction::ALL {
            let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
            app.view = View::Driva;
            app.focus = Focus::List;
            crate::launch::open_command_prompt(&mut app, action);
            assert!(!action_menu_available(&app));
            for character in "cargo run -- --help ?".chars() {
                input::handle_command_prompt_key(
                    &mut app,
                    KeyEvent::from(KeyCode::Char(character)),
                );
            }
            assert_eq!(
                app.launch.command_prompt.as_ref().unwrap().1,
                "cargo run -- --help ?"
            );
            app.focus = Focus::Input;
            assert!(!message_paste_active(&app));
        }
    }

    #[test]
    fn global_actions_pass_through_the_navigator() {
        let press = |character| KeyEvent::from(KeyCode::Char(character));
        assert!(interaction_navigator_passthrough(press('S')));
        assert!(interaction_navigator_passthrough(press('i')));
        assert!(interaction_navigator_passthrough(KeyEvent::new(
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
        )));
        assert!(!interaction_navigator_passthrough(press('l')));
    }
}
