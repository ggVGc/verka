//! Input handling and action dispatch for the terminal client.
//!
//! Key assignments live in [`crate::keybindings`]. This module owns the behavior
//! performed after a key has been recognized: prompts, view dispatch, and
//! actions that touch application state or the server.

use crate::keybindings::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::path::Path;

use crate::app::{App, Request, View};
use crate::branch_picker::{BranchChoice, BranchPicker};
use crate::insert;
use crate::launch;
use crate::link_menu::LinkAction;
use crate::preferences;
use crate::session::{self, Attachment};
use styra_diff::Outcome;
use styra_protocol::{CompletionState, Contract, ProjectAction};
use styra_server::Client;
use styra_ui::fuzzy_list::FuzzyList;
use styra_ui::query::{Edit, Words};

/// Keys for the event list's `/` search prompt. It is modal — every printable
/// key is part of the term, including the letters bound to commands on the
/// list underneath — so the event loop routes keys here ahead of the view.
///
/// Enter hands the keys back with the term still marked; Esc leaves the list
/// unmarked, as does backspacing the term away.
pub fn handle_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.search.cancel(),
        KeyCode::Enter => app.search.accept(),
        _ => {
            if let Some(edit) = Edit::from_key(key) {
                app.search.edit(edit);
            }
        }
    }
}

pub fn handle_rename_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some(text) = app.rename_prompt.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Esc => app.rename_prompt = None,
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.ask(Request::GenerateSessionName)
        }
        KeyCode::Enter => {
            let name = (!text.trim().is_empty()).then_some(text.as_str());
            match client.rename_session(&app.session_id, name) {
                Ok(session) => {
                    app.interactions.note_name(session.id, session.name);
                    app.rename_prompt = None;
                }
                Err(error) => app.show_error(format!("could not rename interaction: {error:#}")),
            }
        }
        _ => match Edit::from_key(key) {
            Some(Edit::Push(_)) if text.chars().count() >= 80 => {}
            Some(edit) => edit.apply(text),
            None => {}
        },
    }
}

/// A key for the open branch picker. A base is only remembered for the
/// launch; a retarget or a return to a previous branch is done at once.
pub fn handle_branch_picker_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some(picker) = app.branch_picker.as_mut() else {
        return;
    };
    let choice = picker.handle_key(key);
    if !matches!(choice, BranchChoice::Stay) {
        app.branch_picker = None;
    }
    match choice {
        BranchChoice::Stay | BranchChoice::Cancel => {}
        BranchChoice::Base(branch) => app.base_branch = branch,
        BranchChoice::Retarget(branch) => app.ask(Request::Retarget(branch)),
        BranchChoice::Restore(branch) => {
            match client.restore_session_branch(&app.session_id, &branch) {
                Ok(()) => app.show_action_message(format!("Returned to {branch}")),
                Err(error) => app.show_warning(format!("Could not restore branch: {error:#}")),
            }
        }
    }
}

pub fn handle_integration_prompt_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.close_integration_prompt(),
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.ask(Request::GenerateIntegrationMessage)
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            if let Some(message) = app.integration_prompt.as_mut() {
                message.push('\n');
            }
        }
        KeyCode::Enter => {
            if let Some(message) = app
                .integration_prompt
                .as_ref()
                .filter(|text| !text.trim().is_empty())
            {
                app.ask(Request::Integrate(message.clone()));
            }
        }
        _ => {
            if let (Some(edit), Some(message)) =
                (Edit::from_key(key), app.integration_prompt.as_mut())
            {
                edit.apply_with(message, Words::Prose);
            }
        }
    }
}

/// Keys for the checkout diff. The diff owns the keyboard while it is open:
/// `styra-diff` answers every key, the same way its own program does, and
/// `i` starts a message about the file and line the diff is on and `q` quits
/// Styra unless the diff's filter is being typed.
/// `u` switches between the checkout's diff and the selected turn's, `D` lets
/// the diff take the whole view and back, and Tab hands navigation to the
/// interaction while the log is shown. Other global bindings and the other
/// views' keys do not act from it.
pub fn handle_diff_key(app: &mut App, key: KeyEvent) {
    // The commit browser owns its navigation and Enter before host actions.
    if let Some(diff) = app.shown_diff_mut() {
        if diff.commits.open
            && (diff.commits.focused || key.code == KeyCode::Tab)
            && diff.handle_key(key) == Outcome::Handled
        {
            return;
        }
    }
    if handle_diff_focus_key(app, key) {
        return;
    }
    let typing_filter = app.shown_diff().is_some_and(|diff| diff.typing());
    if key.code == KeyCode::Enter
        && !typing_filter
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && app.jump_to_diff_prompt()
    {
        return;
    }
    if DIFF_TURN_HIGHLIGHT.matches(key) && !typing_filter {
        app.diff_turn_highlight = !app.diff_turn_highlight;
        app.diff_turn = false;
        app.sync_turn_diff();
        return;
    }
    if DIFF_FILES_BELOW.matches(key) && !typing_filter {
        app.diff_files_below = !app.diff_files_below;
        return;
    }
    if DIFF_MAXIMIZE.matches(key) && !typing_filter {
        // The log it hides cannot hold the keys.
        app.diff_maximized = !app.diff_maximized;
        app.diff_interaction_focus = false;
        return;
    }
    if DIFF_TURN.matches(key) && !typing_filter {
        app.diff_turn = !app.diff_turn;
        app.sync_turn_diff();
        return;
    }
    if app.turn_diff_shown()
        && !typing_filter
        && (DIFF_NEXT_TURN.matches(key) || DIFF_PREV_TURN.matches(key))
    {
        app.select_diff_turn(DIFF_NEXT_TURN.matches(key));
        return;
    }
    if GLOBAL_QUIT.matches(key) && !typing_filter {
        app.ask(Request::Quit);
        return;
    }
    if GLOBAL_FOCUS_MESSAGE.matches(key) && !typing_filter {
        app.enter_input_from_diff();
        return;
    }
    let outcome = match app.shown_diff_mut() {
        Some(diff) => diff.handle_key(key),
        None if matches!(key.code, KeyCode::Esc | KeyCode::Char('d'))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Outcome::Close
        }
        None => Outcome::Ignored,
    };
    match outcome {
        Outcome::Handled | Outcome::Ignored => {}
        Outcome::Close | Outcome::Toggle => {
            app.diff_preview = false;
            app.diff_interaction_focus = false;
            app.diff_turn = false;
        }
        Outcome::OpenFile => open_diff_file(app),
        Outcome::GenerateReport | Outcome::DescribeFile(_) if app.turn_diff_shown() => {
            app.show_warning("edit reports describe the checkout's diff: u shows it")
        }
        Outcome::GenerateReport => app.ask(Request::GenerateEditReport(None)),
        Outcome::DescribeFile(path) => app.ask(Request::GenerateEditReport(Some(path))),
    }
}

/// Tab exchanges the side diff's reading keys with the interaction pane.
/// Prompts retain their keys.
pub fn handle_diff_focus_key(app: &mut App, key: KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        || app.focus != crate::app::Focus::List
        || !app.diff_preview
        || !matches!(app.view, View::Events | View::Files)
        || (app.diff_maximized && app.view == View::Events)
        || app.search.typing()
        || app.interactions.typing_filter()
        || app
            .shown_diff()
            .is_some_and(|diff| diff.typing() || (diff.commits.open && !app.diff_interaction_focus))
    {
        return false;
    }
    app.diff_interaction_focus = !app.diff_interaction_focus;
    app.entry_log.unfocus();
    true
}

/// Keys for the driva view's "add a mount" prompt. It is modal — every
/// printable key is part of the path being typed, `?` included — so the event
/// loop routes keys here ahead of the keybind reference and every view and
/// global binding.
pub fn handle_mount_prompt_key(app: &mut App, key: KeyEvent) {
    // The prompt's text can be set without a key, so the list is brought up to
    // date with it before a key acts on the list, and again after the key has
    // changed it.
    launch::refresh_paths(app);
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Tab => launch::take_highlighted_path(app),
        KeyCode::Esc => launch::cancel_prompt(app),
        KeyCode::Enter => launch::confirm_prompt(app),
        KeyCode::Down => launch::move_path_cursor(app, FuzzyList::next),
        KeyCode::Char('n') if control => launch::move_path_cursor(app, FuzzyList::next),
        KeyCode::Up => launch::move_path_cursor(app, FuzzyList::prev),
        KeyCode::Char('p') if control => launch::move_path_cursor(app, FuzzyList::prev),
        KeyCode::PageDown => launch::move_path_cursor(app, FuzzyList::page_down),
        KeyCode::PageUp => launch::move_path_cursor(app, FuzzyList::page_up),
        // Shift-Tab climbs back out of the directory Tab went into.
        KeyCode::BackTab => delete_prompt_path_part(app),
        KeyCode::Char('w') if control => delete_prompt_path_part(app),
        _ => {
            if let (Some(edit), Some(text)) = (Edit::from_key(key), app.launch.prompt.as_mut()) {
                edit.apply(text);
            }
        }
    }
    launch::refresh_paths(app);
}

fn delete_prompt_path_part(app: &mut App) {
    if let Some(text) = app.launch.prompt.as_mut() {
        delete_path_part(text);
    }
}

/// Remove the last path component without erasing an absolute path's root.
/// This is deliberately path-oriented rather than readline's word-oriented:
/// a mount prompt begins with a filesystem path, and `/work/project` becomes
/// `/work/` after `Ctrl-W`. The parent keeps its `/` so the list under the
/// prompt shows what is in it, `project` among the rest.
fn delete_path_part(path: &mut String) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path.truncate(1);
        return;
    }
    match trimmed.rfind('/') {
        Some(slash) => path.truncate(slash + 1),
        None => path.clear(),
    }
}

/// Keys for the driva view's command prompt. Modal, like the mount prompt:
/// a shell command is made of the characters that are shortcuts elsewhere.
pub fn handle_command_prompt_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => launch::cancel_command_prompt(app),
        KeyCode::Enter => launch::confirm_command_prompt(app),
        _ => {
            if let (Some(edit), Some((_, text))) =
                (Edit::from_key(key), app.launch.command_prompt.as_mut())
            {
                edit.apply_with(text, Words::Prose);
            }
        }
    }
}

/// Keys for the Workspace Git-checkout prompt. This is durable Workspace
/// metadata, rather than an individual sandbox grant.
pub fn handle_git_repository_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.git_repository_prompt = None,
        KeyCode::Enter => {
            let Some(path) = app.git_repository_prompt.take() else {
                return;
            };
            let Some(workspace_id) = app.workspace.id.as_deref() else {
                return app.show_warning("no Workspace is selected");
            };
            let repository = (!path.trim().is_empty()).then(|| Path::new(path.trim()));
            match client.set_workspace_git_repository(workspace_id, repository) {
                Ok(workspace) => {
                    app.show_workspace(&workspace);
                    app.show_action_message(if repository.is_some() {
                        "Git checkout associated for future launches"
                    } else {
                        "Git checkout association cleared"
                    });
                }
                Err(error) => app.show_error(format!("could not set Git checkout: {error:#}")),
            }
        }
        _ => {
            if let (Some(edit), Some(text)) =
                (Edit::from_key(key), app.git_repository_prompt.as_mut())
            {
                edit.apply(text);
            }
        }
    }
}

/// The branch composer shares editing and launch controls with normal input.
pub fn handle_branch_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some(prompt) = app.branch_prompt.as_mut() else {
        return;
    };
    if BRANCH_CHANGE.matches(key) {
        prompt.change();
    } else if handle_composer_key(&mut prompt.composer, key) {
    } else if GLOBAL_LEAVE_MESSAGE.matches(key) || EDITOR_BRANCH.matches(key) {
        leave_branch_prompt(app);
    } else if EDITOR_SEND.matches(key) || EDITOR_SEND_IN_BRANCH.matches(key) {
        let message = prompt.composer.message();
        if message.is_empty() {
            return;
        }
        let at_ms = prompt.at_ms();
        let history = prompt.history();
        // A message carried over from the main box keeps what it was started
        // from in the diff.
        let message = if prompt.from_input {
            app.with_message_source(message)
        } else {
            message
        };
        let checkout = if EDITOR_SEND_IN_BRANCH.matches(key) {
            styra_protocol::BranchCheckout::NewWorktree
        } else {
            styra_protocol::BranchCheckout::ShareSource
        };
        session::branch_session(app, client, at_ms, history, checkout, message);
    } else {
        handle_editor_controls(app, key);
    }
}

/// Close the branch box. A message carried over from the main message box
/// goes back to it, so switching to a branch and back loses nothing.
fn leave_branch_prompt(app: &mut App) {
    let Some(mut prompt) = app.branch_prompt.take() else {
        return;
    };
    if prompt.from_input {
        prompt.composer.hand_over(&mut app.composer);
    }
}

/// Carry the message being typed over to a branch from the selected entry.
fn switch_to_branch_prompt(app: &mut App) {
    session::open_branch_prompt(app);
    if let Some(prompt) = app.branch_prompt.as_mut() {
        app.composer.hand_over(&mut prompt.composer);
        prompt.from_input = true;
    }
}

/// Keys for the menu of actions on the highlighted link. Taking an action
/// closes the menu; cancelling it leaves the link highlighted.
pub fn handle_link_menu_key(app: &mut App, key: KeyEvent) {
    let Some(menu) = app.link_menu.as_mut() else {
        return;
    };
    match key {
        k if LINK_MENU_NEXT.matches(k) => menu.select_next(),
        k if LINK_MENU_PREV.matches(k) => menu.select_previous(),
        k if LINK_MENU_CONFIRM.matches(k) => {
            let action = menu.selected();
            app.link_menu = None;
            match action {
                LinkAction::Open => app.open_highlighted_link(),
                LinkAction::Mount { writable } => mount_highlighted_link(app, writable),
            }
        }
        k if LINK_MENU_CANCEL.matches(k) => app.link_menu = None,
        _ => {}
    }
}

/// Add the highlighted file to this interaction's own mounts, the layer the
/// message editor's path prompt grants into, for the same reason: the path came
/// up in this conversation. The link stays highlighted, so the operator can go
/// on to open it or mount the next one.
fn mount_highlighted_link(app: &mut App, writable: bool) {
    let path = match app.highlighted_link_target() {
        Some((_, Ok(path), _)) => path,
        Some((_, Err(problem), _)) => return app.show_error(problem),
        None => return app.show_warning("selected link names no file to mount"),
    };
    if !app.allow_mount_edit() {
        return;
    }
    // Driva canonicalizes a mount source, so the grant is recorded in the
    // same terms and compared against the sandbox's mounts in them.
    let source = match std::fs::canonicalize(&path) {
        Ok(source) => source,
        Err(error) => return app.show_error(format!("{}: {error}", path.display())),
    };
    let mounts = app
        .launch
        .driva
        .as_ref()
        .map(styra_protocol::DrivaOptions::plain_mounts);
    if let Some(visible) = mounts
        .as_deref()
        .and_then(|mounts| crate::mount::visibility(mounts, &source))
    {
        return app.show_warning(format!(
            "already in the sandbox at {} ({})",
            visible.path.display(),
            visible.access
        ));
    }
    let mount = styra_protocol::LaunchMount {
        source,
        destination: None,
        writable,
    };
    crate::launch::grant_interaction_mount(app, mount);
}

/// List the local branches of the repository the new interaction would launch
/// in, for choosing the one its checkout starts from.
fn open_base_branch_picker(app: &mut App) {
    let Some(root) = app.workspace.root().map(Path::to_path_buf) else {
        return app.show_warning("no Workspace directory to list branches in");
    };
    match crate::branch_picker::local_branches(&root) {
        Ok(branches) if branches.is_empty() => {
            app.show_warning("this Workspace's repository has no local branches")
        }
        Ok(branches) => {
            let default = crate::presentation::default_source_branch(app);
            app.branch_picker = Some(BranchPicker::base(
                default.as_deref(),
                branches,
                app.base_branch.as_deref(),
            ));
        }
        Err(error) => app.show_warning(format!("could not list branches: {error:#}")),
    }
}

/// List the branches the current interaction can be moved to: those it
/// worked on before, to return to, then every other local branch of its
/// repository, to retarget onto. Both are the server's to read, since the
/// interaction's checkout is.
fn open_retarget_picker(app: &mut App, client: &Client) {
    let history = match client.session_branch_history(&app.session_id) {
        Ok(history) => history,
        Err(error) => return app.show_warning(format!("Could not read branch history: {error:#}")),
    };
    let branches = match client.diff_branches(&app.session_id) {
        Ok(branches) => branches,
        Err(error) => return app.show_warning(format!("Could not list branches: {error:#}")),
    };
    let current = app
        .interactions
        .current(&app.session_id)
        .and_then(|interaction| interaction.checkout.as_ref())
        .and_then(|checkout| checkout.branch.clone());
    let previous: Vec<String> = history.into_iter().map(|saved| saved.branch).collect();
    let picker = BranchPicker::retarget(&previous, branches, current.as_deref());
    if picker.rows.is_empty() {
        return app.show_warning("no other branch to retarget onto");
    }
    app.branch_picker = Some(picker);
}

pub fn handle_list_key(
    app: &mut App,
    client: &Client,
    live: &mut Attachment,
    key: KeyEvent,
    pending_fold: &mut bool,
    preferences_path: &Path,
) {
    if handle_diff_focus_key(app, key) {
        return;
    }
    if app.diff_focused() {
        return handle_diff_key(app, key);
    }
    if std::mem::take(pending_fold) {
        match key {
            k if EVENTS_EXPAND_ALL.matches(k) => app.timeline.expand_all(),
            k if EVENTS_COLLAPSE_ALL.matches(k) => app.timeline.collapse_all(),
            _ => {}
        }
        return;
    }
    if app.view == View::Overview && handle_overview_key(app, key) {
        return;
    }
    let window = crate::presentation::view_window(app.view);
    match key {
        // A view's own command on a key wins over the global one on the same
        // key — details keeps `D`, `w` and `I`, files `e` and `a`, and so on.
        // The reference and the action menu ask the same question, so neither
        // offers a global command its key would not reach here.
        k if window.claims(k) => {}
        k if GLOBAL_QUIT.matches(k) => return app.ask(Request::Quit),
        // The event list is the bottom of the stack every other view is
        // opened over, so Esc goes straight back to it from any of them. The
        // modals that claim Esc for themselves (prompts, pickers, the
        // navigator, the reference) are answered before this is reached. A
        // link highlight is cleared first, as it is on the event list.
        k if GLOBAL_BACK.matches_in(k, window) => {
            if app.link_highlight.is_some() {
                return app.clear_link_highlight();
            }
            app.set_view(View::Events);
            return;
        }
        k if GLOBAL_INTERRUPT.matches(k) => {
            return session::interrupt_interaction(app, client, live)
        }
        k if GLOBAL_STOP.matches(k) => return session::pause_interaction(app, client, live),
        k if GLOBAL_BRANCH.matches(k) => return session::open_branch_prompt(app),
        // The same key as retarget: before the first message there is no
        // branch to move, only the one the checkout is going to start from.
        k if GLOBAL_BASE_BRANCH.matches(k) && app.session_id.is_empty() => {
            return open_base_branch_picker(app)
        }
        k if GLOBAL_RETARGET.matches(k) && !app.session_id.is_empty() => {
            return open_retarget_picker(app, client)
        }
        k if GLOBAL_REBASE.matches(k) && !app.session_id.is_empty() => {
            return app.ask(Request::Rebase);
        }
        k if GLOBAL_INTEGRATE.matches(k) && !app.session_id.is_empty() => {
            app.open_integration_prompt();
            return;
        }
        k if GLOBAL_SHELL.matches(k) => {
            let Attachment::Attached { .. } = live else {
                return app.show_warning("no live interaction to open a shell for");
            };
            return app.ask(Request::OpenShell);
        }
        // Beside `!`, and deliberately not the same shell: `!` attaches to the
        // agent's sandbox, `~` opens the operator's own shell on the host,
        // standing where the interaction is working. That works with no live
        // interaction — a finished one still has a directory to look at.
        k if GLOBAL_DIRECTORY.matches(k) => return app.ask(Request::OpenDirectory),
        // `d` opens the diff pane, or closes it from the log as from the pane.
        // A highlighted link is a place to show in the diff, so it still opens
        // the diff there.
        k if GLOBAL_DIFF.matches(k) && app.diff_visible() && app.link_highlight.is_none() => {
            app.diff_preview = false;
            app.diff_interaction_focus = false;
            app.diff_turn = false;
            return;
        }
        k if GLOBAL_DIFF.matches(k) => return ask_for_diff(app),
        // Not from the preview. The overview takes `i` itself, for the tile
        // under its cursor.
        k if GLOBAL_FOCUS_MESSAGE.matches_in(k, window) => return app.enter_input(),
        // Global, unlike `y`: what it copies is the session's exchange, which
        // does not change with the view the operator happens to be in.
        k if GLOBAL_COPY_CONVERSATION.matches(k) => return copy_conversation(app),
        k if GLOBAL_MESSAGES.matches(k) => return app.toggle_view(View::Messages),
        k if GLOBAL_RAW.matches(k) => return app.toggle_raw(),
        k if GLOBAL_LOG.matches(k) => return app.toggle_view(View::Log),
        k if GLOBAL_LAUNCHER.matches(k) => return app.open_launcher(),
        // Opening the view also refreshes it: the log lives in the daemon's
        // memory, so there is nothing local to show without asking.
        k if GLOBAL_QUOTA.matches(k) => {
            app.toggle_view(View::Quota);
            return app.ask(Request::Quota);
        }
        // Only the server can see the sandbox, so opening asks it; the event
        // loop then asks again each second while the view stays open.
        k if GLOBAL_PROCESSES.matches(k) => {
            app.toggle_view(View::Processes);
            return app.ask(Request::Processes);
        }
        k if GLOBAL_PROCESSES_PANE.matches(k) => {
            app.toggle_processes_pane();
            return app.ask(Request::Processes);
        }
        k if GLOBAL_TRANSCRIPT.matches(k) => return app.toggle_view(View::Transcript),
        // `e` toggles the pane below the event list. The files and answer
        // views keep the key for opening the editor.
        k if GLOBAL_ENTRY_LOG.matches(k) => return app.toggle_entry_log(),
        k if GLOBAL_DETAILS.matches(k) => return app.toggle_view(View::Driva),
        k if GLOBAL_JUMP_BACK.matches(k) => return app.ask(Request::JumpBack),
        k if GLOBAL_JUMP_FORWARD.matches(k) => return app.ask(Request::JumpForward),
        k if GLOBAL_INTERACTION_HISTORY.matches(k) => return app.ask(Request::InteractionHistory),
        k if GLOBAL_AUTO_COMMIT.matches(k) => return toggle_auto_commit(app),
        k if GLOBAL_FILES.matches(k) || GLOBAL_FILES_ALIAS.matches(k) => return app.toggle_files(),
        k if GLOBAL_ANSWER.matches(k) => return app.toggle_answer(),
        k if GLOBAL_PREVIEW.matches(k) => return app.toggle_view(View::Preview),
        k if GLOBAL_NEW_SESSION.matches(k) => return app.ask(Request::NewSession),
        k if GLOBAL_INTERACTIONS.matches(k) => return app.ask(Request::Interactions),
        k if GLOBAL_OVERVIEW.matches(k) => return app.toggle_overview(),
        k if GLOBAL_WORKSPACES.matches(k) => return app.ask(Request::Workspace),
        k if GLOBAL_WORKSPACE_JUMP.matches(k) => {
            return app.ask(Request::WorkspaceJump { listed: false })
        }
        k if GLOBAL_WORKSPACE_JUMP_LISTED.matches(k) => {
            return app.ask(Request::WorkspaceJump { listed: true })
        }
        k if GLOBAL_WORKTREES.matches(k) => return app.ask(Request::Worktrees),
        k if GLOBAL_TAGS.matches(k) => return edit_current_interaction_tags(app, client),
        k if GLOBAL_SESSION_WORKTREE.matches(k) && !app.session_id.is_empty() => {
            return app.ask(Request::CreateWorktree { message: None })
        }
        k if GLOBAL_SESSIONS.matches(k) => return app.ask(Request::Sessions),
        // Newly idle work needs attention first; without one, `n` walks every
        // interaction still running.
        k if GLOBAL_NEXT_LIVE.matches(k) => return app.ask(Request::NextLiveInteraction),
        k if GLOBAL_NEXT_WORKING.matches(k) => return app.ask(Request::NextWorkingInteraction),
        _ => {}
    }
    match app.view {
        View::Events => match key {
            k if EVENTS_RENAME.matches(k) && !app.session_id.is_empty() => {
                app.rename_prompt = Some(
                    app.interactions
                        .name(&app.session_id)
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
            k if EVENTS_RUN.matches(k) => app.ask(Request::ProjectAction(ProjectAction::Run)),
            k if EVENTS_TEST.matches(k) => app.ask(Request::ProjectAction(ProjectAction::Test)),
            k if EVENTS_PARENT.matches(k) => session::open_parent(app),
            k if EVENTS_LINKS.matches(k) => app.highlight_first_link(),
            k if EVENTS_SEARCH.matches(k) => app.search.open(),
            // A search that stands after the prompt has closed is cleared
            // where it is being read, rather than by reopening the prompt in
            // order to cancel it.
            k if k.code == KeyCode::Esc && app.link_highlight.is_some() => {
                app.clear_link_highlight()
            }
            k if k.code == KeyCode::Esc && app.search.query().is_some() => app.search.cancel(),
            k if EVENTS_LINK_DESTINATIONS.matches(k) => app.toggle_link_display(),
            k if EVENTS_ALL_EVENTS.matches(k) => app.toggle_all_events(),
            k if EVENTS_PREVIEW_TARGET.matches(k) && app.preview.open => {
                app.preview.toggle_target()
            }
            // Same key as the live-interactions navigator's `C`, and the same
            // action: finish the interaction on screen without first having to
            // open the navigator to find the row for it. Guarded so it does
            // not steal the preview pane's own `C`, which claims the key while
            // that pane is open.
            k if EVENTS_COMPLETE.matches(k) && !app.preview.open => {
                session::finish_interaction(app, client, live, CompletionState::Completed);
            }
            k if EVENTS_ABANDON.matches(k) => {
                session::finish_interaction(app, client, live, CompletionState::Abandoned);
            }
            // The Events screen shows two windows when the entry-log pane is
            // open, and Tab is what moves the navigation keys between them.
            k if EVENTS_ENTRY_LOG_FOCUS.matches(k) && app.entry_log.open => {
                app.toggle_entry_log_focus()
            }
            // With the pane holding the keys, the movement keys walk its
            // entries and the preview follows them. The list stands still, so
            // the operator keeps their place in it.
            k if (EVENTS_NEXT_LINE.matches(k) || k.code == KeyCode::Down)
                && app.entry_log.focused() =>
            {
                app.entry_log_select_next()
            }
            k if (EVENTS_PREV_LINE.matches(k) || k.code == KeyCode::Up)
                && app.entry_log.focused() =>
            {
                app.entry_log_select_prev()
            }
            k if EVENTS_FIRST.matches(k) && app.entry_log.focused() => app.entry_log_select_first(),
            k if EVENTS_LAST.matches(k) && app.entry_log.focused() => app.entry_log_select_last(),
            // A focused pane scrolls by moving its cursor: the pane always
            // shows where the cursor is, so paging the offset on its own would
            // be undone by the next draw.
            k if EVENTS_PAGE_DOWN.matches(k) && app.entry_log.focused() => {
                app.entry_log_page_down()
            }
            k if EVENTS_PAGE_UP.matches(k) && app.entry_log.focused() => app.entry_log_page_up(),
            k if EVENTS_PAGE_DOWN.matches(k) && app.preview.open && !app.diff_visible() => {
                app.preview.scroll.half_page_down()
            }
            k if EVENTS_PAGE_UP.matches(k) && app.preview.open && !app.diff_visible() => {
                app.preview.scroll.half_page_up()
            }
            // With the preview open the arrows read it, the way `j`/`k` do the
            // full-screen one; `j`/`k` still step through the list.
            k if EVENTS_PREVIEW_SCROLL_DOWN.matches(k)
                && app.preview.open
                && !app.diff_visible() =>
            {
                app.preview.scroll.page_down()
            }
            k if EVENTS_PREVIEW_SCROLL_UP.matches(k) && app.preview.open && !app.diff_visible() => {
                app.preview.scroll.page_up()
            }
            k if EVENTS_PAGE_DOWN.matches(k) && app.entry_log.open => {
                app.entry_log.scroll.page_down()
            }
            k if EVENTS_PAGE_UP.matches(k) && app.entry_log.open => app.entry_log.scroll.page_up(),
            k if EVENTS_SCROLL_DOWN.matches(k) => app.scroll_interaction_down(),
            k if EVENTS_SCROLL_UP.matches(k) => app.scroll_interaction_up(),
            k if EVENTS_NEXT_LINE.matches(k) && app.link_highlight.is_some() => {
                app.highlight_next_link()
            }
            k if EVENTS_PREV_LINE.matches(k) && app.link_highlight.is_some() => {
                app.highlight_prev_link()
            }
            k if EVENTS_NEXT_LINE.matches(k) => app.select_next_line(),
            k if EVENTS_PREV_LINE.matches(k) => app.select_prev_line(),
            k if k.code == KeyCode::Enter && app.link_highlight.is_some() => {
                app.open_highlighted_link()
            }
            k if EVENTS_LINK_MENU.matches(k) && app.link_highlight.is_some() => {
                app.open_link_menu()
            }
            // Branch markers are reciprocal links between the source and its
            // child Session. Enter follows either direction; all other
            // entries retain Enter's usual fold/unfold behavior.
            k if k.code == KeyCode::Enter
                && app
                    .timeline
                    .selected_entry()
                    .is_some_and(|entry| entry.event().branch_target().is_some()) =>
            {
                session::follow_branch(app)
            }
            k if EVENTS_TOGGLE_EXPAND.matches(k) => app.timeline.toggle_expand(),
            k if EVENTS_EXPAND_ONLY.matches(k) => app.timeline.expand_only_selected(),
            k if EVENTS_FIRST.matches(k) => app.select_first(),
            k if EVENTS_LAST.matches(k) => app.select_last(),
            k if EVENTS_FOLD_PREFIX.matches(k) => *pending_fold = true,
            k if EVENTS_MINOR.matches(k) => app.toggle_minor(),
            k if EVENTS_PREVIEW_PANEL.matches(k) => app.preview.toggle(),
            k if EVENTS_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Raw => match key {
            k if READING_PROVIDER_RAW.matches(k) => toggle_provider_raw(app, client),
            k if READING_PAGE_DOWN.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().preview.page_down()
            }
            k if READING_PAGE_UP.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().preview.page_up()
            }
            k if READING_DOWN.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_next()
            }
            k if READING_UP.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_prev()
            }
            k if READING_FIRST.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_first()
            }
            k if READING_LAST.matches(k) && app.provider_raw_open => {
                app.provider_raw.as_mut().unwrap().select_last()
            }
            k if READING_PAGE_DOWN.matches(k) => app.raw.preview.page_down(),
            k if READING_PAGE_UP.matches(k) => app.raw.preview.page_up(),
            k if READING_DOWN.matches(k) => app.raw.select_next(),
            k if READING_UP.matches(k) => app.raw.select_prev(),
            k if READING_FIRST.matches(k) => app.raw.select_first(),
            k if READING_LAST.matches(k) => app.raw.select_last(),
            k if READING_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Messages => match key {
            k if READING_PAGE_DOWN.matches(k) => app.messages.preview.page_down(),
            k if READING_PAGE_UP.matches(k) => app.messages.preview.page_up(),
            k if READING_DOWN.matches(k) => app.messages.select_next(),
            k if READING_UP.matches(k) => app.messages.select_prev(),
            k if READING_FIRST.matches(k) => app.messages.select_first(),
            k if READING_LAST.matches(k) => app.messages.select_last(),
            k if READING_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Log => match key {
            k if READING_DOWN.matches(k) => app.log.scroll_down(),
            k if READING_UP.matches(k) => app.log.scroll_up(),
            k if READING_FIRST.matches(k) => app.log.scroll_to_top(),
            k if READING_LAST.matches(k) => app.log.scroll_to_bottom(),
            _ => {}
        },
        // `R` for retry, in the view that shows the limit and the minute it
        // resets. It belongs here rather than among the global bindings
        // because that is where an operator whose session has just been cut
        // off is already looking: the notice saying the window is exhausted is
        // the reason they pressed `Q`.
        View::Quota => match key {
            k if READING_RETRY.matches(k) => app.ask(Request::SetAutoRetry(!app.auto_retry)),
            k if READING_DOWN.matches(k) => app.quota.scroll_down(),
            k if READING_UP.matches(k) => app.quota.scroll_up(),
            _ => {}
        },
        View::Processes => match key {
            k if READING_DOWN.matches(k) => app.processes.select_next(),
            k if READING_UP.matches(k) => app.processes.select_prev(),
            k if READING_FIRST.matches(k) => app.processes.select_first(),
            k if READING_LAST.matches(k) => app.processes.select_last(),
            k if READING_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Transcript => match key {
            k if READING_LINKS.matches(k) => app.highlight_first_link(),
            k if READING_ALL_EVENTS.matches(k) => app.toggle_all_events(),
            k if READING_DOWN.matches(k) => app.transcript.line_down(),
            k if READING_UP.matches(k) => app.transcript.line_up(),
            k if READING_FIRST.matches(k) => app.transcript.reset(),
            k if READING_LAST.matches(k) => app.transcript.scroll_to_end(),
            _ => {}
        },
        // Editing the launch policy. These keys deliberately avoid the letters
        // the global bindings above already claim (`t`, `n`, `d`, …), since
        // reaching the transcript or a new session from this view must keep
        // working while the policy is being edited.
        //
        // Every editing key acts on whichever of the two layers the up/down
        // arrows focus, so there is one set of them to learn rather than one per
        // layer — and the view says which layer that is.
        View::Driva => match key {
            k if DRIVA_TAB.matches(k) => app.details_tab = app.details_tab.other(),
            // Git checkout association is Workspace metadata, so it does not
            // depend on which policy pane happens to be focused.
            k if DRIVA_GIT_CHECKOUT.matches(k) => {
                app.git_repository_prompt = Some(
                    app.workspace
                        .git_repository
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                )
            }
            k if DRIVA_SCOPE.matches(k) => launch::toggle_scope(app),
            k if DRIVA_NETWORK.matches(k) => launch::cycle_network(app),
            // `R` for read-only: the workspace mount's access. Lowercase `r`
            // is claimed globally above (the raw view) and never gets here.
            k if DRIVA_ACCESS.matches(k) => launch::cycle_workspace_access(app),
            // `I` for whether this launch inherits: `S` is claimed globally
            // above (stopping the interaction) and never reaches this match.
            k if DRIVA_IGNORE_WORKSPACE.matches(k) => launch::toggle_ignore_workspace(app),
            k if DRIVA_TEMPLATES.matches(k) => {
                if app.allow_launch_edit() {
                    app.ask(Request::Templates);
                }
            }
            k if DRIVA_ADD_MOUNT.matches(k) => launch::open_prompt(app),
            k if DRIVA_RUN_COMMAND.matches(k) => {
                launch::open_command_prompt(app, ProjectAction::Run)
            }
            k if DRIVA_TEST_COMMAND.matches(k) => {
                launch::open_command_prompt(app, ProjectAction::Test)
            }
            k if DRIVA_REMOVE_MOUNT.matches(k) => launch::remove_selected_mount(app),
            // Mirrors `D` in the launch picker: keep this policy as the one a
            // brand-new client starts from, rather than only this session's.
            // Only this interaction's own settings are saved — the Workspace's
            // are already durable, and saving the merge would make every launch
            // elsewhere carry grants meant for this Workspace.
            k if DRIVA_SAVE_DEFAULT.matches(k) => {
                if app.allow_launch_edit() {
                    let launch = app.launch.interaction.clone();
                    match preferences::save_launch(preferences_path, &launch) {
                        Ok(()) => app.show_action_message(
                            "saved this interaction's settings as the default for new clients",
                        ),
                        Err(error) => app.report_error(format!(
                            "could not save the default launch policy: {error:#}"
                        )),
                    }
                }
            }
            // Move what this interaction added up into the Workspace's standing
            // policy, once it turns out not to be particular to this
            // conversation after all.
            k if DRIVA_PROMOTE.matches(k) => launch::promote_to_workspace(app),
            k if DRIVA_NEXT_MOUNT.matches(k) => launch::select_next_mount(app),
            k if DRIVA_PREV_MOUNT.matches(k) => launch::select_prev_mount(app),
            k if DRIVA_PAGE_DOWN.matches(k) => {
                if app.details_tab == crate::app::DetailsTab::Details {
                    app.details_scroll.page_down()
                } else {
                    app.launch.scroll.page_down()
                }
            }
            k if DRIVA_PAGE_UP.matches(k) => {
                if app.details_tab == crate::app::DetailsTab::Details {
                    app.details_scroll.page_up()
                } else {
                    app.launch.scroll.page_up()
                }
            }
            _ => {}
        },
        // Re-reading is on the capitals so `j` and `k` stay navigation, as
        // they are in every other view.
        View::Answer => match key {
            k if ANSWER_AS_TEXT.matches(k) => app.reread_answer(Contract::Text),
            k if ANSWER_AS_LINES.matches(k) => app.reread_answer(Contract::Lines),
            k if ANSWER_AS_FILES.matches(k) => app.reread_answer(Contract::Files),
            k if ANSWER_AS_JSON.matches(k) => app.reread_answer(Contract::Json),
            k if ANSWER_REREAD.matches(k) => app.ask(Request::Answer { contract: None }),
            k if ANSWER_EDIT.matches(k) && app.answer.selected_file().is_some() => {
                app.ask(Request::EditFile)
            }
            k if ANSWER_NEXT.matches(k) => app.answer.select_next(),
            k if ANSWER_PREV.matches(k) => app.answer.select_prev(),
            k if ANSWER_FIRST.matches(k) => app.answer.select_first(),
            k if ANSWER_LAST.matches(k) => app.answer.select_last(),
            k if ANSWER_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        View::Files => match key {
            k if FILES_EDIT.matches(k) && app.selected_file_path().is_some() => {
                app.ask(Request::EditFile)
            }
            k if FILES_NEXT.matches(k) => app.file_select_next(),
            k if FILES_PREV.matches(k) => app.file_select_prev(),
            k if FILES_NEXT_ENTRY.matches(k) => {
                app.select_next_line();
                app.files.select_first();
            }
            k if FILES_PREV_ENTRY.matches(k) => {
                app.select_prev_line();
                app.files.select_first();
            }
            k if FILES_FIRST.matches(k) => app.files.select_first(),
            k if FILES_LAST.matches(k) => {
                let last = app.file_paths().len().saturating_sub(1);
                app.files.select_last(last);
            }
            k if FILES_SCOPE.matches(k) => app.toggle_file_scope(),
            k if FILES_PREVIEW.matches(k) => app.preview.toggle(),
            k if FILES_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        // Full-screen preview is the one view where the text, not the entry
        // list, is what the reader is moving through: `j`/`k` and the arrows
        // scroll it ten lines at a time, `PgUp`/`PgDn` half a screen, and
        // `J`/`K` change entry.
        View::Preview => match key {
            k if PREVIEW_LINKS.matches(k) => app.highlight_first_link(),
            k if PREVIEW_LINK_DESTINATIONS.matches(k) => app.toggle_link_display(),
            k if PREVIEW_TARGET.matches(k) => app.preview.toggle_target(),
            k if PREVIEW_PAGE_DOWN.matches(k) => app.preview.scroll.half_page_down(),
            k if PREVIEW_PAGE_UP.matches(k) => app.preview.scroll.half_page_up(),
            k if PREVIEW_SCROLL_DOWN.matches(k) && app.link_highlight.is_some() => {
                app.highlight_next_link()
            }
            k if PREVIEW_SCROLL_UP.matches(k) && app.link_highlight.is_some() => {
                app.highlight_prev_link()
            }
            k if PREVIEW_SCROLL_DOWN.matches(k) => app.preview.scroll.page_down(),
            k if PREVIEW_SCROLL_UP.matches(k) => app.preview.scroll.page_up(),
            k if PREVIEW_NEXT_ENTRY.matches(k) => app.select_next_line(),
            k if PREVIEW_PREV_ENTRY.matches(k) => app.select_prev_line(),
            k if PREVIEW_FIRST.matches(k) => app.select_first(),
            k if PREVIEW_LAST.matches(k) => app.select_last(),
            k if PREVIEW_COPY.matches(k) => copy_selection(app),
            _ => {}
        },
        // Every key of its own was taken by `handle_overview_key`.
        View::Overview => {}
    }
}

/// Cursor moves that share the live navigator's settle-and-load path.
pub(crate) fn overview_navigation_key(key: KeyEvent) -> bool {
    [
        OVERVIEW_LEFT,
        OVERVIEW_RIGHT,
        OVERVIEW_DOWN,
        OVERVIEW_UP,
        OVERVIEW_NEXT,
        OVERVIEW_PREV,
        OVERVIEW_FIRST,
        OVERVIEW_LAST,
        OVERVIEW_NTH,
    ]
    .iter()
    .any(|binding| binding.matches(key))
}

/// The overview's keys are offered before global keys because the grid uses
/// `l` to move right. Reports whether `key` was one of its own.
fn handle_overview_key(app: &mut App, key: KeyEvent) -> bool {
    let current = app.session_id.clone();
    match key {
        k if OVERVIEW_LEFT.matches(k) => {
            app.overview
                .left(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_RIGHT.matches(k) => {
            app.overview
                .right(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_DOWN.matches(k) => {
            app.overview
                .down(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_UP.matches(k) => {
            app.overview
                .up(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_NEXT.matches(k) => {
            app.overview
                .next(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_PREV.matches(k) => {
            app.overview
                .prev(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_FIRST.matches(k) => {
            app.overview
                .first(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_LAST.matches(k) => {
            app.overview
                .last(&app.interactions, &current, app.workspace.id.as_deref())
        }
        k if OVERVIEW_NTH.matches(k) => {
            if let KeyCode::Char(digit @ '1'..='9') = k.code {
                let position = digit as usize - '1' as usize;
                app.overview.nth(
                    &app.interactions,
                    position,
                    &current,
                    app.workspace.id.as_deref(),
                )
            }
        }
        // Loading another interaction is a server round-trip the event loop
        // makes, as it does for the navigator.
        k if OVERVIEW_OPEN.matches(k) => {
            match app
                .overview
                .selected_id(&app.interactions, &current, app.workspace.id.as_deref())
            {
                Some(id) => {
                    let id = id.to_owned();
                    app.ask(Request::ShowInteraction(id))
                }
                None => app.show_action_message(if app.overview.running_only {
                    "no interaction is currently running"
                } else {
                    "no interaction is running or idle"
                }),
            }
        }
        // The message box sends to the interaction on screen, so the tile's
        // has to be made current first — unless it already is.
        k if OVERVIEW_MESSAGE.matches(k) => {
            match app
                .overview
                .selected_id(&app.interactions, &current, app.workspace.id.as_deref())
            {
                Some(id) if id == current => app.enter_input(),
                Some(id) => {
                    let id = id.to_owned();
                    app.ask(Request::MessageInteraction(id))
                }
                None => app.show_action_message(if app.overview.running_only {
                    "no interaction is currently running"
                } else {
                    "no interaction is running or idle"
                }),
            }
        }
        k if OVERVIEW_SCOPE.matches(k) => {
            app.interactions.toggle_workspace_scope();
            app.overview.open();
        }
        k if OVERVIEW_RUNNING_ONLY.matches(k) => app.overview.toggle_running_only(),
        // The window is the navigator's, so cycling it here narrows both.
        k if OVERVIEW_TIME_FILTER.matches(k) => {
            app.interactions
                .cycle_time_filter(&current, app.workspace.id.as_deref());
            app.overview.open();
        }
        k if OVERVIEW_LAYOUT.matches(k) => app.overview.toggle_layout(),
        k if OVERVIEW_SEARCH.matches(k) => app.search.open(),
        // As in the event list, Esc clears a standing search before it
        // leaves the grid.
        k if k.code == KeyCode::Esc && app.search.query().is_some() => app.search.cancel(),
        k if OVERVIEW_CLOSE.matches(k) => app.set_view(View::Events),
        _ => return false,
    }
    if overview_navigation_key(key) {
        follow_overview_cursor(app);
    }
    true
}

/// Schedule the load of the tile under the grid's cursor, on the live list's
/// settle-and-load path.
pub(crate) fn follow_overview_cursor(app: &mut App) {
    let current = app.session_id.clone();
    if let Some(id) =
        app.overview
            .selected_id(&app.interactions, &current, app.workspace.id.as_deref())
    {
        let id = id.to_owned();
        app.interactions.move_cursor_to(id, &current);
    }
}

/// Open the tag editor for the Interaction currently on screen, with the tags
/// the loop's interactions snapshot holds for it. That snapshot is kept fresh
/// whether or not the live-interactions navigator has ever been opened.
fn edit_current_interaction_tags(app: &mut App, client: &Client) {
    if !app.interactions.listed() {
        match client.list_interactions(&app.interactions_query()) {
            Ok(interactions) => {
                app.interactions.refresh(interactions);
            }
            Err(error) => {
                app.show_error(format!("could not list interactions: {error:#}"));
                return;
            }
        }
    }
    let Some(selected_tags) = app
        .interactions
        .current(&app.session_id)
        .map(|item| item.tags.clone())
    else {
        app.show_warning("current interaction is no longer available");
        return;
    };
    let tags = match client.list_tags() {
        Ok(tags) => tags,
        Err(error) => {
            app.show_error(format!("could not list tags: {error:#}"));
            return;
        }
    };
    app.tag_picker = Some(crate::tag_picker::TagPicker::new(tags, selected_tags));
}

/// Switch the raw panel between Styra's wire capture and the provider's
/// native persisted JSONL. Read freshly when opening it: a live Codex thread
/// can append records after the previous visit.
fn toggle_provider_raw(app: &mut App, client: &Client) {
    if app.provider_raw_open {
        app.provider_raw_open = false;
        return;
    }
    match client.provider_raw(&app.session_id) {
        Ok(raw) => {
            app.provider_raw = Some(crate::raw::ProviderRawView::new(raw.text));
            app.provider_raw_open = true;
        }
        Err(error) => app.show_error(format!("could not read provider raw: {error:#}")),
    }
}

/// Copy whatever the current view treats as the selected entry to the
/// clipboard (see `App::copy_text`).
fn copy_selection(app: &mut App) {
    let Some(text) = app.copy_text() else {
        return app.show_warning("nothing selected to copy");
    };
    copy(app, text, "copied to clipboard");
}

/// Copy the session's whole conversation — messages, errors, and model
/// changes, without the tool calls between them.
fn copy_conversation(app: &mut App) {
    let Some(text) = app.conversation_text() else {
        return app.show_warning("no conversation to copy yet");
    };
    copy(app, text, "copied the conversation to clipboard");
}

/// Send text to the clipboard, reporting the outcome the same way
/// [`terminal::open_shell`](crate::terminal::open_shell) does.
fn copy(app: &mut App, text: String, done: &str) {
    match crate::clipboard::copy(&text) {
        Ok(()) => app.show_action_message(done),
        Err(error) => app.report_error(format!("could not copy to clipboard: {error:#}")),
    }
}

/// Open the path prompt over the message box, against what this session's
/// sandbox carries and whether that sandbox can still be changed.
fn open_insert(app: &mut App) {
    app.insert = Some(insert::Prompt::new(
        app.workspace.root().map(std::path::Path::to_path_buf),
        app.launch.driva.as_ref(),
        app.can_change_mounts(),
    ));
}

/// Route a key to the open path prompt and apply what it decided to the
/// message being composed. The prompt itself knows nothing about [`App`]; this
/// is where its outcome becomes message text, a mount request, and a notice.
pub fn handle_insert_key(app: &mut App, key: KeyEvent) {
    let Some(prompt) = app.insert.as_mut() else {
        return;
    };
    match prompt.key(key) {
        insert::Outcome::Open => {}
        insert::Outcome::Closed => app.insert = None,
        insert::Outcome::Notice(notice) => app.show_warning(notice),
        insert::Outcome::Insert { path, notice } => {
            app.insert = None;
            app.editor_composer_mut()
                .insert(&path.display().to_string());
            if let Some(notice) = notice {
                app.show_warning(notice);
            }
        }
        insert::Outcome::Grant { mount, path } => {
            app.insert = None;
            // The mount is a request, not a live change: nothing rebinds a
            // running sandbox. An idle one is restarted under it before this
            // message goes out; otherwise it waits for the launch. The path
            // goes in either way — the operator asked for it.
            crate::launch::grant_interaction_mount(app, mount);
            app.editor_composer_mut()
                .insert(&path.display().to_string());
        }
    }
}

/// Whether this keypress sends the message into a new Git workspace.
///
/// Ctrl-Enter is a distinct submission: it creates the Session's branch and
/// linked workspace as it sends the message, with no standing option to leak
/// into a later Session. On a first prompt the Session launches there; on a
/// later message the running interaction is moved there first, as `Ctrl+W` does.
///
/// Such a send is handed to the event loop rather than made here, the same
/// request `Ctrl+W` makes for a Session that already exists: branching blocks long
/// enough to need the notice only the loop can paint.
fn creates_worktree(app: &App, key: KeyEvent) -> bool {
    // A base chosen with alt-b is a fresh checkout of that branch, so the
    // first message launches into a worktree from plain Enter too.
    let chosen_base = app.base_branch.is_some() && app.session_id.is_empty();
    (EDITOR_SEND_IN_BRANCH.matches(key) || chosen_base)
        // Nothing is sent, and so nothing is branched, for a blank box.
        && !app.composer.message().is_empty()
}

/// Ask the server to commit this interaction's turns as they end, or to stop.
/// A screen with no interaction yet has nothing to answer for: the one it
/// starts commits its turns if it is given a linked checkout of its own.
fn toggle_auto_commit(app: &mut App) {
    if app.session_id.is_empty() {
        return app.show_warning(
            "no interaction yet — one started in its own worktree commits each turn",
        );
    }
    app.ask(Request::SetAutoCommit(!app.auto_commit));
}

/// Ask for the file at the top of the diff to be opened, at the line there,
/// or say why there is none to open.
fn open_diff_file(app: &mut App) {
    let Some(diff) = app.shown_diff() else {
        return;
    };
    match diff.location() {
        Ok((path, line)) => app.ask(Request::OpenPathAt(path, line)),
        Err(reason) => app.show_error(reason),
    }
}

/// Ask the event loop to open the checkout against where it will be integrated.
/// An interaction not yet started is diffed against a branch the operator
/// picks, from the checkout it will start in.
pub(crate) fn ask_for_diff(app: &mut App) {
    if app.session_id.is_empty() {
        return app.ask(Request::ShowDiff(String::new()));
    }
    let Some(interaction) = app.interactions.current(&app.session_id) else {
        return app.show_warning("no interaction to diff");
    };
    if interaction.checkout.is_none() {
        return app.show_error("this interaction has no Git checkout to diff");
    }
    let id = interaction.id.clone();
    let location = if app.link_highlight.is_some() {
        match app.highlighted_link_target() {
            Some((_, Ok(path), line)) => Some((path, line)),
            Some((_, Err(reason), _)) => return app.show_warning(reason),
            None => return app.show_warning("selected link names no file to diff"),
        }
    } else if app.view == View::Files {
        match app.selected_file_path() {
            Some(path) => Some((path, None)),
            None => return app.show_warning("no file selected to diff"),
        }
    } else {
        None
    };
    if let Some((path, line)) = location {
        app.ask(Request::ShowDiffAt { id, path, line });
    } else {
        app.ask(Request::ShowDiff(id));
    }
}

pub fn handle_input_key(
    app: &mut App,
    client: &Client,
    workspace_id: &str,
    live: &mut Attachment,
    key: KeyEvent,
) {
    if handle_composer_key(&mut app.composer, key) {
        return;
    }
    match key {
        // With several boxes, Esc steps back to choosing between them; a
        // second Esc, from there, leaves as it does with one.
        k if GLOBAL_LEAVE_MESSAGE.matches(k) => app.enter_list(),
        k if EDITOR_BRANCH.matches(k) => switch_to_branch_prompt(app),
        // Choosing a shape is part of writing the message, so it lives in the
        // box rather than being a mode entered from outside it.
        k if EDITOR_SEND.matches(k) || EDITOR_SEND_IN_BRANCH.matches(k) => {
            // A mid-turn move is refused by the server, which puts the
            // message back in the box with its reason.
            let create_worktree = creates_worktree(app, k);
            if let Some(message) = app.take_message() {
                app.enter_list();
                if create_worktree {
                    app.ask(Request::CreateWorktree {
                        message: Some(message),
                    });
                } else {
                    session::submit_message(app, client, workspace_id, live, message, false);
                }
            }
        }
        _ => handle_editor_controls(app, key),
    }
}

/// Editing is identical in the normal and branch message boxes.
fn handle_composer_key(composer: &mut crate::composer::Composer, key: KeyEvent) -> bool {
    if composer.choosing() {
        match key {
            k if EDITOR_BOX_LEAVE.matches(k) => return false,
            k if EDITOR_BOX_NEXT.matches(k) => composer.choose_next(),
            k if EDITOR_BOX_PREV.matches(k) => composer.choose_previous(),
            k if EDITOR_BOX_EDIT.matches(k) => composer.stop_choosing(),
            k if EDITOR_BOX_DELETE.matches(k) => composer.remove_chosen(),
            k if EDITOR_ADD_BOX.matches(k) => composer.add_part(),
            _ => {}
        }
        return true;
    }
    match key {
        k if EDITOR_CHOOSE_BOX.matches(k) && composer.start_choosing() => {}
        k if EDITOR_ADD_BOX.matches(k) => composer.add_part(),
        k if EDITOR_NEWLINE.matches(k) => composer.newline(),
        k if EDITOR_DELETE_WORD.matches(k) => composer.delete_word(),
        k if EDITOR_HISTORY_OLDER.matches(k) => composer.history_previous(),
        k if EDITOR_HISTORY_NEWER.matches(k) => composer.history_next(),
        k if k.code == KeyCode::Backspace => composer.backspace(),
        k if !k
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if let KeyCode::Char(ch) = k.code {
                composer.char(ch);
            } else {
                return false;
            }
        }
        _ => return false,
    }
    true
}

fn handle_editor_controls(app: &mut App, key: KeyEvent) {
    match key {
        k if EDITOR_CONTRACT.matches(k) => {
            if let Some(prompt) = app.branch_prompt.as_mut() {
                prompt.contract = session::next_contract(prompt.contract);
            } else {
                app.outbox.cycle_contract();
            }
        }
        k if EDITOR_LAUNCHER.matches(k) => app.open_launcher(),
        k if EDITOR_CYCLE_PROVIDER.matches(k) => app.open_provider_picker(),
        k if EDITOR_CYCLE_MODEL.matches(k) => open_cycling(app, crate::launcher::Field::Model),
        k if EDITOR_CYCLE_EFFORT.matches(k) => open_cycling(app, crate::launcher::Field::Effort),
        k if EDITOR_INSERT_PATH.matches(k) => open_insert(app),
        k if EDITOR_AUTO_COMMIT.matches(k) => toggle_auto_commit(app),
        _ => {}
    }
}

/// Open the model picker already one step along `field`, so a cycling
/// shortcut changes the choice on its first press like on every later one.
fn open_cycling(app: &mut App, field: crate::launcher::Field) {
    app.open_model_picker();
    if let Some(picker) = app.launcher.as_mut() {
        picker.cycle(field);
    }
}

#[cfg(test)]
mod tests;
