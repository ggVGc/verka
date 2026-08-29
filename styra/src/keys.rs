use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;

use crate::app::{App, Request, Status, View};
use crate::launch;
use crate::notes;
use crate::preferences;
use crate::session::{self, Live};
use crate::terminal;
use styra_server::{Client, Contract, LogEntry};

/// Keys for the launch picker: `j`/`k` within a column, `Tab`/`h`/`l` between
/// them, `Enter` to apply the choice to this workspace, `D` to also save it as
/// the standing default, `Esc`/`q` to leave it as it was.
///
/// Neither launches: before launch the operator's first message still starts
/// the agent. On a live session, confirming switches its model there and then
/// (see [`App::confirm_launcher`]).
pub fn handle_launcher_key(app: &mut App, key: KeyEvent, preferences_path: &Path) {
    let Some(launcher) = app.launcher.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('j' | 'J') | KeyCode::Down => launcher.next(),
        KeyCode::Char('k' | 'K') | KeyCode::Up => launcher.prev(),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => launcher.next_column(),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => launcher.prev_column(),
        KeyCode::Enter => confirm(app, preferences_path),
        KeyCode::Char('D') => {
            confirm(app, preferences_path);
            if let Err(error) = preferences::save_selection(preferences_path, &app.selection) {
                app.push_log(LogEntry::error(format!(
                    "could not save launch defaults: {error:#}"
                )));
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => app.cancel_launcher(),
        _ => {}
    }
}

/// Adopt the picker's choice, and remember the model it names so the picker
/// lists it first next time. The ordering is a convenience rather than a
/// setting, so failing to persist it is logged and no more.
fn confirm(app: &mut App, preferences_path: &Path) {
    app.confirm_launcher();
    if let Err(error) = preferences::save_recent_models(preferences_path, &app.recent_models) {
        app.push_log(LogEntry::error(format!(
            "could not save the model ordering: {error:#}"
        )));
    }
}

/// Keys for the driva view's "add a mount" prompt. It is modal — every
/// printable key is part of the path being typed, `?` included — so the event
/// loop routes keys here ahead of the keybind reference and every view and
/// global binding, exactly as it does for the notes editor.
pub fn handle_mount_prompt_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => launch::cancel_prompt(app),
        KeyCode::Enter => launch::confirm_prompt(app),
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

pub fn handle_list_key(
    app: &mut App,
    client: &Client,
    live: &mut Live,
    key: KeyEvent,
    pending_fold: &mut bool,
    preferences_path: &Path,
) {
    if std::mem::take(pending_fold) {
        match key.code {
            KeyCode::Char('R') => app.timeline.expand_all(),
            KeyCode::Char('M') => app.timeline.collapse_all(),
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Char('q') => return app.ask(Request::Quit),
        KeyCode::Char('s') => return session::interrupt_interaction(app, client, live),
        KeyCode::Char('S') => return session::pause_interaction(app, client, live),
        KeyCode::Char('b') => return session::branch_session(app, client),
        KeyCode::Char('!') => {
            let Live::Running { session_id, .. } = live else {
                return app.show_action_message("no live interaction to open a shell for");
            };
            match terminal::open_shell(client, session_id) {
                Ok(program) => app.show_action_message(format!("opened shell in {program}")),
                Err(error) => app.push_log(LogEntry::error(format!(
                    "could not open session shell: {error:#}"
                ))),
            }
            return;
        }
        KeyCode::Char('i') if app.view != View::Preview => return app.enter_input(),
        KeyCode::Char('r') => return app.toggle_raw(),
        KeyCode::Char('l') => return app.toggle_view(View::Log),
        KeyCode::Char('t') => return app.toggle_view(View::Transcript),
        KeyCode::Char('d') => return app.toggle_view(View::Driva),
        KeyCode::Char('f') => return app.toggle_files(),
        KeyCode::Char('X') => return app.toggle_answer(),
        KeyCode::Char('P') => return app.toggle_view(View::Preview),
        KeyCode::Char('L') => return app.open_launcher(),
        KeyCode::Char('E') => return notes::open(app),
        KeyCode::Char('a') if app.view != View::Files => return app.ask(Request::Interactions),
        KeyCode::Char('V') => return app.ask(Request::Workspace),
        KeyCode::Char('A') => return app.ask(Request::Sessions),
        KeyCode::Char('N') => return app.ask(Request::Reset),
        KeyCode::Char('n') => return app.ask(Request::NewSession),
        _ => {}
    }
    match app.view {
        View::Events => match key.code {
            KeyCode::Char('c') => app.toggle_conversation_only(),
            KeyCode::Char('v') if app.show_preview => app.toggle_preview_mode(),
            KeyCode::Char('C') if app.show_preview => app.toggle_preview_target(),
            KeyCode::PageDown if app.show_preview => app.preview.page_down(),
            KeyCode::PageUp if app.show_preview => app.preview.page_up(),
            KeyCode::Char('J') | KeyCode::Down => app.select_next(),
            KeyCode::Char('K') | KeyCode::Up => app.select_prev(),
            KeyCode::Char('j') => app.select_next_line(),
            KeyCode::Char('k') => app.select_prev_line(),
            KeyCode::Char(' ') | KeyCode::Enter => app.timeline.toggle_expand(),
            KeyCode::Char('o') => app.timeline.toggle_expand(),
            KeyCode::Char('O') => app.timeline.expand_only_selected(),
            KeyCode::Char('g') => app.select_first(),
            KeyCode::Char('G') => app.select_last(),
            KeyCode::Char('z') => *pending_fold = true,
            KeyCode::Char('m') => app.toggle_minor(),
            KeyCode::Char('p') => app.toggle_preview(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        View::Raw => match key.code {
            KeyCode::PageDown => app.raw_preview.page_down(),
            KeyCode::PageUp => app.raw_preview.page_up(),
            KeyCode::Char('j') | KeyCode::Down => app.raw_select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.raw_select_prev(),
            KeyCode::Char('g') => app.raw_select_first(),
            KeyCode::Char('G') => app.raw_select_last(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        View::Log => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.log_scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => app.log_scroll_up(),
            KeyCode::Char('g') => app.log_to_top(),
            KeyCode::Char('G') => app.log_to_bottom(),
            _ => {}
        },
        View::Transcript => match key.code {
            KeyCode::Char('c') => app.toggle_conversation_only(),
            KeyCode::Char('j') | KeyCode::Down => app.transcript_scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => app.transcript_scroll_up(),
            KeyCode::Char('g') => app.transcript_to_top(),
            KeyCode::Char('G') => app.transcript_to_bottom(),
            _ => {}
        },
        // Editing the launch policy. These keys deliberately avoid the letters
        // the global bindings above already claim (`t`, `n`, `d`, …), since
        // reaching the transcript or a new session from this view must keep
        // working while the policy is being edited.
        //
        // Every editing key acts on whichever of the two layers `Tab` has
        // focused, so there is one set of them to learn rather than one per
        // layer — and the view says which layer that is.
        View::Driva => match key.code {
            KeyCode::Tab | KeyCode::BackTab => launch::toggle_scope(app),
            KeyCode::Char('w') => launch::cycle_network(app),
            // `I` for whether this launch inherits: `S` is claimed globally
            // above (stopping the interaction) and never reaches this match.
            KeyCode::Char('I') => launch::toggle_standalone(app),
            KeyCode::Char('T') => {
                if app.allow_launch_edit() {
                    app.ask(Request::Templates);
                }
            }
            KeyCode::Char('m') => launch::open_prompt(app),
            // The mount nobody should have to type out: the checkout this
            // client was started in, writable.
            KeyCode::Char('g') => launch::add_git_history(app),
            KeyCode::Char('x') => launch::remove_selected_mount(app),
            // Mirrors `D` in the launch picker: keep this policy as the one a
            // brand-new client starts from, rather than only this session's.
            // Only this interaction's own settings are saved — the Workspace's
            // are already durable, and saving the merge would make every launch
            // elsewhere carry grants meant for this Workspace.
            KeyCode::Char('D') => {
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
            // Edits to the Workspace's layer are stored as they are made, so
            // this is only ever a retry after one of those failed to reach the
            // server. Needs the client, so the event loop does the asking.
            KeyCode::Char('W') => launch::store_workspace(app),
            // Move what this interaction added up into the Workspace's standing
            // policy, once it turns out not to be particular to this
            // conversation after all.
            KeyCode::Char('U') => launch::promote_to_workspace(app),
            KeyCode::Char('j') | KeyCode::Down => launch::select_next_mount(app),
            KeyCode::Char('k') | KeyCode::Up => launch::select_prev_mount(app),
            _ => {}
        },
        // Re-reading is on the capitals so `j` and `k` stay navigation, as
        // they are in every other view.
        View::Answer => match key.code {
            KeyCode::Char('T') => app.reread_answer(Contract::Text),
            KeyCode::Char('L') => app.reread_answer(Contract::Lines),
            KeyCode::Char('F') => app.reread_answer(Contract::Files),
            KeyCode::Char('J') => app.reread_answer(Contract::Json),
            KeyCode::Char('R') => app.ask(Request::Answer { contract: None }),
            KeyCode::Char('e') if app.selected_answer_file().is_some() => {
                app.ask(Request::EditFile)
            }
            KeyCode::Char('j') | KeyCode::Down => app.answer_select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.answer_select_prev(),
            KeyCode::Char('g') => app.answer_selected = 0,
            KeyCode::Char('G') => app.answer_selected = app.answer_rows().saturating_sub(1),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        View::Files => match key.code {
            KeyCode::Char('e') if app.selected_file_path().is_some() => app.ask(Request::EditFile),
            KeyCode::Char('j') | KeyCode::Down => app.file_select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.file_select_prev(),
            KeyCode::Char('J') => {
                app.select_next_line();
                app.file_selected = 0;
            }
            KeyCode::Char('K') => {
                app.select_prev_line();
                app.file_selected = 0;
            }
            KeyCode::Char('g') => app.file_selected = 0,
            KeyCode::Char('G') => app.file_selected = app.file_paths().len().saturating_sub(1),
            KeyCode::Char('a') => app.toggle_file_scope(),
            KeyCode::Char('p') => app.toggle_preview(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        // Full-screen preview is the one view where the text, not the entry
        // list, is what the reader is moving through: `j`/`k` scroll it a line
        // at a time and the shifted pair changes entry.
        View::Preview => match key.code {
            KeyCode::Char('v') => app.toggle_preview_mode(),
            KeyCode::Char('C') => app.toggle_preview_target(),
            KeyCode::PageDown => app.preview.page_down(),
            KeyCode::PageUp => app.preview.page_up(),
            KeyCode::Char('j') => app.preview.line_down(),
            KeyCode::Char('k') => app.preview.line_up(),
            KeyCode::Char('J') | KeyCode::Down => app.select_next_line(),
            KeyCode::Char('K') | KeyCode::Up => app.select_prev_line(),
            KeyCode::Char('g') => app.select_first(),
            KeyCode::Char('G') => app.select_last(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
    }
}

/// Copy whatever the current view treats as the selected entry to the
/// clipboard (see `App::copy_text`), reporting the outcome the same way
/// [`terminal::open_shell`](crate::terminal::open_shell) does.
fn copy_selection(app: &mut App) {
    let Some(text) = app.copy_text() else {
        return app.show_action_message("nothing selected to copy");
    };
    match crate::clipboard::copy(&text) {
        Ok(()) => app.show_action_message("copied to clipboard"),
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not copy to clipboard: {error:#}"
        ))),
    }
}

pub fn handle_input_key(
    app: &mut App,
    client: &Client,
    workspace_id: &str,
    live: &mut Live,
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Esc => app.enter_list(),
        // Choosing a shape is part of writing the message, so it lives in the
        // box rather than being a mode entered from outside it.
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => app.cycle_contract(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => app.composer.newline(),
        KeyCode::Enter => {
            if let Some(message) = app.take_message() {
                app.enter_list();
                if let Some(directory) = message.strip_prefix("/cd ") {
                    let Live::Running { session_id, .. } = live else {
                        return app
                            .push_log(LogEntry::warn("/cd requires a live Codex interaction"));
                    };
                    if directory.trim().is_empty() {
                        return app.push_log(LogEntry::warn("usage: /cd <directory>"));
                    }
                    match client
                        .set_interaction_working_directory(session_id, directory.trim().into())
                    {
                        Ok(()) => app.show_action_message(format!(
                            "working directory: {}",
                            directory.trim()
                        )),
                        Err(error) => app.push_log(LogEntry::error(format!(
                            "could not change working directory: {error:#}"
                        ))),
                    }
                    return;
                }
                // The contract belongs to this message, so it is taken here
                // and travels with it down whichever send path applies.
                let contract = app.take_contract();
                match live {
                    Live::Running { session_id, .. } if app.status == Status::Running => {
                        // The durable queue stores text alone, so a contract
                        // cannot ride it. Said plainly rather than dropped
                        // quietly: the operator asked for a shape and would
                        // otherwise get prose back with no explanation.
                        if contract.is_some() {
                            app.push_log(LogEntry::warn(
                                "queued messages cannot ask for a shape; this one was queued untyped",
                            ));
                        }
                        if let Err(error) = client.queue_message(session_id, &message) {
                            app.push_log(LogEntry::error(format!(
                                "could not persist queued message: {error:#}"
                            )));
                        }
                        app.queue_message(message);
                        app.push_log(LogEntry::info(format!(
                            "message queued ({} waiting)",
                            app.queued_message_count()
                        )));
                    }
                    Live::Running { session_id, .. }
                        if matches!(app.status, Status::Idle | Status::Background) =>
                    {
                        let turn = session::turn(&message, app, contract);
                        match client.send_turn(session_id, turn) {
                            Ok(()) => app.status = Status::Running,
                            Err(error) => {
                                app.push_log(LogEntry::error(format!("send failed: {error:#}")))
                            }
                        }
                    }
                    Live::Running { .. } | Live::Viewing => {
                        session::resume_and_send(app, client, live, message, contract)
                    }
                    Live::Pending => {
                        let selection = app.selection.clone();
                        let launch = app.launch.interaction.clone();
                        match session::create_session(
                            client,
                            &launch,
                            workspace_id,
                            &selection,
                            Some(&message),
                            contract,
                        ) {
                            Ok(info) => {
                                app.selection = info.selection;
                                app.workspace_id = Some(info.workspace_id);
                                app.session_id = info.id.clone();
                                app.session_name = info.name;
                                app.set_workspace_root(info.workspace);
                                app.launch.record(info.driva);
                                app.push_log(LogEntry::info(format!(
                                    "journal: {}",
                                    info.journal_path.display()
                                )));
                                app.status = Status::Running;
                                *live = Live::Running {
                                    session_id: info.id,
                                    cursor: info.updates_after,
                                };
                            }
                            Err(error) => {
                                app.push_log(LogEntry::error(format!(
                                    "could not launch the agent: {error:#}"
                                )));
                                app.set_input(message);
                                app.enter_input();
                            }
                        }
                    }
                }
            }
        }
        KeyCode::Backspace => app.composer.backspace(),
        KeyCode::Up => app.composer.history_previous(),
        KeyCode::Down => app.composer.history_next(),
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.composer.delete_word()
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => app.open_launcher(),
        KeyCode::Char(ch) => app.composer.char(ch),
        _ => {}
    }
}
