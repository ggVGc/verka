use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;

use crate::app::{App, Status, View};
use crate::cli::Cli;
use crate::preferences;
use crate::session::{self, Live};
use styra_server::{Client, LogEntry};

/// Keys for the launch picker: `j`/`k` within a column, `Tab`/`h`/`l` between
/// them, `Enter` to apply the choice to this workspace, `D` to also save it as
/// the standing default (neither launches — the operator's first message still
/// does that), `Esc`/`q` to leave it as it was.
pub fn handle_launcher_key(app: &mut App, key: KeyEvent, preferences_path: &Path) {
    let Some(launcher) = app.launcher.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => launcher.next(),
        KeyCode::Char('k') | KeyCode::Up => launcher.prev(),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => launcher.next_column(),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => launcher.prev_column(),
        KeyCode::Enter => app.confirm_launcher(),
        KeyCode::Char('D') => {
            app.confirm_launcher();
            if let Err(error) = preferences::save(preferences_path, &app.selection) {
                app.push_log(LogEntry::error(format!(
                    "could not save launch defaults: {error:#}"
                )));
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => app.cancel_launcher(),
        _ => {}
    }
}

pub fn handle_list_key(
    app: &mut App,
    client: &Client,
    live: &mut Live,
    key: KeyEvent,
    pending_fold: &mut bool,
) {
    if std::mem::take(pending_fold) {
        match key.code {
            KeyCode::Char('R') => app.expand_all(),
            KeyCode::Char('M') => app.collapse_all(),
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Char('q') => return app.request_quit(),
        KeyCode::Char('s') => return session::pause_interaction(app, client, live),
        KeyCode::Char('i') if app.view != View::Preview => return app.enter_input(),
        KeyCode::Char('r') => return app.toggle_raw(),
        KeyCode::Char('l') => return app.toggle_log(),
        KeyCode::Char('t') => return app.toggle_transcript(),
        KeyCode::Char('d') => return app.toggle_driva(),
        KeyCode::Char('P') => return app.toggle_fullscreen_preview(),
        KeyCode::Char('L') => return app.open_launcher(),
        KeyCode::Char('a') => return app.request_sessions(),
        KeyCode::Char('V') => return app.request_workspace(),
        KeyCode::Char('A') => return app.request_interactions(),
        KeyCode::Char('S') => return app.request_reset(),
        _ => {}
    }
    match app.view {
        View::Events => match key.code {
            KeyCode::Char('c') => app.toggle_conversation_only(),
            KeyCode::Char('v') if app.show_preview => app.toggle_preview_mode(),
            KeyCode::PageDown if app.show_preview => app.preview_page_down(),
            KeyCode::PageUp if app.show_preview => app.preview_page_up(),
            KeyCode::Char('j') | KeyCode::Down => app.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
            KeyCode::Char('J') => app.select_next_line(),
            KeyCode::Char('K') => app.select_prev_line(),
            KeyCode::Char(' ') | KeyCode::Enter => app.toggle_expand(),
            KeyCode::Char('o') => app.toggle_expand(),
            KeyCode::Char('O') => app.expand_only_selected(),
            KeyCode::Char('g') => app.select_first(),
            KeyCode::Char('G') => app.select_last(),
            KeyCode::Char('z') => *pending_fold = true,
            KeyCode::Char('m') => app.toggle_minor(),
            KeyCode::Char('p') => app.toggle_preview(),
            KeyCode::Char('C') => app.expand_conversation(),
            _ => {}
        },
        View::Raw => match key.code {
            KeyCode::PageDown => app.raw_preview_page_down(),
            KeyCode::PageUp => app.raw_preview_page_up(),
            KeyCode::Char('j') | KeyCode::Down => app.raw_select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.raw_select_prev(),
            KeyCode::Char('g') => app.raw_select_first(),
            KeyCode::Char('G') => app.raw_select_last(),
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
        View::Driva => {}
        View::Preview => match key.code {
            KeyCode::Char('v') => app.toggle_preview_mode(),
            KeyCode::PageDown => app.preview_page_down(),
            KeyCode::PageUp => app.preview_page_up(),
            KeyCode::Char('j') | KeyCode::Down => app.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
            KeyCode::Char('J') => app.select_next_line(),
            KeyCode::Char('K') => app.select_prev_line(),
            KeyCode::Char('g') => app.select_first(),
            KeyCode::Char('G') => app.select_last(),
            _ => {}
        },
    }
}

pub fn handle_input_key(
    app: &mut App,
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    live: &mut Live,
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Esc => app.enter_list(),
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => app.input_newline(),
        KeyCode::Enter => {
            if let Some(message) = app.take_message() {
                app.enter_list();
                match live {
                    Live::Running { session_id, .. } if app.status == Status::Running => {
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
                    Live::Running { session_id, .. } if app.status == Status::Idle => {
                        if let Err(error) = client.send_message(session_id, &message) {
                            app.push_log(LogEntry::error(format!("send failed: {error:#}")));
                        }
                    }
                    Live::Running { .. } | Live::Viewing => {
                        session::resume_and_send(app, client, cli, live, message)
                    }
                    Live::Pending => {
                        let selection = app.selection.clone();
                        match session::create_session(
                            client,
                            cli,
                            workspace_id,
                            &selection,
                            Some(&message),
                        ) {
                            Ok(info) => {
                                app.selection = info.selection;
                                app.workspace_id = Some(info.workspace_id);
                                app.session_id = info.id.clone();
                                app.set_workspace_root(info.workspace);
                                app.set_driva_options(info.driva);
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
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Up => app.input_history_previous(),
        KeyCode::Down => app.input_history_next(),
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input_delete_word()
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => app.open_launcher(),
        KeyCode::Char(ch) => app.input_char(ch),
        _ => {}
    }
}
