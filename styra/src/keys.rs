use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;

use crate::activity::Status;
use crate::app::{App, Request, View};
use crate::insert;
use crate::launch;
use crate::preferences;
use crate::session::{self, Live};
use crate::terminal;
use styra_server::{Client, Contract, LogEntry, QueuedMessage};

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
/// global binding.
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
            let Live::Running { .. } = live else {
                return app.show_action_message("no live interaction to open a shell for");
            };
            match terminal::open_shell(client, &app.session_id) {
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
        // Opening the view also refreshes it: the log lives in the daemon's
        // memory, so there is nothing local to show without asking.
        KeyCode::Char('Q') => {
            app.toggle_view(View::Quota);
            return app.ask(Request::Quota);
        }
        KeyCode::Char('t') => return app.toggle_view(View::Transcript),
        KeyCode::Char('d') => return app.toggle_view(View::Driva),
        KeyCode::Char('f') => return app.toggle_files(),
        KeyCode::Char('X') => return app.toggle_answer(),
        KeyCode::Char('P') => return app.toggle_view(View::Preview),
        KeyCode::Char('L') => return app.open_launcher(),
        KeyCode::Char('a') if app.view != View::Files => return app.ask(Request::Interactions),
        KeyCode::Char('V') => return app.ask(Request::Workspace),
        KeyCode::Char('W') => {
            return app.ask(Request::SetWorktreesEnabled(
                !app.workspace.worktrees_enabled,
            ))
        }
        KeyCode::Char('A') => return app.ask(Request::Sessions),
        KeyCode::Char('N') => return app.ask(Request::Reset),
        KeyCode::Char('n') => return app.ask(Request::NewSession),
        _ => {}
    }
    match app.view {
        View::Events => match key.code {
            KeyCode::Char('c') => app.toggle_conversation_only(),
            KeyCode::Char('v') if app.preview.open => app.preview.toggle_mode(),
            KeyCode::Char('C') if app.preview.open => app.preview.toggle_target(),
            KeyCode::PageDown if app.preview.open => app.preview.scroll.page_down(),
            KeyCode::PageUp if app.preview.open => app.preview.scroll.page_up(),
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
            KeyCode::Char('p') => app.preview.toggle(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        View::Raw => match key.code {
            KeyCode::PageDown => app.raw.preview.page_down(),
            KeyCode::PageUp => app.raw.preview.page_up(),
            KeyCode::Char('j') | KeyCode::Down => app.raw.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.raw.select_prev(),
            KeyCode::Char('g') => app.raw.select_first(),
            KeyCode::Char('G') => app.raw.select_last(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        View::Log => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.log.scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => app.log.scroll_up(),
            KeyCode::Char('g') => app.log.scroll_to_top(),
            KeyCode::Char('G') => app.log.scroll_to_bottom(),
            _ => {}
        },
        View::Quota => match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.quota.scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => app.quota.scroll_up(),
            _ => {}
        },
        View::Transcript => match key.code {
            KeyCode::Char('c') => app.toggle_conversation_only(),
            KeyCode::Char('j') | KeyCode::Down => app.transcript.line_down(),
            KeyCode::Char('k') | KeyCode::Up => app.transcript.line_up(),
            KeyCode::Char('g') => app.transcript.reset(),
            KeyCode::Char('G') => app.transcript.scroll_to_end(),
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
            KeyCode::Char('e') if app.answer.selected_file().is_some() => {
                app.ask(Request::EditFile)
            }
            KeyCode::Char('j') | KeyCode::Down => app.answer.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.answer.select_prev(),
            KeyCode::Char('g') => app.answer.select_first(),
            KeyCode::Char('G') => app.answer.select_last(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        View::Files => match key.code {
            KeyCode::Char('e') if app.selected_file_path().is_some() => app.ask(Request::EditFile),
            KeyCode::Char('j') | KeyCode::Down => app.file_select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.file_select_prev(),
            KeyCode::Char('J') => {
                app.select_next_line();
                app.files.select_first();
            }
            KeyCode::Char('K') => {
                app.select_prev_line();
                app.files.select_first();
            }
            KeyCode::Char('g') => app.files.select_first(),
            KeyCode::Char('G') => {
                let last = app.file_paths().len().saturating_sub(1);
                app.files.select_last(last);
            }
            KeyCode::Char('a') => app.toggle_file_scope(),
            KeyCode::Char('p') => app.preview.toggle(),
            KeyCode::Char('y') => copy_selection(app),
            _ => {}
        },
        // Full-screen preview is the one view where the text, not the entry
        // list, is what the reader is moving through: `j`/`k` scroll it a line
        // at a time and the shifted pair changes entry.
        View::Preview => match key.code {
            KeyCode::Char('v') => app.preview.toggle_mode(),
            KeyCode::Char('C') => app.preview.toggle_target(),
            KeyCode::PageDown => app.preview.scroll.page_down(),
            KeyCode::PageUp => app.preview.scroll.page_up(),
            KeyCode::Char('j') => app.preview.scroll.line_down(),
            KeyCode::Char('k') => app.preview.scroll.line_up(),
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

/// Open the path prompt over the message box, against what this session's
/// sandbox carries and whether that sandbox can still be changed.
fn open_insert(app: &mut App) {
    app.insert = Some(insert::Prompt::new(
        app.workspace.root().map(std::path::Path::to_path_buf),
        app.launch.driva.as_ref(),
        app.can_edit_launch(),
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
                // The mount is a request, not a live change: nothing rebinds
                // the sandbox of an interaction that has already started, so
                // the message says when it will actually apply rather than
                // only that it was added.
                Ok(()) => format!("added {label} — applies when this Session next launches"),
                Err(reason) => reason.to_owned(),
            };
            app.composer.insert(&path.display().to_string());
            app.show_action_message(message);
        }
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
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.outbox.cycle_contract()
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => app.composer.newline(),
        KeyCode::Enter => {
            if let Some(message) = app.take_message() {
                app.enter_list();
                if let Some(directory) = message.strip_prefix("/cd ") {
                    let Live::Running { .. } = live else {
                        return app
                            .push_log(LogEntry::warn("/cd requires a live Codex interaction"));
                    };
                    if directory.trim().is_empty() {
                        return app.push_log(LogEntry::warn("usage: /cd <directory>"));
                    }
                    match client
                        .set_interaction_working_directory(&app.session_id, directory.trim().into())
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
                let contract = app.outbox.take_contract();
                match live {
                    Live::Running { .. } if app.activity.status == Status::Running => {
                        // Queued as composed, contract included: the shape was
                        // chosen for this question and is asked for whenever
                        // the agent gets to it.
                        let turn = session::turn(&message, &app.selection, contract);
                        if let Err(error) = client.queue_turn(&app.session_id, turn) {
                            app.push_log(LogEntry::error(format!(
                                "could not persist queued message: {error:#}"
                            )));
                        }
                        app.outbox
                            .queue(QueuedMessage::new(message).asking_for(contract));
                        app.push_log(LogEntry::info(format!(
                            "message queued ({} waiting)",
                            app.outbox.queued_count()
                        )));
                    }
                    Live::Running { .. }
                        if matches!(app.activity.status, Status::Idle | Status::Background) =>
                    {
                        let turn = session::turn(&message, &app.selection, contract);
                        match client.send_turn(&app.session_id, turn) {
                            Ok(()) => app.activity.status = Status::Running,
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
                                app.workspace.id = Some(info.workspace_id);
                                app.session_id = info.id.clone();
                                app.session_name = info.name;
                                app.workspace.enter(info.workspace);
                                app.launch.record(info.driva);
                                app.push_log(LogEntry::info(format!(
                                    "journal: {}",
                                    info.journal_path.display()
                                )));
                                app.activity.status = Status::Running;
                                *live = Live::Running {
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
        // Naming a file is part of writing the message, so it opens from the
        // box rather than from the driva view that the grant it may ask for
        // would otherwise have to be made in.
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => open_insert(app),
        KeyCode::Char(ch) => app.composer.char(ch),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_server::{
        AttributedMount, DrivaOptions, LaunchMount, Mount, MountAccess, MountOrigin,
    };

    /// A session whose sandbox binds `root` at `/workspace` and nothing else,
    /// with nothing launched — so the launch policy is still open to editing.
    fn app(root: &Path) -> App {
        let mut app = App::pending(styra_server::agent::Selection::parse("codex").unwrap());
        app.workspace.enter(root.to_path_buf());
        app.launch.record(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/workspace"),
            network: false,
            mounts: vec![AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source: root.to_path_buf(),
                    destination: PathBuf::from("/workspace"),
                    access: MountAccess::ReadWrite,
                },
            }],
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
    fn uppercase_w_requests_the_opposite_worktree_state() {
        let root = tree("worktree-toggle");
        let mut app = app(&root);
        app.enter_list();
        let client = Client::new(root.join("missing.sock"));
        let mut live = Live::Pending;
        let mut pending_fold = false;

        handle_list_key(
            &mut app,
            &client,
            &mut live,
            KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT),
            &mut pending_fold,
            &root.join("preferences.toml"),
        );
        assert_eq!(app.take_request(), Some(Request::SetWorktreesEnabled(true)));

        app.workspace.worktrees_enabled = true;
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
            Some(Request::SetWorktreesEnabled(false))
        );

        let _ = std::fs::remove_dir_all(root);
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
