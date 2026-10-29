//! Input handling and action dispatch for the terminal client.
//!
//! Key assignments live in [`crate::keybindings`]. This module owns the behavior
//! performed after a key has been recognized: prompts, view dispatch, and
//! actions that touch application state or the server.

use crate::keybindings::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::path::Path;

use crate::activity::Status;
use crate::app::{App, Request, View};
use crate::insert;
use crate::launch;
use crate::link_menu::LinkAction;
use crate::preferences;
use crate::session::{self, Attachment};
use styra_diff::Outcome;
use styra_protocol::{CompletionState, Contract, LogEntry, ProjectAction};
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

pub fn handle_retarget_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some((text, history)) = app.retarget_prompt.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Esc => app.retarget_prompt = None,
        KeyCode::Tab if !history.is_empty() => {
            let index = history
                .iter()
                .position(|saved| saved.branch == *text)
                .map(|index| (index + history.len() - 1) % history.len())
                .unwrap_or(history.len() - 1);
            *text = history[index].branch.clone();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        KeyCode::Enter if !text.trim().is_empty() => {
            let branch = text.trim().to_owned();
            let restore = history.iter().any(|saved| saved.branch == branch);
            app.retarget_prompt = None;
            if restore {
                match client.restore_session_branch(&app.session_id, &branch) {
                    Ok(()) => app.show_action_message(format!("Returned to {branch}")),
                    Err(error) => app.show_warning(format!("Could not restore branch: {error:#}")),
                }
            } else {
                app.ask(Request::Retarget(branch));
            }
        }
        _ => {
            if let Some(edit) = Edit::from_key(key) {
                edit.apply(text);
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
/// `u` switches between the checkout's diff and the selected turn's. Tab
/// hands navigation to the interaction when the diff is in a side pane, and
/// fullscreen swaps the diff's header for the conversation's latest
/// messages; `D` takes a side diff fullscreen as it is, turn diff and all. Other global bindings and the other views' keys do not act from
/// it.
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
    if DIFF_CONVERSATION.matches(key) && app.view == View::CheckoutDiff && !typing_filter {
        app.diff_conversation = !app.diff_conversation;
        return;
    }
    if DIFF_TURN_HIGHLIGHT.matches(key) && !typing_filter {
        app.diff_turn_highlight = !app.diff_turn_highlight;
        app.diff_turn = false;
        app.sync_turn_diff();
        return;
    }
    if DIFF_TURN.matches(key) && !typing_filter {
        app.diff_turn = !app.diff_turn;
        app.sync_turn_diff();
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
    if key.code == KeyCode::Char('D') && !typing_filter {
        app.diff_preview = false;
        app.diff_interaction_focus = false;
        app.set_view(View::CheckoutDiff);
        app.sync_turn_diff();
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
            if app.view == View::CheckoutDiff {
                app.set_view(View::Events);
            } else {
                app.diff_preview = false;
                app.diff_interaction_focus = false;
            }
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
/// Prompts retain their keys, and fullscreen diffs keep their own Tab action.
pub fn handle_diff_focus_key(app: &mut App, key: KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        || app.focus != crate::app::Focus::List
        || !app.diff_preview
        || !matches!(app.view, View::Events | View::Files)
        || app.search.typing()
        || app.interactions.typing_filter()
        || app.shown_diff().is_some_and(|diff| {
            diff.typing() || (diff.commits.open && !app.diff_interaction_focus)
        })
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

/// Keys for the history and checkout choices. Confirming closes the modal before the
/// server call, so success can replace the screen and failure returns to it.
pub fn handle_branch_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some(prompt) = app.branch_prompt.as_mut() else {
        return;
    };
    match key {
        k if BRANCH_NEXT.matches(k) => prompt.select_next(),
        k if BRANCH_PREV.matches(k) => prompt.select_previous(),
        k if BRANCH_CHANGE.matches(k) => prompt.change(),
        k if BRANCH_CONFIRM.matches(k) => {
            let at_ms = prompt.at_ms();
            let history = prompt.history();
            let checkout = prompt.checkout();
            app.branch_prompt = None;
            session::branch_session(app, client, at_ms, history, checkout);
        }
        k if BRANCH_CANCEL.matches(k) => app.branch_prompt = None,
        _ => {}
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
        k if GLOBAL_RETARGET.matches(k) && !app.session_id.is_empty() => {
            match client.session_branch_history(&app.session_id) {
                Ok(history) => app.retarget_prompt = Some((String::new(), history)),
                Err(error) => app.show_warning(format!("Could not read branch history: {error:#}")),
            }
            return;
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
        // `d` opens the diff in the preview; `D` opens it fullscreen.
        k if GLOBAL_DIFF.matches(k) => return ask_for_diff(app, false),
        k if GLOBAL_DIFF_FULLSCREEN.matches(k) => return ask_for_diff(app, true),
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
            k if EVENTS_RUN.matches(k) => app.ask(Request::ProjectAction(ProjectAction::Run)),
            k if EVENTS_TEST.matches(k) => app.ask(Request::ProjectAction(ProjectAction::Test)),
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
            k if (EVENTS_NEXT_ENTRY.matches(k) || EVENTS_NEXT_LINE.matches(k))
                && app.entry_log.focused() =>
            {
                app.entry_log_select_next()
            }
            k if (EVENTS_PREV_ENTRY.matches(k) || EVENTS_PREV_LINE.matches(k))
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
            // full-screen one; `J`/`K` still step between entries.
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
            k if EVENTS_NEXT_ENTRY.matches(k) => app.select_next(),
            k if EVENTS_PREV_ENTRY.matches(k) => app.select_prev(),
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
        // Every key was taken by `handle_diff_key`.
        View::CheckoutDiff => {}
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
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not save the default launch policy: {error:#}"
                        ))),
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
        k if OVERVIEW_FIRST.matches(k) => app
            .overview
            .first(&app.interactions, app.workspace.id.as_deref()),
        k if OVERVIEW_LAST.matches(k) => app
            .overview
            .last(&app.interactions, app.workspace.id.as_deref()),
        k if OVERVIEW_NTH.matches(k) => {
            if let KeyCode::Char(digit @ '1'..='9') = k.code {
                let position = digit as usize - '1' as usize;
                app.overview
                    .nth(&app.interactions, position, app.workspace.id.as_deref())
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
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not copy to clipboard: {error:#}"
        ))),
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
            app.composer.insert(&path.display().to_string());
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
            app.composer.insert(&path.display().to_string());
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
    EDITOR_SEND_IN_BRANCH.matches(key)
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
fn ask_for_diff(app: &mut App, fullscreen: bool) {
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
        app.ask(Request::ShowDiffAt {
            id,
            path,
            line,
            fullscreen,
        });
    } else if fullscreen {
        app.ask(Request::ShowDiffFullscreen(id));
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
    if app.composer.choosing() {
        return handle_choosing_key(app, key);
    }
    match key {
        // With several boxes, Esc steps back to choosing between them; a
        // second Esc, from there, leaves as it does with one.
        k if EDITOR_CHOOSE_BOX.matches(k) && app.composer.start_choosing() => {}
        k if GLOBAL_LEAVE_MESSAGE.matches(k) => leave_message(app),
        k if EDITOR_ADD_BOX.matches(k) => app.composer.add_part(),
        // Choosing a shape is part of writing the message, so it lives in the
        // box rather than being a mode entered from outside it.
        k if EDITOR_CONTRACT.matches(k) => app.outbox.cycle_contract(),
        k if EDITOR_NEWLINE.matches(k) => app.composer.newline(),
        k if EDITOR_SEND.matches(k) || EDITOR_SEND_IN_BRANCH.matches(k) => {
            let create_worktree = creates_worktree(app, k);
            // Moving the interaction means stopping it, which would throw
            // away the turn under way; the message stays in the box instead.
            if create_worktree
                && matches!(live, Attachment::Attached { .. })
                && app.activity.status == Status::Running
            {
                return app.show_warning(
                    "the agent is mid-turn — wait for it to finish before moving it to a new Git workspace",
                );
            }
            if app.branch_needs_prompt_name {
                if let Some(name) =
                    styra_server::journal::name_from_message(Some(&app.composer.message()))
                {
                    if let Err(error) = client.rename_session(&app.session_id, Some(&name)) {
                        return app.show_error(format!("could not name the branch: {error:#}"));
                    }
                    app.branch_needs_prompt_name = false;
                }
            }
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
        k if EDITOR_DELETE_WORD.matches(k) => app.composer.delete_word(),
        k if EDITOR_LAUNCHER.matches(k) => app.open_launcher(),
        k if EDITOR_CYCLE_PROVIDER.matches(k) => app.open_provider_picker(),
        k if EDITOR_CYCLE_MODEL.matches(k) => app.open_model_picker(),
        k if EDITOR_CYCLE_EFFORT.matches(k) => {
            app.open_model_picker();
            if let Some(picker) = app.launcher.as_mut() {
                picker.active = crate::launcher::Field::Effort;
            }
        }
        // Naming a file is part of writing the message, so it opens from the
        // box rather than from the driva view that the grant it may ask for
        // would otherwise have to be made in.
        k if EDITOR_INSERT_PATH.matches(k) => open_insert(app),
        // Also from the box, so the turn about to be sent can be kept out of
        // the history without leaving the message half-written.
        k if EDITOR_AUTO_COMMIT.matches(k) => toggle_auto_commit(app),
        k if EDITOR_HISTORY_OLDER.matches(k) => app.composer.history_previous(),
        k if EDITOR_HISTORY_NEWER.matches(k) => app.composer.history_next(),
        k if k.code == KeyCode::Backspace => app.composer.backspace(),
        // Everything else printable is the message itself, once the modified
        // keys above have had their turn.
        k if !k.modifiers.contains(KeyModifiers::CONTROL)
            && !k.modifiers.contains(KeyModifiers::ALT) =>
        {
            if let KeyCode::Char(ch) = k.code {
                app.composer.char(ch)
            }
        }
        _ => {}
    }
}

fn leave_message(app: &mut App) {
    app.branch_needs_prompt_name = false;
    app.enter_list();
}

/// Keys while choosing between the message's boxes: nothing is typed, so
/// plain letters move, edit, and delete instead.
fn handle_choosing_key(app: &mut App, key: KeyEvent) {
    match key {
        k if EDITOR_BOX_LEAVE.matches(k) => leave_message(app),
        k if EDITOR_BOX_NEXT.matches(k) => app.composer.choose_next(),
        k if EDITOR_BOX_PREV.matches(k) => app.composer.choose_previous(),
        k if EDITOR_BOX_EDIT.matches(k) => app.composer.stop_choosing(),
        k if EDITOR_BOX_DELETE.matches(k) => app.composer.remove_chosen(),
        k if EDITOR_ADD_BOX.matches(k) => app.composer.add_part(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_diff::DiffTarget;
    use styra_protocol::{
        AttributedMount, DrivaOptions, LaunchMount, Mount, MountAccess, MountOrigin,
    };

    #[test]
    fn retarget_prompt_cycles_saved_branches_and_submits_a_new_target() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        let client = Client::new(PathBuf::from("/missing.sock"));
        app.retarget_prompt = Some((
            String::new(),
            vec![
                styra_protocol::SavedBranch {
                    branch: "styra/first".into(),
                    branched_from: None,
                },
                styra_protocol::SavedBranch {
                    branch: "styra/second".into(),
                    branched_from: None,
                },
            ],
        ));
        for expected in ["styra/second", "styra/first", "styra/second"] {
            handle_retarget_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Tab));
            assert_eq!(app.retarget_prompt.as_ref().unwrap().0, expected);
        }
        handle_retarget_prompt_key(
            &mut app,
            &client,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        for c in "release".chars() {
            handle_retarget_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Char(c)));
        }
        handle_retarget_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Enter));
        assert!(app.retarget_prompt.is_none());
        assert!(
            matches!(app.take_request(), Some(Request::Retarget(target)) if target == "release")
        );
        app.retarget_prompt = Some((String::new(), Vec::new()));
        handle_retarget_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Enter));
        assert!(app.retarget_prompt.is_some());
        handle_retarget_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Esc));
        assert!(app.retarget_prompt.is_none());
        assert!(app.take_request().is_none());
    }

    #[test]
    fn integration_message_defaults_to_the_name_and_keeps_edits_on_escape() {
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex").unwrap(),
            "session-1",
        );
        app.interactions
            .note_name("session-1", Some("Fix integration".into()));
        app.open_integration_prompt();
        assert_eq!(app.integration_prompt.as_deref(), Some("Fix integration"));
        handle_integration_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
        );
        handle_integration_prompt_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.integration_prompt.is_none());
        app.open_integration_prompt();
        assert_eq!(app.integration_prompt.as_deref(), Some("Fix integration!"));

        // Clearing the default is also an intentional draft.
        app.integration_prompt = Some(String::new());
        handle_integration_prompt_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.open_integration_prompt();
        assert_eq!(app.integration_prompt.as_deref(), Some(""));
    }

    #[test]
    fn ctrl_w_deletes_the_previous_integration_message_word() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.integration_prompt = Some("fix\n café\u{2003}世界 \t".into());
        for expected in ["fix\n café\u{2003}", "fix\n ", "", ""] {
            handle_integration_prompt_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            );
            assert_eq!(app.integration_prompt.as_deref(), Some(expected));
            assert!(app.take_request().is_none());
        }
    }

    #[test]
    fn integration_message_is_explicitly_submitted_and_generation_stays_editable() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.integration_prompt = Some(String::new());
        handle_integration_prompt_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.take_request().is_none());
        handle_integration_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
        );
        assert!(matches!(
            app.take_request(),
            Some(Request::GenerateIntegrationMessage)
        ));
        app.integration_prompt = Some("Generated subject".into());
        handle_integration_prompt_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        handle_integration_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
        );
        assert!(app.take_request().is_none());
        handle_integration_prompt_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(app.take_request(), Some(Request::Integrate(message)) if message == "Generated subject\n!")
        );
        handle_integration_prompt_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.integration_prompt.is_none());
        assert!(app.take_request().is_none());
    }

    /// A session whose sandbox binds `root` at `/workspace` and nothing else,
    /// with nothing launched — so the launch policy is still open to editing.
    fn app(root: &Path) -> App {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.workspace.enter(root.to_path_buf());
        app.launch.record(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/workspace"),
            network: false,
            base: Vec::new(),
            mounts: vec![AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source: root.to_path_buf(),
                    destination: PathBuf::from("/workspace"),
                    access: MountAccess::ReadWrite,
                },
            }],
            ..Default::default()
        });
        app.enter_input();
        app
    }

    fn tree(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("styra-keys-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("reports")).unwrap();
        std::fs::write(base.join("reports/summary.md"), "x").unwrap();
        std::fs::write(base.join("notes.txt"), "x").unwrap();
        std::fs::canonicalize(base).unwrap()
    }

    fn typed(app: &mut App, text: &str) {
        open_insert(app);
        for ch in text.chars() {
            handle_insert_key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        handle_insert_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    #[test]
    fn selection_shortcuts_open_fuzzy_pickers_and_apply_only_on_enter() {
        use crate::launcher::{self, Choice};
        use styra_protocol::agent::{Provider, Selection};

        let root = tempfile::tempdir().unwrap();
        let preferences = root.path().join("defaults.json");
        let client = Client::new(PathBuf::from("/missing.sock"));
        let mut live = Attachment::Detached;
        let active_list = |picker: &launcher::Launcher| {
            if picker.choice == Choice::ModelEffort && picker.active == launcher::Field::Effort {
                (picker.effort_list.clone(), picker.effort_labels())
            } else {
                (picker.list.clone(), picker.labels())
            }
        };
        for (shortcut, choice) in [
            ('p', Choice::Provider),
            ('o', Choice::ModelEffort),
            ('e', Choice::ModelEffort),
        ] {
            let provider = Provider::Codex;
            let model = &provider.models()[0];
            let original = Selection {
                provider,
                model: model.id.to_owned(),
                effort: model.efforts[0],
            };
            let mut app = App::pending(original.clone());
            app.enter_input();
            app.composer.insert("draft message");
            let key = KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::CONTROL);
            handle_input_key(&mut app, &client, "workspace", &mut live, key);
            assert_eq!(app.launcher.as_ref().unwrap().choice, choice);
            assert_eq!(app.selection, original);
            assert!(app.take_request().is_none());

            // The same chord advances, including ctrl-p which normally moves up.
            launcher::handle_key(&mut app, key, &preferences);
            let picker = app.launcher.as_ref().unwrap();
            let chosen = picker.selection();
            assert_ne!(chosen, original);
            assert_eq!(app.selection, original);
            assert!(app.take_request().is_none());

            let count = active_list(app.launcher.as_ref().unwrap()).1.len();
            for _ in 0..count {
                launcher::handle_key(&mut app, key, &preferences);
            }
            let picker = app.launcher.as_ref().unwrap();
            let (_, labels) = active_list(picker);
            assert_eq!(labels[active_list(picker).0.selected], labels[1]);
            assert_eq!(app.selection, original);

            // Narrow to the highlighted row; typing and navigation are previews.
            let query = active_list(app.launcher.as_ref().unwrap()).1[1].clone();
            for ch in query.chars() {
                launcher::handle_key(&mut app, KeyCode::Char(ch).into(), &preferences);
            }
            let filtered = app.launcher.as_ref().unwrap().selection();
            if shortcut == 'o' {
                assert_eq!(filtered.model, chosen.model);
            } else {
                assert_eq!(filtered, chosen);
            }
            assert_eq!(app.selection, original);
            let picker = app.launcher.as_ref().unwrap();
            let (list, labels) = active_list(picker);
            let matches = list.matches(&labels);
            let next = (list.selected + 1) % matches.len();
            launcher::handle_key(&mut app, key, &preferences);
            let picker = app.launcher.as_ref().unwrap();
            let (list, _) = active_list(picker);
            assert_eq!(list.selected, next);
            assert_eq!(list.query, query);
            let chosen = picker.selection();
            assert_eq!(app.selection, original);
            launcher::handle_key(&mut app, KeyCode::Enter.into(), &preferences);
            assert_eq!(app.selection, chosen);
            assert_eq!(app.composer.message(), "draft message");
            assert_eq!(app.focus, crate::app::Focus::Input);
            assert!(app.launcher.is_none());
            assert!(app.take_request().is_none());
        }
    }

    #[test]
    fn escape_cancels_filtered_selection_pickers_and_empty_results_do_not_apply() {
        use crate::launcher;
        use styra_protocol::agent::{Provider, Selection};

        let root = tempfile::tempdir().unwrap();
        let preferences = root.path().join("defaults.json");
        let client = Client::new(PathBuf::from("/missing.sock"));
        let mut live = Attachment::Detached;
        for shortcut in ['p', 'o', 'e'] {
            let original = Selection::new(Provider::Codex);
            let mut app = App::pending(original.clone());
            app.enter_input();
            handle_input_key(
                &mut app,
                &client,
                "workspace",
                &mut live,
                KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::CONTROL),
            );
            for ch in "no-such-choice".chars() {
                launcher::handle_key(&mut app, KeyCode::Char(ch).into(), &preferences);
            }
            launcher::handle_key(&mut app, KeyCode::Enter.into(), &preferences);
            assert!(app.launcher.is_some(), "empty results cannot be selected");
            launcher::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                &preferences,
            );
            assert!(app.launcher.is_some(), "only Enter selects a choice");
            launcher::handle_key(&mut app, KeyCode::Esc.into(), &preferences);
            assert!(app.launcher.is_none(), "one Esc cancels a filtered picker");
            assert_eq!(app.selection, original);
            assert!(app.take_request().is_none());
            assert!(app.recent_models.is_empty());
            assert!(!preferences.exists());
        }
    }

    #[test]
    fn escape_closes_the_branch_prompt_without_naming_it_from_a_later_message() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.session_id = "branch-1".into();
        app.branch_needs_prompt_name = true;
        app.enter_input();
        app.composer.insert("discarded prompt");
        let client = Client::new(PathBuf::from("/missing.sock"));
        let mut live = Attachment::Detached;
        handle_input_key(
            &mut app,
            &client,
            "workspace",
            &mut live,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(app.focus, crate::app::Focus::List);
        assert!(!app.branch_needs_prompt_name);
        assert!(app.take_request().is_none());
    }

    /// Ctrl-N opens another box; with two, Esc chooses between them rather
    /// than leaving, and only a second Esc leaves — with both boxes kept.
    #[test]
    fn escape_chooses_between_boxes_before_leaving() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.enter_input();
        let client = Client::new(PathBuf::from("/missing.sock"));
        let mut live = Attachment::Detached;
        let mut press = |app: &mut App, code: KeyCode, modifiers: KeyModifiers| {
            handle_input_key(
                app,
                &client,
                "workspace",
                &mut live,
                KeyEvent::new(code, modifiers),
            )
        };
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE);
        assert_eq!(app.composer.parts(), ["a", "b"]);

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.focus, crate::app::Focus::Input);
        assert!(app.composer.choosing());
        // While choosing, letters move between boxes instead of typing.
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.composer.focused(), 0);
        assert_eq!(app.composer.parts(), ["a", "b"]);

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.focus, crate::app::Focus::List);
        assert!(!app.composer.choosing());
        assert_eq!(app.composer.parts(), ["a", "b"]);
    }

    #[test]
    fn a_blank_branch_prompt_keeps_the_fallback_and_waits_for_input() {
        let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
        app.session_id = "branch-1".into();
        app.branch_needs_prompt_name = true;
        app.enter_input();
        let client = Client::new(PathBuf::from("/missing.sock"));
        let mut live = Attachment::Detached;
        handle_input_key(
            &mut app,
            &client,
            "workspace",
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(app.focus, crate::app::Focus::Input);
        assert!(app.branch_needs_prompt_name);
        assert!(app.take_request().is_none());
    }

    #[test]
    fn uppercase_g_in_details_opens_the_git_checkout_prompt_prefilled() {
        let root = tree("git-checkout-prompt");
        let mut app = app(&root);
        app.enter_list();
        app.workspace.git_repository = Some(root.join("repository"));
        app.toggle_view(View::Driva);
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(
            app.git_repository_prompt.as_deref(),
            Some(root.join("repository").to_string_lossy().as_ref())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// A view's own key wins over the global command on the same key, as the
    /// reference and the action menu say: `I` in details is the inherit
    /// toggle, not integration.
    #[test]
    fn a_views_own_key_wins_over_the_global_one() {
        let root = tree("own-key-wins");
        let mut app = app(&root);
        app.enter_list();
        app.session_id = "session-1".into();
        app.toggle_view(View::Driva);
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert!(app.integration_prompt.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// `T` is global, so it edits the interaction's tags from the reading
    /// views too, not only from the event list.
    #[test]
    fn tags_are_edited_from_a_reading_view() {
        let root = tree("tags-from-raw");
        let mut app = app(&root);
        app.enter_list();
        app.session_id = "session-1".into();
        app.toggle_view(View::Raw);
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        // With no server to answer, reaching the tag editor shows as its
        // failure to list the interactions.
        assert!(app
            .notices
            .iter()
            .any(|notice| notice.text.contains("could not list interactions")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn control_w_removes_mount_path_parts() {
        let root = tree("mount-path-prompt");
        let mut app = app(&root);
        app.launch.prompt = Some("/home/op/project/crates/inner".into());

        handle_mount_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            app.launch.prompt.as_deref(),
            Some("/home/op/project/crates/")
        );

        handle_mount_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.launch.prompt.as_deref(), Some("/home/op/project/"));

        app.launch.prompt = Some("/".into());
        handle_mount_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.launch.prompt.as_deref(), Some("/"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The list under the mount prompt narrows with every letter typed; the
    /// arrows move through it, Tab takes the highlighted entry and goes into
    /// it when it is a directory, Shift-Tab climbs back out, and Esc closes
    /// the prompt with its list.
    #[test]
    fn the_mount_prompt_narrows_a_live_list_of_paths() {
        let root = tree("mount-path-list");
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        let base = root.display().to_string();
        let mut app = app(&root);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let prompt = |app: &App| app.launch.prompt.clone().unwrap();
        let shown = |app: &App| {
            let paths = app.launch.paths.as_ref().unwrap();
            paths
                .list
                .matches(&paths.rows)
                .into_iter()
                .map(|found| paths.rows[found.index].clone())
                .collect::<Vec<_>>()
        };

        app.launch.prompt = Some(format!("{base}/"));
        handle_mount_prompt_key(&mut app, key(KeyCode::Down));
        assert_eq!(shown(&app), ["alpha/", "beta/", "notes.txt", "reports/"]);
        handle_mount_prompt_key(&mut app, key(KeyCode::Up));

        for letter in "rp".chars() {
            handle_mount_prompt_key(&mut app, key(KeyCode::Char(letter)));
        }
        assert_eq!(shown(&app), ["reports/"]);
        handle_mount_prompt_key(&mut app, key(KeyCode::Tab));
        assert_eq!(prompt(&app), format!("{base}/reports/"));
        assert_eq!(shown(&app), ["summary.md"]);

        handle_mount_prompt_key(&mut app, key(KeyCode::BackTab));
        assert_eq!(prompt(&app), format!("{base}/"));
        for letter in "bt".chars() {
            handle_mount_prompt_key(&mut app, key(KeyCode::Char(letter)));
        }
        assert_eq!(shown(&app), ["beta/"]);
        handle_mount_prompt_key(&mut app, key(KeyCode::Backspace));
        handle_mount_prompt_key(&mut app, key(KeyCode::Backspace));
        handle_mount_prompt_key(&mut app, key(KeyCode::Down));
        handle_mount_prompt_key(&mut app, key(KeyCode::Tab));
        assert_eq!(prompt(&app), format!("{base}/beta/"));

        handle_mount_prompt_key(&mut app, key(KeyCode::Esc));
        assert!(app.launch.prompt.is_none());
        assert!(app.launch.paths.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Enter on a name that is still being narrowed mounts the row the list
    /// has highlighted for it; a path that exists is mounted as typed, even
    /// while the list shows its siblings.
    #[test]
    fn enter_mounts_the_highlighted_path_for_a_partial_name() {
        let root = tree("mount-path-enter");
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        let base = root.display().to_string();
        let mut app = app(&root);
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let sources = |app: &App| {
            app.launch
                .interaction
                .mounts
                .iter()
                .map(|mount| mount.source.clone())
                .collect::<Vec<_>>()
        };

        app.launch.prompt = Some(format!("{base}/alp"));
        handle_mount_prompt_key(&mut app, key(KeyCode::Enter));
        assert!(app.launch.prompt.is_none());
        assert_eq!(sources(&app), [root.join("alpha")]);

        app.launch.prompt = Some(format!("{base}/reports"));
        handle_mount_prompt_key(&mut app, key(KeyCode::Enter));
        assert_eq!(sources(&app), [root.join("alpha"), root.join("reports")]);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Ctrl-N starts a new session, while `N` moves among actively working
    /// interactions and plain `n` follows newly idle work first.
    #[test]
    fn interaction_shortcuts_distinguish_new_live_and_working() {
        let root = tree("live-step");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        let mut press = |app: &mut App, code, modifiers| {
            handle_list_key(
                app,
                &client,
                &mut live,
                KeyEvent::new(code, modifiers),
                &mut pending_fold,
                &root.join("preferences.toml"),
            );
        };

        press(&mut app, KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(app.take_request(), Some(Request::NewSession));

        press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(app.take_request(), Some(Request::NextLiveInteraction));

        press(&mut app, KeyCode::Char('N'), KeyModifiers::SHIFT);
        assert_eq!(app.take_request(), Some(Request::NextWorkingInteraction));

        press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert_eq!(app.take_request(), Some(Request::JumpBack));

        press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(app.take_request(), Some(Request::JumpForward));

        press(&mut app, KeyCode::Char('o'), KeyModifiers::ALT);
        assert_eq!(app.take_request(), Some(Request::InteractionHistory));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn plain_l_opens_the_launcher_from_the_main_view() {
        let root = tree("launcher-shortcut");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert!(app.launcher.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    /// The grid takes `l` from the launcher to move right, Tab and Shift-Tab
    /// cycle the tiles, `Enter` asks for the tile's interaction, `i` for its
    /// message box, and `o` toggles the overview from both sides.
    #[test]
    fn the_overview_walks_its_grid_and_opens_the_chosen_tile() {
        use styra_protocol::InteractionActivity;
        let root = tree("overview-keys");
        let mut app = app(&root);
        app.enter_list();
        app.interactions.refresh(vec![
            crate::interactions::tests::interaction("2-left", InteractionActivity::Running),
            crate::interactions::tests::interaction("1-right", InteractionActivity::Pending),
        ]);
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        let mut press = |app: &mut App, code| {
            handle_list_key(
                app,
                &client,
                &mut live,
                KeyEvent::new(code, KeyModifiers::NONE),
                &mut pending_fold,
                &root.join("preferences.toml"),
            );
        };

        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.take_request(), Some(Request::Overview));

        app.view = View::Overview;
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.take_request(), Some(Request::NextLiveInteraction));
        assert_eq!(app.view, View::Overview);
        press(&mut app, KeyCode::Char('l'));
        assert!(app.launcher.is_none(), "`l` moves in the grid");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.take_request(),
            Some(Request::ShowInteraction("1-right".into()))
        );

        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(
            app.take_request(),
            Some(Request::MessageInteraction("1-right".into())),
            "Shift-Tab wraps back to the last tile, and `i` messages it"
        );
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(
            app.take_request(),
            Some(Request::MessageInteraction("2-left".into())),
            "Tab wraps on to the first"
        );

        press(&mut app, KeyCode::Char('r'));
        assert!(app.overview.running_only);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.take_request(),
            Some(Request::ShowInteraction("2-left".into()))
        );
        press(&mut app, KeyCode::Char('r'));
        assert!(!app.overview.running_only);

        // `/` opens the search over the grid; the event loop then hands the
        // typed keys to the prompt. Esc clears a standing term before a
        // second one leaves the grid.
        press(&mut app, KeyCode::Char('/'));
        assert!(app.search.typing());
        for character in "retry backoff".chars() {
            handle_search_key(&mut app, KeyEvent::from(KeyCode::Char(character)));
        }
        // Ctrl-W takes back a word, as in every search field, rather than
        // typing a `w`.
        handle_search_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        handle_search_key(&mut app, KeyEvent::from(KeyCode::Backspace));
        handle_search_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.search.query(), Some("retry"));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.search.query(), None);
        assert_eq!(app.view, View::Overview);

        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.view, View::Events);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn grid_navigation_uses_the_live_list_pending_selection() {
        use styra_protocol::InteractionActivity;
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "one",
        );
        app.interactions.refresh(
            ["one", "two", "three", "four"]
                .into_iter()
                .map(|id| crate::interactions::tests::interaction(id, InteractionActivity::Running))
                .collect(),
        );
        app.view = View::Overview;
        app.overview.note_rows(&[2, 2]);
        for (key, target) in [
            (KeyCode::Right, Some("two")),
            (KeyCode::Down, Some("four")),
            (KeyCode::Left, Some("three")),
            (KeyCode::Up, None),
            (KeyCode::Char('G'), Some("four")),
            (KeyCode::Tab, None),
            (KeyCode::BackTab, Some("four")),
            (KeyCode::Char('g'), None),
            (KeyCode::Char('3'), Some("three")),
            (KeyCode::Char('9'), Some("three")),
            (KeyCode::Char('2'), Some("two")),
            (KeyCode::Char('1'), None),
        ] {
            assert!(handle_overview_key(&mut app, KeyEvent::from(key)));
            assert_eq!(
                app.interactions.pending("one").map(|item| item.id.as_str()),
                target,
                "{key:?} must schedule the same load as the live list"
            );
            assert_eq!(
                app.session_id, "one",
                "loading waits for the cursor to settle"
            );
            assert_eq!(app.view, View::Overview);
            assert!(app.take_request().is_none());
        }
    }

    #[test]
    fn overview_scope_key_toggles_the_shared_interaction_scope() {
        let mut app = crate::app::App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "current",
        );
        let mut current = crate::interactions::tests::interaction(
            "current",
            styra_protocol::InteractionActivity::Running,
        );
        current.workspace_id = "here".into();
        let mut other = current.clone();
        other.id = "other".into();
        other.workspace_id = "elsewhere".into();
        app.interactions.open(vec![current, other], vec![]);
        app.workspace.id = Some("here".into());
        app.view = View::Overview;
        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE);
        assert!(handle_overview_key(&mut app, key));
        assert!(app.interactions.only_current_workspace);
        assert_eq!(app.interactions.visible_indices("", Some("here")).len(), 1);
        assert_eq!(
            app.overview.indices(&app.interactions, Some("here")).len(),
            1
        );
        assert!(app
            .interactions
            .next_active("current", Some("here"))
            .is_none());
        // Changing scope in the live list is immediately reflected in the grid.
        app.interactions.toggle_workspace_scope();
        assert_eq!(
            app.overview.indices(&app.interactions, Some("here")).len(),
            2
        );
        assert!(handle_overview_key(&mut app, key));
        assert!(handle_overview_key(&mut app, key));
        assert!(!app.interactions.only_current_workspace);
    }

    #[test]
    fn messages_shortcut_opens_and_closes_the_interactions_log() {
        let root = tree("messages-shortcut");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        for expected in [View::Messages, View::Events] {
            handle_list_key(
                &mut app,
                &client,
                &mut live,
                KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT),
                &mut pending_fold,
                &root.join("preferences.toml"),
            );
            assert_eq!(app.view, expected);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn processes_shortcut_opens_the_view_and_asks_the_server() {
        let root = tree("processes-shortcut");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        for expected in [View::Processes, View::Events] {
            handle_list_key(
                &mut app,
                &client,
                &mut live,
                KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT),
                &mut pending_fold,
                &root.join("preferences.toml"),
            );
            assert_eq!(app.view, expected);
            assert!(matches!(app.take_request(), Some(Request::Processes)));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lowercase_h_opens_the_processes_pane_in_the_entry_logs_place() {
        let root = tree("processes-pane-shortcut");
        let mut app = app(&root);
        app.enter_list();
        app.view = View::Quota;
        app.toggle_entry_log();
        assert!(app.entry_log.open);
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        let mut press = |app: &mut App| {
            handle_list_key(
                app,
                &client,
                &mut live,
                KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
                &mut pending_fold,
                &root.join("preferences.toml"),
            )
        };
        app.view = View::Quota;
        press(&mut app);
        assert_eq!(app.view, View::Events);
        assert!(app.processes_pane && app.processes_visible());
        assert!(!app.entry_log.open);
        assert!(matches!(app.take_request(), Some(Request::Processes)));

        press(&mut app);
        assert!(!app.processes_pane && !app.processes_visible());

        press(&mut app);
        app.toggle_entry_log();
        assert!(app.entry_log.open && !app.processes_pane);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn control_l_toggles_the_log_view() {
        let root = tree("log-shortcut");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(app.view, View::Log);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn esc_returns_to_the_event_list_from_every_other_view() {
        let root = tree("esc-back");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        for view in [
            View::Raw,
            View::Messages,
            View::Log,
            View::Quota,
            View::Processes,
            View::Transcript,
            View::Driva,
            View::Files,
            View::Answer,
            View::Preview,
            View::Overview,
        ] {
            app.view = view;
            handle_list_key(
                &mut app,
                &client,
                &mut live,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut pending_fold,
                &root.join("preferences.toml"),
            );
            assert_eq!(app.view, View::Events, "Esc from {view:?}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn link_highlight_temporarily_opens_the_file_preview() {
        let root = tree("links-preview-restore");
        for open in [false, true] {
            for exit in [KeyCode::Esc, KeyCode::Enter, KeyCode::Char('M')] {
                let mut app = app(&root);
                app.preview.open = open;
                app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
                    text: "[notes](notes.txt) and [summary](reports/summary.md)".into(),
                });
                // Re-entering link navigation must preserve the original choice.
                for _ in 0..2 {
                    press_list_key(&mut app, &root, KeyCode::Char('f').into());
                    assert!(app.link_highlight.is_some());
                    assert!(app.preview.open);
                }
                assert_eq!(
                    app.highlighted_link_target().unwrap().1,
                    Ok(root.join("notes.txt"))
                );
                press_list_key(&mut app, &root, KeyCode::Char('j').into());
                assert_eq!(
                    app.highlighted_link_target().unwrap().1,
                    Ok(root.join("reports/summary.md"))
                );
                assert!(app.preview.open);
                press_list_key(&mut app, &root, KeyCode::Char('k').into());
                // An initially open preview is restored even if hidden here;
                // an initially closed one closes on exit without a manual toggle.
                if open {
                    press_list_key(&mut app, &root, KeyCode::Char('p').into());
                }
                assert_eq!(app.preview.open, !open);
                press_list_key(&mut app, &root, exit.into());
                assert!(app.link_highlight.is_none());
                assert_eq!(app.preview.open, open);
                if exit == KeyCode::Enter {
                    assert_eq!(
                        app.take_request(),
                        Some(Request::OpenPath(root.join("notes.txt")))
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// With a preview open the arrows scroll it ten lines, as `j`/`k` do the
    /// full-screen one, and `PgUp`/`PgDn` scroll it by half of what it shows.
    /// Full-screen `J`/`K` still move between previews; in the event list they
    /// scroll the interaction independently instead (covered below).
    #[test]
    fn arrows_scroll_an_open_preview_and_page_keys_move_half_of_it() {
        let root = tree("preview-arrows");
        let mut app = app(&root);
        app.enter_list();
        for index in 0..3 {
            app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
                text: format!("message {index}\n\nwith more below"),
            });
        }
        app.select_first();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        let mut press = |app: &mut App, code: KeyCode| {
            handle_list_key(
                app,
                &client,
                &mut live,
                KeyEvent::new(code, KeyModifiers::NONE),
                &mut pending_fold,
                &root.join("preferences.toml"),
            )
        };

        for view in [View::Events, View::Preview] {
            app.view = view;
            app.preview.show();
            app.preview.scroll.reset();
            app.preview.scroll.note_limit(100);
            app.preview.scroll.note_viewport(30);
            let selected = app.timeline.selected;

            press(&mut app, KeyCode::Down);
            assert_eq!(app.preview.scroll.offset, 10, "{view:?}: ↓ scrolls");
            assert_eq!(app.timeline.selected, selected, "{view:?}: and stays put");
            press(&mut app, KeyCode::PageDown);
            assert_eq!(app.preview.scroll.offset, 25, "{view:?}: half of 30");
            press(&mut app, KeyCode::PageUp);
            press(&mut app, KeyCode::Up);
            assert_eq!(app.preview.scroll.offset, 0, "{view:?}: back to the top");

            press(&mut app, KeyCode::Char('J'));
            if view == View::Preview {
                assert_ne!(app.timeline.selected, selected, "{view:?}: J moves");
            } else {
                assert_eq!(app.timeline.selected, selected, "{view:?}: J does not move");
                assert_eq!(app.timeline.list_scroll_delta, 10, "{view:?}: J scrolls");
                press(&mut app, KeyCode::Char('K'));
                assert_eq!(
                    app.timeline.list_scroll_delta, 0,
                    "{view:?}: K scrolls back"
                );
            }
            app.select_first();
        }
        let _ = std::fs::remove_dir_all(root);
    }

    /// Only these presses are handed to the event loop to branch; every other
    /// send goes out from the message box as it is.
    #[test]
    fn only_a_typed_message_sent_with_control_enter_branches() {
        let root = tree("creates-worktree");
        let mut app = app(&root);
        let press = |modifiers| KeyEvent::new(KeyCode::Enter, modifiers);

        app.composer.set("start here".into());
        assert!(creates_worktree(&app, press(KeyModifiers::CONTROL)));
        assert!(
            !creates_worktree(&app, press(KeyModifiers::NONE)),
            "a plain Enter sends into the current workspace"
        );

        app.composer.set("   ".into());
        assert!(
            !creates_worktree(&app, press(KeyModifiers::CONTROL)),
            "a blank box sends nothing, so it branches nothing"
        );

        app.composer.set("continue".into());
        app.session_id = "session-1".into();
        assert!(
            creates_worktree(&app, press(KeyModifiers::CONTROL)),
            "a Session already under way can still be branched"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    /// `Ctrl-Enter` asks for the same thing `Ctrl+W` does, carrying the prompt the
    /// new checkout is for, so both wait behind the event loop's one notice.
    #[test]
    fn control_enter_hands_the_first_prompt_to_the_worktree_request() {
        let root = tree("first-prompt-worktree");
        let mut app = app(&root);
        app.enter_input();
        app.composer.set("start here".into());
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;

        handle_input_key(
            &mut app,
            &client,
            "workspace-1",
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::CreateWorktree {
                message: Some("start here".into())
            })
        );
        assert!(app.composer.text().is_empty());
        assert_eq!(live, Attachment::Detached, "nothing launches from the box");
        let _ = std::fs::remove_dir_all(root);
    }

    /// Later in a Session the message rides the same request, and nothing is
    /// sent from the box: the interaction has to be moved before it hears it.
    #[test]
    fn control_enter_mid_session_hands_the_message_to_the_worktree_request() {
        let root = tree("later-message-worktree");
        let mut app = app(&root);
        app.session_id = "session-1".into();
        app.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);
        app.enter_input();
        app.composer.set("carry on in a branch".into());
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Attached { cursor: 0 };

        handle_input_key(
            &mut app,
            &client,
            "workspace-1",
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::CreateWorktree {
                message: Some("carry on in a branch".into())
            })
        );
        assert!(app.composer.text().is_empty());
        assert_eq!(live, Attachment::Attached { cursor: 0 });
        let _ = std::fs::remove_dir_all(root);
    }

    /// Moving a working agent would stop its turn, so the press is refused and
    /// the message is left where it was written.
    #[test]
    fn control_enter_mid_turn_is_refused_and_keeps_the_message() {
        let root = tree("mid-turn-worktree");
        let mut app = app(&root);
        app.session_id = "session-1".into();
        app.activity.status = Status::Running;
        app.enter_input();
        app.composer.set("carry on in a branch".into());
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Attached { cursor: 0 };

        handle_input_key(
            &mut app,
            &client,
            "workspace-1",
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );

        assert_eq!(app.take_request(), None);
        assert_eq!(app.composer.text(), "carry on in a branch");
        assert_eq!(live, Attachment::Attached { cursor: 0 });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn control_w_requests_a_worktree_for_an_existing_session() {
        let root = tree("session-worktree");
        let mut app = app(&root);
        app.enter_list();
        // `Ctrl+W` only means anything for a session that exists; the guard on the
        // key reads the id the list is sitting on.
        app.session_id = "session-1".into();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::CreateWorktree { message: None })
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enter_on_a_branch_marker_opens_the_linked_interaction() {
        let root = tree("follow-branch-enter");
        let mut app = app(&root);
        app.enter_list();
        app.push_event(styra_protocol::event::AgentEvent::Branched {
            direction: styra_protocol::event::BranchDirection::To,
            session: "branch-2".into(),
            name: None,
        });
        app.select_last();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::OpenSession("branch-2".into()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enter_on_a_from_branch_marker_opens_the_linked_interaction() {
        let root = tree("follow-branch-enter-source");
        let mut app = app(&root);
        app.enter_list();
        app.push_event(styra_protocol::event::AgentEvent::Branched {
            direction: styra_protocol::event::BranchDirection::From,
            session: "source-1".into(),
            name: None,
        });
        app.select_last();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::OpenSession("source-1".into()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enter_on_a_regular_event_still_toggles_its_expansion() {
        let root = tree("regular-enter");
        let mut app = app(&root);
        app.enter_list();
        app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
            text: "ordinary reply".into(),
        });
        app.select_last();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert!(app.timeline.selected_entry().unwrap().expanded);
        assert!(app.take_request().is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    /// `~` asks for a terminal wherever the interaction is working, with no
    /// live attachment needed: unlike `!`, which attaches to a running
    /// sandbox, a finished interaction still has a directory to stand in.
    #[test]
    fn tilde_asks_for_a_terminal_in_the_interaction_directory() {
        let root = tree("terminal-here");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('~'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );

        assert_eq!(app.take_request(), Some(Request::OpenDirectory));

        let _ = std::fs::remove_dir_all(&root);
    }

    fn press_list_key(app: &mut App, root: &Path, key: KeyEvent) {
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;
        handle_list_key(
            app,
            &client,
            &mut live,
            key,
            &mut pending_fold,
            &root.join("preferences.toml"),
        );
    }

    fn press_d(app: &mut App, root: &Path) {
        press_list_key(
            app,
            root,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
    }

    fn app_with_diffable_interaction(root: &Path) -> App {
        let mut app = app(root);
        app.enter_list();
        app.session_id = "styra-7".into();
        let mut interaction = crate::interactions::tests::interaction(
            "styra-7",
            styra_protocol::InteractionActivity::Pending,
        );
        interaction.checkout = Some(styra_protocol::CheckoutState {
            worktree: PathBuf::from("/state/worktrees/styra-7"),
            repository: PathBuf::from("/home/me/project"),
            branch: Some("styra/rename".into()),
        });
        interaction.branched_from = Some(styra_protocol::BranchPoint {
            branch: Some("main".into()),
            commit: "4bf5c35d".into(),
        });
        app.interactions.items = vec![interaction];
        app
    }

    fn diffable_target() -> DiffTarget {
        DiffTarget {
            worktree: PathBuf::from("/state/worktrees/styra-7"),
            branch: Some("styra/rename".into()),
            base_branch: Some("main".into()),
            base: "4bf5c35d".into(),
            branch_point: true,
        }
    }

    #[test]
    fn diff_keys_carry_the_selected_file_or_reference_location() {
        let root = tree("diff-selected-location");
        let mut app = app_with_diffable_interaction(&root);
        app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
            text: "see `notes.txt`".into(),
        });
        app.view = View::Files;
        press_d(&mut app, &root);
        assert_eq!(
            app.take_request(),
            Some(Request::ShowDiffAt {
                id: "styra-7".into(),
                path: root.join("notes.txt"),
                line: None,
                fullscreen: false,
            })
        );

        app.view = View::Events;
        app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
            text: "see [here](notes.txt:3)".into(),
        });
        app.select_last();
        app.highlight_first_link();
        press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Char('D')));
        assert_eq!(
            app.take_request(),
            Some(Request::ShowDiffAt {
                id: "styra-7".into(),
                path: root.join("notes.txt"),
                line: Some(3),
                fullscreen: true,
            })
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `d` asks the event loop to resolve this Session's diff base.
    #[test]
    fn d_asks_for_a_diff_of_the_checkout() {
        let root = tree("diff-checkout");
        let mut app = app_with_diffable_interaction(&root);

        press_d(&mut app, &root);

        assert_eq!(
            app.take_request(),
            Some(Request::ShowDiff("styra-7".into()))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `D` asks for the same diff fullscreen.
    #[test]
    fn shift_d_asks_for_the_same_diff_fullscreen() {
        let root = tree("diff-external");
        let mut app = app_with_diffable_interaction(&root);

        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
        );

        assert_eq!(
            app.take_request(),
            Some(Request::ShowDiffFullscreen("styra-7".into()))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The details view keeps `D` for saving the launch default, so no diff
    /// is asked for there.
    #[test]
    fn shift_d_in_the_details_view_is_not_a_diff() {
        let root = tree("diff-details");
        let mut app = app_with_diffable_interaction(&root);
        app.view = View::Driva;

        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
        );

        assert!(!matches!(
            app.take_request(),
            Some(Request::ShowDiffFullscreen(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preview_diff_can_be_scrolled_promoted_and_closed() {
        let root = tree("diff-preview");
        let mut app = app_with_diffable_interaction(&root);
        app.checkout_diff = Some(styra_diff::CheckoutDiff::read(diffable_target()));
        app.diff_preview = true;
        app.checkout_diff.as_mut().unwrap().scroll.note_limit(100);

        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 10);
        assert_eq!(app.view, View::Events);

        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!app.diff_visible());
        assert!(!app.preview.open);
        assert_eq!(app.view, View::Events);

        // A preview that was already open remains open after the diff closes.
        app.preview.show();
        app.diff_preview = true;
        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!app.diff_visible());
        assert!(app.preview.open);

        app.diff_preview = true;
        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
        );
        assert_eq!(app.view, View::CheckoutDiff);
        assert!(!app.diff_preview);
        let diff = app.checkout_diff.as_ref().unwrap();
        assert!(diff.per_file);
        assert_eq!(diff.scroll.offset, 0);
        assert_eq!(diff.combined_scroll.offset, 10);
        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!app.diff_visible());
        assert!(app.preview.open);

        // Promoting a temporary diff also preserves a closed preview.
        app.preview.toggle();
        app.diff_preview = true;
        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
        );
        assert_eq!(app.view, View::CheckoutDiff);
        press_list_key(
            &mut app,
            &root,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!app.diff_visible());
        assert!(!app.preview.open);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `u` swaps the diff between the checkout's and the selected turn's, the
    /// diff keys then acting on the turn's; closing the diff leaves the next
    /// one opened on the checkout's diff again.
    #[test]
    fn t_switches_the_diff_to_the_turn_diff_and_back() {
        let root = tree("diff-turn");
        let mut app = app_with_diffable_interaction(&root);
        app.push_event(styra_protocol::event::AgentEvent::UserMessage {
            text: "change a".into(),
        });
        app.push_event(styra_protocol::event::AgentEvent::FileChanged {
            id: String::new(),
            paths: vec!["a.txt".into()],
            diff: Some("@@ -1 +1 @@\n-a\n+b".into()),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.select_first();
        app.checkout_diff = Some(styra_diff::CheckoutDiff::read(diffable_target()));
        app.diff_preview = true;
        let press = |app: &mut App, code| press_list_key(app, &root, KeyEvent::from(code));

        press(&mut app, KeyCode::Char('u'));
        assert!(app.turn_diff_shown());
        let turn = app.shown_diff().unwrap();
        assert!(turn.target().is_none());
        assert!(turn.diff.as_deref().unwrap().contains("+b"));
        press(&mut app, KeyCode::Char('l'));
        assert!(app.shown_diff().unwrap().per_file);
        assert!(!app.checkout_diff.as_ref().unwrap().per_file);
        press(&mut app, KeyCode::Char('E'));
        assert!(app.take_request().is_none(), "no report for a turn diff");

        press(&mut app, KeyCode::Char('u'));
        assert!(!app.turn_diff_shown());
        assert!(app.shown_diff().unwrap().target().is_some());
        press(&mut app, KeyCode::Char('u'));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(!app.diff_visible());
        assert!(!app.diff_turn);

        // `D` takes the turn diff fullscreen as it is, where `u` switches
        // the same way, and leaving fullscreen forgets it.
        app.diff_preview = true;
        press(&mut app, KeyCode::Char('u'));
        press(&mut app, KeyCode::Char('D'));
        assert_eq!(app.view, View::CheckoutDiff);
        assert!(app.turn_diff_shown());
        assert!(app.shown_diff().unwrap().per_file, "the same turn diff");
        crate::presentation::test_support::inspect(&app, |v| {
            assert!(matches!(
                v.main,
                styra_ui::application::MainView::CheckoutDiff(_)
            ))
        });
        press(&mut app, KeyCode::Char('u'));
        assert!(!app.turn_diff_shown());
        assert!(app.shown_diff().unwrap().target().is_some());
        press(&mut app, KeyCode::Char('u'));
        assert!(app.turn_diff_shown());
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert_ne!(app.view, View::CheckoutDiff);
        assert!(!app.diff_turn);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diff_prompt_enter_reveals_latest_matching_message() {
        let root = tree("diff-prompt-jump");
        let mut app = app_with_diffable_interaction(&root);
        let mut diff = styra_diff::CheckoutDiff::read(diffable_target());
        diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n".into());
        diff.per_file = true;
        diff.line_cursor = Some(3);
        app.checkout_diff = Some(diff);
        app.view = View::CheckoutDiff;
        handle_diff_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            app.view,
            View::CheckoutDiff,
            "unmarked lines stay in the diff"
        );
        app.checkout_diff.as_mut().unwrap().line_cursor = Some(3);
        for text in [
            "file\n\nSource: a.txt",
            "old\n\nSource: a.txt:1",
            "latest\n\nSource: a.txt:1",
            "other\n\nSource: b.txt:1",
        ] {
            app.push_event(styra_protocol::event::AgentEvent::UserMessage { text: text.into() });
        }
        let expected = app.timeline.entries.len() - 2;
        assert_eq!(app.diff_prompts().len(), 4);
        app.checkout_diff.as_mut().unwrap().search.open();
        handle_diff_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(
            app.view,
            View::CheckoutDiff,
            "Enter accepts the filter first"
        );
        handle_diff_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.view, View::Events);
        assert_eq!(app.timeline.selected, expected);
        assert!(!app.timeline.follow);
        assert!(app.timeline.anchor_selection);
        assert!(app.timeline.entries[expected].expanded);

        app.view = View::CheckoutDiff;
        app.diff_preview = true;
        app.checkout_diff.as_mut().unwrap().line_cursor = None;
        handle_diff_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.timeline.selected, expected - 2, "file prompt");
        assert!(app.diff_interaction_focus, "side pane returns keys to log");
        let _ = std::fs::remove_dir_all(root);
    }

    /// `i` starts a message about what the diff is on — the file it shows,
    /// and the line cursor's line — and sending puts that under it.
    #[test]
    fn a_message_started_from_the_diff_notes_its_file_and_line() {
        let root = tree("diff-message-source");
        let mut app = app_with_diffable_interaction(&root);
        app.view = View::CheckoutDiff;
        let mut diff = styra_diff::CheckoutDiff::read(diffable_target());
        diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
                        diff --git a/b.txt b/b.txt\n@@ -4,2 +4,2 @@\n c\n-d\n+e\n"
            .into());
        diff.scroll.note_limit(100);
        app.checkout_diff = Some(diff);
        let press = |app: &mut App, character| {
            press_list_key(app, &root, KeyEvent::from(KeyCode::Char(character)))
        };

        press(&mut app, 'i');
        assert_eq!(app.focus, crate::app::Focus::Input);
        assert_eq!(
            app.message_source, None,
            "the combined diff is on no one file"
        );
        app.enter_list();

        press(&mut app, 'l');
        press(&mut app, 'j');
        press(&mut app, 'i');
        assert_eq!(app.message_source.as_deref(), Some("b.txt"));
        app.enter_list();
        assert_eq!(app.message_source, None, "leaving the box forgets it");

        press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Enter));
        press(&mut app, 'j');
        press(&mut app, 'j');
        assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 1);
        press(&mut app, 'i');
        assert_eq!(app.message_source.as_deref(), Some("b.txt:5"));
        app.composer.char('x');
        assert_eq!(app.take_message().as_deref(), Some("x\n\nSource: b.txt:5"));
        assert_eq!(app.message_source, None);

        app.enter_input();
        assert_eq!(app.message_source, None, "a message started elsewhere");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn per_file_diff_navigation_and_scrolling_use_separate_keys() {
        let root = tree("diff-files");
        let mut app = app_with_diffable_interaction(&root);
        app.enter_list();
        app.view = View::CheckoutDiff;
        app.checkout_diff = Some(styra_diff::CheckoutDiff {
            commits: Default::default(),
            source: styra_diff::Source::Checkout(diffable_target()),
            diff: Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n".into()),
            base_names: Vec::new(),
            scroll: Default::default(),
            combined_scroll: Default::default(),
            per_file: false,
            side_by_side: false,
            rendered_side: false,
            combined_side: false,
            help: None,
            horizontal_scroll: 0,
            hide_removed: false,
            compact_dirs: true,
            tint: Default::default(),
            word_highlights: true,
            layers: None,
            involved_layers: None,
            thumbnail_hidden: false,
            selected_file: 0,
                        file_list_offset: 0,
            search: Default::default(),
            report: Default::default(),
            report_page: None,
            report_preview: false,
            line_cursor: None,
            find: Default::default(),
            find_from: None,
        });
        let diff = app.checkout_diff.as_mut().unwrap();
        diff.scroll.note_limit(100);
        diff.scroll.note_viewport(30);
        let press = |app: &mut App, code| press_list_key(app, &root, KeyEvent::from(code));
        press(&mut app, KeyCode::Char('h'));
        assert!(app.checkout_diff.as_ref().unwrap().commits.focused);
        press(&mut app, KeyCode::Enter);
        assert!(!app.checkout_diff.as_ref().unwrap().commits.focused);
        assert!(app.checkout_diff.as_ref().unwrap().line_cursor.is_none());
        press(&mut app, KeyCode::Tab);
        assert!(app.checkout_diff.as_ref().unwrap().commits.focused);
        assert!(!app.diff_conversation, "Tab belongs to the open commit list");
        press(&mut app, KeyCode::Char('h'));
        assert!(!app.checkout_diff.as_ref().unwrap().commits.open);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 10);
        assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 0);
        press(&mut app, KeyCode::Char('r'));
        assert!(app.checkout_diff.as_ref().unwrap().hide_removed);
        // The place is kept rather than reset: "+d", row 10 of the eleven
        // with the rules around the headers, is row 8 once the two removed
        // lines are gone.
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 8);
        assert_eq!(app.view, View::CheckoutDiff, "r is the diff's here");
        press(&mut app, KeyCode::Char('r'));
        assert!(!app.checkout_diff.as_ref().unwrap().hide_removed);
        // Tab swaps the header for the latest messages and back, and is
        // nothing to the diff itself.
        press(&mut app, KeyCode::Tab);
        assert!(app.diff_conversation);
        assert!(!app.checkout_diff.as_ref().unwrap().per_file);
        assert_eq!(app.view, View::CheckoutDiff);
        press(&mut app, KeyCode::BackTab);
        assert!(!app.diff_conversation);
        press(&mut app, KeyCode::Char('l'));
        assert!(app.checkout_diff.as_ref().unwrap().per_file);
        // As drawing the per-file view would.
        let diff = app.checkout_diff.as_mut().unwrap();
        diff.scroll.note_limit(100);
        diff.scroll.note_viewport(30);
        press(&mut app, KeyCode::Char('C'));
        assert!(!app.checkout_diff.as_ref().unwrap().compact_dirs);
        assert_eq!(app.view, View::CheckoutDiff, "C is the diff's here");
        press(&mut app, KeyCode::Char('C'));
        assert!(app.checkout_diff.as_ref().unwrap().compact_dirs);
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(
            app.checkout_diff.as_ref().unwrap().tint,
            styra_diff::Tint::Half
        );
        assert_eq!(app.view, View::CheckoutDiff, "t is the diff's here");
        // `e` puts the edit report away and back; `E` asks for a new one.
        press(&mut app, KeyCode::Char('e'));
        assert!(app.checkout_diff.as_ref().unwrap().report.hidden);
        assert_eq!(app.view, View::CheckoutDiff, "e is the diff's here");
        press(&mut app, KeyCode::Char('e'));
        assert!(!app.checkout_diff.as_ref().unwrap().report.hidden);
        press(&mut app, KeyCode::Char('E'));
        assert_eq!(app.take_request(), Some(Request::GenerateEditReport(None)));
        assert_eq!(app.take_request(), None);
        press(&mut app, KeyCode::Char('r'));
        assert!(app.checkout_diff.as_ref().unwrap().hide_removed);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 10);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 25);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 10);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 0);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('j'));
        let diff = app.checkout_diff.as_ref().unwrap();
        assert_eq!(diff.selected_file, 1);
        assert_eq!(diff.scroll.offset, 0);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 1);
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 0);
        press(&mut app, KeyCode::Char('l'));
        assert!(!app.checkout_diff.as_ref().unwrap().per_file);
        assert!(app.checkout_diff.as_ref().unwrap().hide_removed);
        // Back where it was left, with removed lines hidden since: "+d", row 8.
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 8);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 9);

        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('B'));
        let diff = app.checkout_diff.as_ref().unwrap();
        assert!(diff.search.typing());
        assert_eq!(diff.search.query(), Some("B"));
        assert_eq!(diff.selected_file, 1);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 1);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::CheckoutDiff);
        assert_eq!(app.checkout_diff.as_ref().unwrap().search.query(), None);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 0);

        press(&mut app, KeyCode::Char('/'));
        for character in "dq?".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        assert_eq!(app.view, View::CheckoutDiff);
        assert_eq!(
            app.checkout_diff.as_ref().unwrap().search.query(),
            Some("dq?")
        );
        assert_eq!(app.take_request(), None);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        assert!(!app.checkout_diff.as_ref().unwrap().search.typing());
        assert_eq!(app.checkout_diff.as_ref().unwrap().search.query(), None);

        // Large steps clamp at the ends and count visible files when filtered.
        let diff = app.checkout_diff.as_mut().unwrap();
        diff.diff = Ok((0..25)
            .map(|index| {
                let group = if index % 2 == 0 { "match" } else { "other" };
                format!(
                    "diff --git a/{index:02}-{group}.txt b/{index:02}-{group}.txt\n@@ -1 +1 @@\n-a\n+b\n"
                )
            })
            .collect());
        diff.selected_file = 0;
        for (key, selected) in [
            ('J', 5),
            ('J', 10),
            ('J', 15),
            ('J', 20),
            ('J', 24),
            ('K', 19),
            ('K', 14),
            ('K', 9),
            ('K', 4),
            ('K', 0),
        ] {
            app.checkout_diff.as_mut().unwrap().scroll.offset = 7;
            press(&mut app, KeyCode::Char(key));
            let diff = app.checkout_diff.as_ref().unwrap();
            assert_eq!(diff.selected_file, selected);
            assert_eq!(diff.scroll.offset, 0);
        }
        press(&mut app, KeyCode::Char('/'));
        for character in "match".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Enter);
        for (key, selected) in [
            ('J', 10),
            ('J', 20),
            ('J', 24),
            ('K', 14),
            ('K', 4),
            ('K', 0),
        ] {
            press(&mut app, KeyCode::Char(key));
            assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, selected);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tab_switches_navigation_between_the_side_diff_and_interaction() {
        let root = tree("diff-tab-focus");
        for preview_open in [false, true] {
            let mut app = app_with_diffable_interaction(&root);
            for index in 0..3 {
                app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
                    text: format!("message {index}\n\nwith more below"),
                });
            }
            app.select_first();
            app.preview.open = preview_open;
            app.preview.scroll.note_limit(100);
            app.diff_preview = true;
            app.checkout_diff = Some(styra_diff::CheckoutDiff::read(diffable_target()));
            app.checkout_diff.as_mut().unwrap().scroll.note_limit(100);

            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Down));
            assert_eq!(app.timeline.selected, 0);
            assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 10);

            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Tab));
            assert!(app.diff_visible());
            assert!(!app.diff_focused());
            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Down));
            assert_eq!(app.timeline.selected, 1);
            assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 10);
            assert_eq!(app.preview.scroll.offset, 0);

            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::BackTab));
            assert!(app.diff_focused());
            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Down));
            assert_eq!(app.timeline.selected, 1);
            assert_eq!(app.checkout_diff.as_ref().unwrap().scroll.offset, 20);
            assert_eq!(app.preview.open, preview_open);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `v` asks for the file at the top of the diff, at the line there, and
    /// says why when that file is gone rather than asking for it.
    #[test]
    fn v_in_the_diff_opens_the_file_at_the_top_at_its_line() {
        let root = tree("diff-open");
        let mut app = app_with_diffable_interaction(&root);
        app.view = View::CheckoutDiff;
        let mut diff = styra_diff::CheckoutDiff::read(DiffTarget {
            worktree: root.clone(),
            ..diffable_target()
        });
        diff.diff = Ok("diff --git a/notes.txt b/notes.txt\n@@ -3 +3 @@\n-x\n+y\n\
                        diff --git a/gone.txt b/gone.txt\n@@ -1 +0,0 @@\n-gone\n"
            .into());
        diff.scroll.note_limit(100);
        diff.scroll.offset = 3;
        app.checkout_diff = Some(diff);
        let v = KeyEvent::from(KeyCode::Char('v'));

        press_list_key(&mut app, &root, v);
        assert_eq!(
            app.take_request(),
            Some(Request::OpenPathAt(root.join("notes.txt"), Some(3)))
        );
        assert_eq!(app.view, View::CheckoutDiff, "v is the diff's here");

        app.checkout_diff.as_mut().unwrap().scroll.offset = 5;
        press_list_key(&mut app, &root, v);
        assert_eq!(app.take_request(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn i_from_the_diff_focuses_the_message_and_esc_returns_to_the_diff() {
        let root = tree("diff-message-focus");
        for view in [View::Events, View::Files, View::CheckoutDiff] {
            let mut app = app_with_diffable_interaction(&root);
            app.view = view;
            app.diff_preview = view != View::CheckoutDiff;
            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Char('i')));
            assert_eq!(app.focus, crate::app::Focus::Input);
            assert!(app.diff_visible());
            assert!(!app.diff_focused());
            assert!(!app.preview.open);

            let client = Client::new(root.join("missing.sock"));
            let mut live = Attachment::Detached;
            handle_input_key(
                &mut app,
                &client,
                "workspace",
                &mut live,
                KeyEvent::from(KeyCode::Char('i')),
            );
            assert_eq!(app.composer.text(), "i");
            handle_input_key(
                &mut app,
                &client,
                "workspace",
                &mut live,
                KeyEvent::from(KeyCode::Esc),
            );
            assert!(app.diff_focused());
            assert_eq!(app.view, view);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn i_and_q_are_still_text_while_typing_the_diff_filter() {
        let root = tree("diff-filter-i");
        let mut app = app_with_diffable_interaction(&root);
        app.diff_preview = true;
        let mut diff = styra_diff::CheckoutDiff::read(diffable_target());
        diff.search.open();
        app.checkout_diff = Some(diff);
        press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Char('i')));
        press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Char('q')));
        press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Tab));
        assert!(app.diff_focused());
        assert_eq!(
            app.checkout_diff.as_ref().unwrap().search.query(),
            Some("iq")
        );
        assert_eq!(app.take_request(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn q_from_the_diff_quits_styra_in_either_layout() {
        let root = tree("diff-quit");
        for view in [View::Events, View::Files, View::CheckoutDiff] {
            let mut app = app_with_diffable_interaction(&root);
            app.view = view;
            app.diff_preview = view != View::CheckoutDiff;
            app.checkout_diff = Some(styra_diff::CheckoutDiff::read(diffable_target()));
            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Char('q')));
            assert_eq!(app.take_request(), Some(Request::Quit));
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Apart from the message and quit shortcuts, global keys do nothing in the diff,
    /// and d or Esc returns to events.
    #[test]
    fn d_and_esc_close_the_diff_view() {
        let root = tree("diff-exclusive");
        let mut app = app_with_diffable_interaction(&root);
        app.view = View::CheckoutDiff;

        for character in "DlawnAQPXe".chars() {
            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Char(character)));
            assert_eq!(app.view, View::CheckoutDiff, "{character} left the diff");
            assert_eq!(app.take_request(), None, "{character} asked for something");
            assert_eq!(
                app.focus,
                crate::app::Focus::List,
                "{character} moved the focus"
            );
        }
        for loaded in [false, true] {
            for code in [KeyCode::Char('d'), KeyCode::Esc] {
                for view in [View::Events, View::Files, View::CheckoutDiff] {
                    app.view = view;
                    app.diff_preview = view != View::CheckoutDiff;
                    app.checkout_diff =
                        loaded.then(|| styra_diff::CheckoutDiff::read(diffable_target()));
                    press_list_key(&mut app, &root, KeyEvent::from(code));
                    assert_eq!(
                        app.view,
                        if view == View::CheckoutDiff {
                            View::Events
                        } else {
                            view
                        }
                    );
                    assert!(!app.diff_visible());
                    assert_eq!(app.take_request(), None);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Session needs no branch point to open its snapshot diff.
    #[test]
    fn d_without_a_start_commit_requests_a_snapshot_diff() {
        let root = tree("diff-no-branch-point");
        let mut app = app(&root);
        app.enter_list();
        app.session_id = "styra-7".into();
        let mut interaction = crate::interactions::tests::interaction(
            "styra-7",
            styra_protocol::InteractionActivity::Pending,
        );
        interaction.checkout = Some(styra_protocol::CheckoutState {
            worktree: PathBuf::from("/home/me/project"),
            repository: PathBuf::from("/home/me/project"),
            branch: Some("main".into()),
        });
        app.interactions.items = vec![interaction];

        press_d(&mut app, &root);

        assert_eq!(
            app.take_request(),
            Some(Request::ShowDiff("styra-7".into()))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the prompt decides reaches the message: a path the sandbox already
    /// carries goes in under the name the agent knows it by.
    #[test]
    fn a_decided_path_goes_into_the_message_being_composed() {
        let root = tree("mounted");
        let mut app = app(&root);

        typed(&mut app, "reports/summary.md");

        assert!(app.insert.is_none());
        assert_eq!(app.composer.text(), "/workspace/reports/summary.md");
        assert!(app.launch.interaction.mounts.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// And a granted one also reaches the launch policy, as this
    /// interaction's own mount.
    #[test]
    fn a_granted_path_is_added_to_this_interactions_mounts() {
        let root = tree("granted");
        let outside = tree("granted-elsewhere");
        let mut app = app(&root);
        let host = outside.join("notes.txt");

        typed(&mut app, &host.display().to_string());
        handle_insert_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
        );

        assert!(app.insert.is_none());
        assert_eq!(
            app.launch.interaction.mounts,
            vec![LaunchMount {
                source: host.clone(),
                destination: None,
                writable: true,
            }]
        );
        assert_eq!(app.composer.text(), host.display().to_string());
        assert!(app
            .notices
            .iter()
            .any(|message| message.text.contains("to this interaction")));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// Highlight the one link in a reply naming `link`, and open its menu.
    fn link_menu_on(app: &mut App, link: &str) {
        app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
            text: format!("see [it]({link})"),
        });
        app.timeline.selected = app.timeline.entries.len() - 1;
        // The reply marks the interaction running; mounts are added between
        // runs, so put it back to where the policy can be edited.
        app.activity.status = crate::activity::Status::Pending;
        app.highlight_first_link();
        app.open_link_menu();
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_link_menu_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_link_menus_first_action_opens_the_link_as_enter_does() {
        let root = tree("link-open");
        let mut app = app(&root);
        link_menu_on(&mut app, &root.join("notes.txt").display().to_string());

        press(&mut app, KeyCode::Enter);

        assert!(app.link_menu.is_none());
        assert_eq!(
            app.take_request(),
            Some(Request::OpenPath(root.join("notes.txt")))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_link_menu_mounts_a_file_outside_the_sandbox_for_this_interaction() {
        let root = tree("link-mount");
        let outside = tree("link-mount-elsewhere");
        let host = outside.join("notes.txt");
        let mut app = app(&root);
        link_menu_on(&mut app, &host.display().to_string());

        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);

        assert!(app.link_menu.is_none());
        assert_eq!(
            app.launch.interaction.mounts,
            vec![LaunchMount {
                source: host.clone(),
                destination: None,
                writable: true,
            }]
        );
        assert!(
            app.link_highlight.is_some(),
            "mounting keeps the link highlighted"
        );
        assert!(app.take_request().is_none());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// An idle interaction takes the mount by restarting, as it does when the
    /// mount is added in the Driva view; a working one refuses it.
    #[test]
    fn the_link_menu_restarts_an_idle_interaction_to_mount() {
        let root = tree("link-mount-idle");
        let outside = tree("link-mount-idle-elsewhere");
        let host = outside.join("notes.txt");

        let mut working = app(&root);
        link_menu_on(&mut working, &host.display().to_string());
        working.activity.status = crate::activity::Status::Running;
        press(&mut working, KeyCode::Char('j'));
        press(&mut working, KeyCode::Enter);
        assert!(working.launch.interaction.mounts.is_empty());
        assert!(!working.restart_for_mounts);

        let mut idle = app(&root);
        link_menu_on(&mut idle, &host.display().to_string());
        idle.activity.status =
            crate::activity::Status::Idle(crate::activity::IdleReason::TurnComplete);
        assert!(!idle.can_edit_launch());
        press(&mut idle, KeyCode::Char('j'));
        press(&mut idle, KeyCode::Enter);
        assert_eq!(idle.launch.interaction.mounts.len(), 1);
        assert!(idle.restart_for_mounts);
        assert!(idle.notices.iter().any(|message| message
            .text
            .contains("restarting the interaction to apply it")));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn the_link_menu_does_not_mount_what_the_sandbox_already_carries() {
        let root = tree("link-mounted");
        let mut app = app(&root);
        link_menu_on(&mut app, "notes.txt");

        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);

        assert!(app.launch.interaction.mounts.is_empty());
        assert!(app.notices.iter().any(|message| message
            .text
            .contains("already in the sandbox at /workspace")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
