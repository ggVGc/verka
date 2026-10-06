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
use crate::preferences;
use crate::session::{self, Attachment};
use styra_protocol::{CompletionState, Contract, LogEntry};
use styra_server::Client;

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
        KeyCode::Backspace => app.search.backspace(),
        KeyCode::Char(character) if !character.is_control() => app.search.push(character),
        _ => {}
    }
}

/// Keys for the driva view's "add a mount" prompt. It is modal — every
/// printable key is part of the path being typed, `?` included — so the event
/// loop routes keys here ahead of the keybind reference and every view and
/// global binding.
pub fn handle_mount_prompt_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => launch::cancel_prompt(app),
        KeyCode::Enter => launch::confirm_prompt(app),
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(text) = app.launch.prompt.as_mut() {
                delete_path_part(text);
            }
        }
        KeyCode::Backspace => {
            if let Some(text) = app.launch.prompt.as_mut() {
                text.pop();
            }
        }
        KeyCode::Char(ch) if !ch.is_control() => {
            if let Some(text) = app.launch.prompt.as_mut() {
                text.push(ch);
            }
        }
        _ => {}
    }
}

/// Remove the last path component without erasing an absolute path's root.
/// This is deliberately path-oriented rather than readline's word-oriented:
/// a mount prompt begins with a filesystem path, and `/work/project` becomes
/// `/work` after `Ctrl-W`.
fn delete_path_part(path: &mut String) {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path.truncate(1);
        return;
    }
    let parent_end = trimmed.rfind('/').unwrap_or(0);
    if parent_end == 0 && trimmed.starts_with('/') {
        path.truncate(1);
    } else {
        path.truncate(parent_end);
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
                return app.show_action_message("no Workspace is selected");
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
                Err(error) => {
                    app.show_action_message(format!("could not set Git checkout: {error:#}"))
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(text) = app.git_repository_prompt.as_mut() {
                text.pop();
            }
        }
        KeyCode::Char(ch) if !ch.is_control() => {
            if let Some(text) = app.git_repository_prompt.as_mut() {
                text.push(ch);
            }
        }
        _ => {}
    }
}

/// Keys for the two-way branch choice. Confirming closes the modal before the
/// server call, so success can replace the screen and failure returns to it.
pub fn handle_branch_prompt_key(app: &mut App, client: &Client, key: KeyEvent) {
    let Some(prompt) = app.branch_prompt.as_mut() else {
        return;
    };
    match key {
        k if BRANCH_NEXT.matches(k) => prompt.select_next(),
        k if BRANCH_PREV.matches(k) => prompt.select_previous(),
        k if BRANCH_CONFIRM.matches(k) => {
            let at_ms = prompt.at_ms();
            let history = prompt.selected();
            app.branch_prompt = None;
            session::branch_session(app, client, at_ms, history);
        }
        k if BRANCH_CANCEL.matches(k) => app.branch_prompt = None,
        _ => {}
    }
}

pub fn handle_list_key(
    app: &mut App,
    client: &Client,
    live: &mut Attachment,
    key: KeyEvent,
    pending_fold: &mut bool,
    preferences_path: &Path,
) {
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
    match key {
        k if GLOBAL_QUIT.matches(k) => return app.ask(Request::Quit),
        // The event list is the bottom of the stack every other view is
        // opened over, so Esc goes straight back to it from any of them. The
        // modals that claim Esc for themselves (prompts, pickers, the
        // navigator, the reference) are answered before this is reached. A
        // link highlight is cleared first, as it is on the event list.
        k if GLOBAL_BACK.matches(k) && app.view != View::Events => {
            if app.link_highlight.is_some() {
                return app.clear_link_highlight();
            }
            app.view = View::Events;
            return;
        }
        k if GLOBAL_INTERRUPT.matches(k) => {
            return session::interrupt_interaction(app, client, live)
        }
        k if GLOBAL_STOP.matches(k) => return session::pause_interaction(app, client, live),
        k if GLOBAL_BRANCH.matches(k) => return session::open_branch_prompt(app),
        k if GLOBAL_SHELL.matches(k) => {
            let Attachment::Attached { .. } = live else {
                return app.show_action_message("no live interaction to open a shell for");
            };
            return app.ask(Request::OpenShell);
        }
        // Beside `!`, and deliberately not the same shell: `!` attaches to the
        // agent's sandbox, `~` opens the operator's own shell on the host,
        // standing where the interaction is working. That works with no live
        // interaction — a finished one still has a directory to look at.
        k if GLOBAL_DIRECTORY.matches(k) => return app.ask(Request::OpenDirectory),
        // Not from the overview, which shows no conversation for the message
        // to join.
        k if GLOBAL_FOCUS_MESSAGE.matches(k)
            && !matches!(app.view, View::Preview | View::Overview) =>
        {
            return app.enter_input()
        }
        // Global, unlike `y`: what it copies is the session's exchange, which
        // does not change with the view the operator happens to be in.
        k if GLOBAL_COPY_CONVERSATION.matches(k) => return copy_conversation(app),
        k if GLOBAL_RAW.matches(k) => return app.toggle_raw(),
        k if GLOBAL_LOG.matches(k) => return app.toggle_view(View::Log),
        k if GLOBAL_LAUNCHER.matches(k) => return app.open_launcher(),
        // Opening the view also refreshes it: the log lives in the daemon's
        // memory, so there is nothing local to show without asking.
        k if GLOBAL_QUOTA.matches(k) => {
            app.toggle_view(View::Quota);
            return app.ask(Request::Quota);
        }
        k if GLOBAL_TRANSCRIPT.matches(k) => return app.toggle_view(View::Transcript),
        // `e` toggles the pane below the event list. The files and answer
        // views keep the key for opening the editor, which is the one thing
        // `e` already meant there.
        k if GLOBAL_ENTRY_LOG.matches(k) && !matches!(app.view, View::Files | View::Answer) => {
            return app.toggle_entry_log()
        }
        k if GLOBAL_DETAILS.matches(k) => return app.toggle_view(View::Driva),
        k if GLOBAL_JUMP_BACK.matches(k) => return app.ask(Request::JumpBack),
        k if GLOBAL_AUTO_COMMIT.matches(k) => return toggle_auto_commit(app),
        k if GLOBAL_FILES.matches(k) && app.view != View::Answer => return app.toggle_files(),
        k if GLOBAL_FILES_ALIAS.matches(k)
            && !matches!(app.view, View::Events | View::Transcript | View::Preview) =>
        {
            return app.toggle_files()
        }
        k if GLOBAL_ANSWER.matches(k) => return app.toggle_answer(),
        k if GLOBAL_PREVIEW.matches(k) => return app.toggle_view(View::Preview),
        k if GLOBAL_NEW_SESSION.matches(k) => return app.ask(Request::NewSession),
        k if GLOBAL_INTERACTIONS.matches(k) && app.view != View::Files => {
            return app.ask(Request::Interactions)
        }
        // The raw view keeps `v` for switching to the provider's own record.
        k if GLOBAL_OVERVIEW.matches(k) && app.view != View::Raw => return app.toggle_overview(),
        k if GLOBAL_WORKSPACES.matches(k) => return app.ask(Request::Workspace),
        // Details keeps `w` for the network toggle it already meant there.
        k if GLOBAL_WORKTREES.matches(k) && app.view != View::Driva => {
            return app.ask(Request::Worktrees)
        }
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
            k if GLOBAL_TAGS.matches(k) => edit_current_interaction_tags(app, client),
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
            k if EVENTS_FOLLOW_BRANCH.matches(k) => session::follow_branch(app),
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
            k if GLOBAL_ENTRY_LOG_FOCUS.matches(k) && app.entry_log.open => {
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
            k if EVENTS_PAGE_DOWN.matches(k) && app.preview.open => {
                app.preview.scroll.half_page_down()
            }
            k if EVENTS_PAGE_UP.matches(k) && app.preview.open => app.preview.scroll.half_page_up(),
            // With the preview open the arrows read it, the way `j`/`k` do the
            // full-screen one; `J`/`K` still step between entries.
            k if EVENTS_PREVIEW_SCROLL_DOWN.matches(k) && app.preview.open => {
                app.preview.scroll.page_down()
            }
            k if EVENTS_PREVIEW_SCROLL_UP.matches(k) && app.preview.open => {
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

/// The overview's own keys. They are offered before the global ones, because
/// walking a grid needs `l`, which is the launcher everywhere else; anything
/// else falls through to them. Reports whether `key` was one of its own.
fn handle_overview_key(app: &mut App, key: KeyEvent) -> bool {
    let current = app.session_id.clone();
    match key {
        k if OVERVIEW_LEFT.matches(k) => app.overview.left(&app.interactions, &current),
        k if OVERVIEW_RIGHT.matches(k) => app.overview.right(&app.interactions, &current),
        k if OVERVIEW_DOWN.matches(k) => app.overview.down(&app.interactions, &current),
        k if OVERVIEW_UP.matches(k) => app.overview.up(&app.interactions, &current),
        k if OVERVIEW_FIRST.matches(k) => app.overview.first(&app.interactions),
        k if OVERVIEW_LAST.matches(k) => app.overview.last(&app.interactions),
        // Loading another interaction is a server round-trip the event loop
        // makes, as it does for the navigator.
        k if OVERVIEW_OPEN.matches(k) => {
            match app.overview.selected_id(&app.interactions, &current) {
                Some(id) => {
                    let id = id.to_owned();
                    app.ask(Request::ShowInteraction(id))
                }
                None => app.show_action_message("no interaction is running or idle"),
            }
        }
        k if OVERVIEW_CLOSE.matches(k) => app.view = View::Events,
        _ => return false,
    }
    true
}

/// Open the tag editor for the Interaction currently on screen. The
/// interaction snapshot is refreshed here because the log is useful even
/// before the live-interactions navigator has ever been opened.
fn edit_current_interaction_tags(app: &mut App, client: &Client) {
    let session_id = app.session_id.clone();
    let interactions = match client.list_interactions() {
        Ok(interactions) => interactions,
        Err(error) => {
            app.show_action_message(format!("could not list interactions: {error:#}"));
            return;
        }
    };
    let Some(selected_tags) = interactions
        .iter()
        .find(|item| item.id == session_id)
        .map(|item| item.tags.clone())
    else {
        app.show_action_message("current interaction is no longer available");
        return;
    };
    let tags = match client.list_tags() {
        Ok(tags) => tags,
        Err(error) => {
            app.show_action_message(format!("could not list tags: {error:#}"));
            return;
        }
    };
    app.interactions.refresh(interactions);
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
        Err(error) => app.show_action_message(format!("could not read provider raw: {error:#}")),
    }
}

/// Copy whatever the current view treats as the selected entry to the
/// clipboard (see `App::copy_text`).
fn copy_selection(app: &mut App) {
    let Some(text) = app.copy_text() else {
        return app.show_action_message("nothing selected to copy");
    };
    copy(app, text, "copied to clipboard");
}

/// Copy the session's whole conversation — messages, errors, and model
/// changes, without the tool calls between them.
fn copy_conversation(app: &mut App) {
    let Some(text) = app.conversation_text() else {
        return app.show_action_message("no conversation to copy yet");
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
        insert::Outcome::Notice(notice) => app.show_action_message(notice),
        insert::Outcome::Insert { path, notice } => {
            app.insert = None;
            app.composer.insert(&path.display().to_string());
            if let Some(notice) = notice {
                app.show_action_message(notice);
            }
        }
        insert::Outcome::Grant { mount, path } => {
            app.insert = None;
            let label = crate::mount::label(&mount);
            let message = match app.launch.add_interaction_mount(mount) {
                // The mount is a request, not a live change: nothing rebinds a
                // running sandbox. An idle one is restarted under it before
                // this message goes out; otherwise it waits for the launch.
                Ok(()) => {
                    app.note_mount_change();
                    if app.restart_for_mounts {
                        format!("added {label} — restarting the interaction to apply it")
                    } else {
                        format!("added {label} — applies when this Session next launches")
                    }
                }
                Err(reason) => reason.to_owned(),
            };
            app.composer.insert(&path.display().to_string());
            app.show_action_message(message);
        }
    }
}

/// Whether this keypress sends the message into a new Git workspace.
///
/// Ctrl-Enter is a distinct submission: it creates the Session's branch and
/// linked workspace as it sends the message, with no standing option to leak
/// into a later Session. On a first prompt the Session launches there; on a
/// later message the running interaction is moved there first, as `W` does.
///
/// Such a send is handed to the event loop rather than made here, the same
/// request `W` makes for a Session that already exists: branching blocks long
/// enough to need the notice only the loop can paint.
fn creates_worktree(app: &App, key: KeyEvent) -> bool {
    EDITOR_SEND_IN_BRANCH.matches(key)
        // Nothing is sent, and so nothing is branched, for a blank box.
        && !app.composer.text.trim().is_empty()
}

/// Ask the server to commit this interaction's turns as they end, or to stop.
/// A screen with no interaction yet has nothing to answer for: the one it
/// starts commits its turns if it is given a linked checkout of its own.
fn toggle_auto_commit(app: &mut App) {
    if app.session_id.is_empty() {
        return app.show_action_message(
            "no interaction yet — one started in its own worktree commits each turn",
        );
    }
    app.ask(Request::SetAutoCommit(!app.auto_commit));
}

pub fn handle_input_key(
    app: &mut App,
    client: &Client,
    workspace_id: &str,
    live: &mut Attachment,
    key: KeyEvent,
) {
    match key {
        k if GLOBAL_LEAVE_MESSAGE.matches(k) => {
            app.branch_needs_prompt_name = false;
            app.enter_list();
        }
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
                return app.show_action_message(
                    "the agent is mid-turn — wait for it to finish before moving it to a new Git workspace",
                );
            }
            if app.branch_needs_prompt_name {
                if let Some(name) =
                    styra_server::journal::name_from_message(Some(&app.composer.text))
                {
                    if let Err(error) = client.rename_session(&app.session_id, Some(&name)) {
                        return app
                            .show_action_message(format!("could not name the branch: {error:#}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_protocol::{
        AttributedMount, DrivaOptions, LaunchMount, Mount, MountAccess, MountOrigin,
    };

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
            Some("/home/op/project/crates")
        );

        handle_mount_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.launch.prompt.as_deref(), Some("/home/op/project"));

        app.launch.prompt = Some("/".into());
        handle_mount_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.launch.prompt.as_deref(), Some("/"));
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

    /// The grid takes `l` from the launcher to move right, `Enter` asks for
    /// the tile's interaction, and `v` toggles the overview from both sides.
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

        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.take_request(), Some(Request::Overview));

        app.view = View::Overview;
        press(&mut app, KeyCode::Char('l'));
        assert!(app.launcher.is_none(), "`l` moves in the grid");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.take_request(),
            Some(Request::ShowInteraction("1-right".into()))
        );

        press(&mut app, KeyCode::Char('v'));
        assert_eq!(app.view, View::Events);
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
            View::Log,
            View::Quota,
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

    /// `Ctrl-Enter` asks for the same thing `W` does, carrying the prompt the
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
        assert!(app.composer.text.is_empty());
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
        assert!(app.composer.text.is_empty());
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
        assert_eq!(app.composer.text, "carry on in a branch");
        assert_eq!(live, Attachment::Attached { cursor: 0 });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn uppercase_w_requests_a_worktree_for_an_existing_session() {
        let root = tree("session-worktree");
        let mut app = app(&root);
        app.enter_list();
        // `W` only means anything for a session that exists; the guard on the
        // key reads the id the list is sitting on.
        app.session_id = "session-1".into();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Attachment::Detached;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT),
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

    /// What the prompt decides reaches the message: a path the sandbox already
    /// carries goes in under the name the agent knows it by.
    #[test]
    fn a_decided_path_goes_into_the_message_being_composed() {
        let root = tree("mounted");
        let mut app = app(&root);

        typed(&mut app, "reports/summary.md");

        assert!(app.insert.is_none());
        assert_eq!(app.composer.text, "/workspace/reports/summary.md");
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
        assert_eq!(app.composer.text, host.display().to_string());
        assert!(app.notices.iter().any(|message| message
            .text
            .contains("applies when this Session next launches")));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
