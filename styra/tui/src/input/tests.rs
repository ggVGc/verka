use super::*;
use crate::activity::Status;
use crate::keyboard::Binding;
use std::path::PathBuf;
use styra_diff::DiffTarget;
use styra_protocol::{AttributedMount, DrivaOptions, LaunchMount, Mount, MountAccess, MountOrigin};
use styra_ui::event_list::ListAnchor;

#[test]
fn branch_input_shares_editing_and_keeps_cancellation_and_model_changes_local() {
    let selection = styra_protocol::agent::Selection::parse("codex").unwrap();
    let mut app = App::new(selection.clone(), "source");
    app.activity.status = Status::Running;
    app.composer.set("source draft".into());
    app.branch_prompt = Some(crate::branch::BranchPrompt::new(42, selection.clone()));
    let client = Client::new(PathBuf::from("/missing.sock"));
    // Empty sends do not create or dismiss the branch.
    handle_branch_prompt_key(&mut app, &client, press(&EDITOR_SEND));
    assert!(app.branch_prompt.is_some());
    for ch in "q j k h l ?".chars() {
        handle_branch_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Char(ch)));
    }
    assert_eq!(
        app.branch_prompt.as_ref().unwrap().composer.message(),
        "q j k h l ?"
    );
    handle_branch_prompt_key(&mut app, &client, press(&EDITOR_NEWLINE));
    handle_branch_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Char('x')));
    handle_branch_prompt_key(&mut app, &client, press(&EDITOR_ADD_BOX));
    app.editor_composer_mut().paste("second box");
    assert_eq!(
        app.branch_prompt.as_ref().unwrap().composer.parts(),
        ["q j k h l ?\nx", "", "second box", ""]
    );
    for picker in [
        EDITOR_LAUNCHER,
        EDITOR_CYCLE_PROVIDER,
        EDITOR_CYCLE_MODEL,
        EDITOR_CYCLE_EFFORT,
    ] {
        handle_branch_prompt_key(&mut app, &client, press(&picker));
        assert!(app.launcher.is_some());
        app.cancel_launcher();
    }
    app.launcher = Some(crate::launcher::Launcher::from_selection(
        &styra_protocol::agent::Selection::parse("claude").unwrap(),
        &[],
        false,
    ));
    app.confirm_launcher();
    assert_eq!(
        app.branch_prompt.as_ref().unwrap().selection.provider,
        styra_protocol::agent::Provider::Claude
    );
    assert_eq!(app.selection, selection);
    assert!(app.take_request().is_none());
    handle_branch_prompt_key(&mut app, &client, press(&EDITOR_CHOOSE_BOX));
    assert!(app.branch_prompt.as_ref().unwrap().composer.choosing());
    handle_branch_prompt_key(&mut app, &client, press(&EDITOR_BOX_LEAVE));
    assert!(app.branch_prompt.is_none());
    assert_eq!(app.composer.message(), "source draft");
}

#[test]
fn ctrl_b_moves_the_message_to_a_branch_and_back() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "source",
    );
    session::apply_update(
        &mut app,
        styra_protocol::InteractionUpdate::Event(styra_protocol::event::AgentEvent::UserMessage {
            text: "question".into(),
        }),
    );
    session::apply_update(
        &mut app,
        styra_protocol::InteractionUpdate::Raw(styra_protocol::RawLine {
            direction: styra_protocol::Direction::ToAgent,
            text: "question".into(),
            at_ms: 42,
        }),
    );
    app.enter_input();
    app.composer.set("try it the other way".into());
    let client = Client::new(PathBuf::from("/missing.sock"));
    let mut live = Attachment::Detached;
    let ctrl_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);

    handle_input_key(&mut app, &client, "workspace", &mut live, ctrl_b);
    let prompt = app.branch_prompt.as_ref().unwrap();
    assert_eq!(prompt.at_ms(), 42);
    assert_eq!(prompt.composer.message(), "try it the other way");
    assert_eq!(app.composer.message(), "");

    handle_branch_prompt_key(&mut app, &client, ctrl_b);
    assert!(app.branch_prompt.is_none());
    assert_eq!(app.composer.message(), "try it the other way");
    assert_eq!(app.focus, crate::app::Focus::Input);

    handle_input_key(&mut app, &client, "workspace", &mut live, ctrl_b);
    handle_branch_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Esc));
    assert!(app.branch_prompt.is_none());
    assert_eq!(app.composer.message(), "try it the other way");
}

#[test]
fn failed_branch_creation_preserves_the_instruction() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "source",
    );
    let mut prompt = crate::branch::BranchPrompt::new(42, app.selection.clone());
    prompt.composer.set("keep this instruction".into());
    app.branch_prompt = Some(prompt);
    let client = Client::new(PathBuf::from("/missing.sock"));
    handle_branch_prompt_key(&mut app, &client, press(&EDITOR_SEND));
    assert_eq!(
        app.branch_prompt.as_ref().unwrap().composer.message(),
        "keep this instruction"
    );
    assert!(app.take_request().is_none());
}

#[test]
fn rename_from_events_prefills_edits_and_cancels_without_changing_the_name() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "session-1",
    );
    app.interactions
        .note_name("session-1", Some("Original".into()));
    let client = Client::new(PathBuf::from("/missing.sock"));
    run_list_action(&mut app, Path::new("/missing"), &EVENTS_RENAME);
    assert_eq!(app.rename_prompt.as_deref(), Some("Original"));
    handle_rename_prompt_key(
        &mut app,
        &client,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    handle_rename_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Char('新')));
    assert_eq!(app.rename_prompt.as_deref(), Some("新"));
    handle_rename_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Enter));
    assert_eq!(
        app.rename_prompt.as_deref(),
        Some("新"),
        "a failed save retains the draft"
    );
    handle_rename_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Esc));
    assert!(app.rename_prompt.is_none());
    assert_eq!(app.interactions.name("session-1"), Some("Original"));
}

#[test]
fn rename_ctrl_g_asks_for_a_generated_name_and_keeps_the_draft() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "session-1",
    );
    app.rename_prompt = Some("Draft".into());
    let client = Client::new(PathBuf::from("/missing.sock"));
    handle_rename_prompt_key(
        &mut app,
        &client,
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.take_request(), Some(Request::GenerateSessionName));
    assert_eq!(app.rename_prompt.as_deref(), Some("Draft"));
}

#[test]
fn rename_saves_and_clears_the_current_interactions_name() {
    struct RenameServer;
    impl styra_server::InProcessServer for RenameServer {
        fn handle(
            &self,
            request: styra_protocol::Request,
        ) -> anyhow::Result<styra_protocol::Response> {
            let styra_protocol::Request::RenameSession(request) = request else {
                anyhow::bail!("unexpected request");
            };
            assert_eq!(request.id, "session-1");
            Ok(styra_protocol::Response::SessionRenamed(
                styra_protocol::SessionSummary {
                    id: request.id,
                    name: request.name,
                    first_prompt: None,
                    tags: Vec::new(),
                    workspace_id: "workspace-1".into(),
                    path: "/sessions/session-1".into(),
                    selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
                    age: String::new(),
                    created_at_ms: None,
                    last_event_at_ms: None,
                    last_event_age: String::new(),
                    origin: None,
                    branched_from: None,
                    completed: Default::default(),
                    integration: None,
                },
            ))
        }
        fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }
    let client = Client::in_process(std::sync::Arc::new(RenameServer));
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "session-1",
    );
    for (draft, expected) in [("New name", Some("New name")), ("   ", None)] {
        app.rename_prompt = Some(draft.into());
        handle_rename_prompt_key(&mut app, &client, KeyEvent::from(KeyCode::Enter));
        assert!(app.rename_prompt.is_none());
        assert_eq!(app.interactions.name("session-1"), expected);
    }
}

#[test]
fn the_branch_picker_retargets_or_keeps_a_base() {
    let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
    let client = Client::new(PathBuf::from("/missing.sock"));
    app.branch_picker = Some(BranchPicker::retarget(
        &["styra/first".into()],
        vec!["main".into(), "release".into()],
        None,
    ));
    for c in "release".chars() {
        handle_branch_picker_key(&mut app, &client, KeyEvent::from(KeyCode::Char(c)));
    }
    handle_branch_picker_key(&mut app, &client, KeyEvent::from(KeyCode::Enter));
    assert!(app.branch_picker.is_none());
    assert!(matches!(app.take_request(), Some(Request::Retarget(target)) if target == "release"));

    app.branch_picker = Some(BranchPicker::base(None, vec!["main".into()], None));
    handle_branch_picker_key(&mut app, &client, KeyEvent::from(KeyCode::Down));
    handle_branch_picker_key(&mut app, &client, KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.base_branch.as_deref(), Some("main"));
    assert!(app.take_request().is_none());

    app.branch_picker = Some(BranchPicker::base(None, vec!["main".into()], None));
    handle_branch_picker_key(&mut app, &client, KeyEvent::from(KeyCode::Esc));
    assert!(app.branch_picker.is_none());
    assert_eq!(app.base_branch.as_deref(), Some("main"));
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

fn tree(name: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::Builder::new()
        .prefix(&format!("styra-keys-{name}-"))
        .tempdir()
        .unwrap();
    let base = directory.path();
    std::fs::create_dir_all(base.join("reports")).unwrap();
    std::fs::write(base.join("reports/summary.md"), "x").unwrap();
    std::fs::write(base.join("notes.txt"), "x").unwrap();
    let path = std::fs::canonicalize(base).unwrap();
    (directory, path)
}

fn typed(app: &mut App, text: &str) {
    open_insert(app);
    for ch in text.chars() {
        handle_insert_key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    handle_insert_key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

/// Each picker command opens its picker without changing the selection;
/// running it again, typing and moving only preview a choice, and only
/// selecting one applies it.
#[test]
fn selection_commands_open_fuzzy_pickers_and_apply_only_on_select() {
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
    for (command, choice) in [
        (EDITOR_CYCLE_PROVIDER, Choice::Provider),
        (EDITOR_CYCLE_MODEL, Choice::ModelEffort),
        (EDITOR_CYCLE_EFFORT, Choice::ModelEffort),
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
        let key = press(&command);
        handle_input_key(&mut app, &client, "workspace", &mut live, key);
        assert_eq!(app.launcher.as_ref().unwrap().choice, choice);
        assert_eq!(app.selection, original);
        assert!(app.take_request().is_none());

        // Model and effort take their first step as they open; the
        // provider command only opens, and then the same command
        // advances, even where its key is also one the picker otherwise
        // moves up with.
        if command == EDITOR_CYCLE_PROVIDER {
            assert_eq!(app.launcher.as_ref().unwrap().selection(), original);
            launcher::handle_key(&mut app, key, &preferences);
        }
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
        if command == EDITOR_CYCLE_MODEL {
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
        launcher::handle_key(&mut app, press(&LAUNCHER_SELECT), &preferences);
        assert_eq!(app.selection, chosen);
        assert_eq!(app.composer.message(), "draft message");
        assert_eq!(app.focus, crate::app::Focus::Input);
        assert!(app.launcher.is_none());
        assert!(app.take_request().is_none());
    }
}

#[test]
fn cancelling_closes_filtered_selection_pickers_and_empty_results_do_not_apply() {
    use crate::launcher;
    use styra_protocol::agent::{Provider, Selection};

    let root = tempfile::tempdir().unwrap();
    let preferences = root.path().join("defaults.json");
    let client = Client::new(PathBuf::from("/missing.sock"));
    let mut live = Attachment::Detached;
    for command in [
        EDITOR_CYCLE_PROVIDER,
        EDITOR_CYCLE_MODEL,
        EDITOR_CYCLE_EFFORT,
    ] {
        let original = Selection::new(Provider::Codex);
        let mut app = App::pending(original.clone());
        app.enter_input();
        handle_input_key(&mut app, &client, "workspace", &mut live, press(&command));
        for ch in "no-such-choice".chars() {
            launcher::handle_key(&mut app, KeyCode::Char(ch).into(), &preferences);
        }
        launcher::handle_key(&mut app, press(&LAUNCHER_SELECT), &preferences);
        assert!(app.launcher.is_some(), "empty results cannot be selected");
        launcher::handle_key(&mut app, press(&LAUNCHER_DEFAULT), &preferences);
        assert!(
            app.launcher.is_some(),
            "only selecting chooses here; a default is the launch picker's"
        );
        launcher::handle_key(&mut app, press(&LAUNCHER_CANCEL), &preferences);
        assert!(
            app.launcher.is_none(),
            "one cancel closes a filtered picker"
        );
        assert_eq!(app.selection, original);
        assert!(app.take_request().is_none());
        assert!(app.recent_models.is_empty());
        assert!(!preferences.exists());
    }
}

/// Adding a box opens another; with two, choosing between them comes
/// before leaving, and only leaving from there leaves — with both boxes
/// kept.
#[test]
fn leaving_chooses_between_boxes_first() {
    let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
    app.enter_input();
    let client = Client::new(PathBuf::from("/missing.sock"));
    let mut live = Attachment::Detached;
    let mut send =
        |app: &mut App, key: KeyEvent| handle_input_key(app, &client, "workspace", &mut live, key);
    send(&mut app, KeyEvent::from(KeyCode::Char('a')));
    send(&mut app, press(&EDITOR_ADD_BOX));
    send(&mut app, KeyEvent::from(KeyCode::Char('b')));
    assert_eq!(app.composer.parts(), ["a", "b"]);

    send(&mut app, press(&EDITOR_CHOOSE_BOX));
    assert_eq!(app.focus, crate::app::Focus::Input);
    assert!(app.composer.choosing());
    // While choosing, the box commands move between boxes instead of
    // typing.
    send(&mut app, press(&EDITOR_BOX_PREV));
    assert_eq!(app.composer.focused(), 0);
    assert_eq!(app.composer.parts(), ["a", "b"]);

    send(&mut app, press(&EDITOR_BOX_LEAVE));
    assert_eq!(app.focus, crate::app::Focus::List);
    assert!(!app.composer.choosing());
    assert_eq!(app.composer.parts(), ["a", "b"]);
}

#[test]
fn the_git_checkout_command_in_details_opens_its_prompt_prefilled() {
    let (_root_directory, root) = tree("git-checkout-prompt");
    let mut app = app(&root);
    app.enter_list();
    app.workspace.git_repository = Some(root.join("repository"));
    app.toggle_view(View::Driva);

    run_list_action(&mut app, &root, &DRIVA_GIT_CHECKOUT);

    assert_eq!(
        app.git_repository_prompt.as_deref(),
        Some(root.join("repository").to_string_lossy().as_ref())
    );
}

/// A view's own command wins over the global command on the same key, as
/// the reference and the action menu say: where details claims
/// integration's key for one of its own commands, integrating does not
/// happen from there, and where it does not, integrating works as
/// anywhere else.
#[test]
fn a_views_own_key_wins_over_the_global_one() {
    let (_root_directory, root) = tree("own-key-wins");
    let mut app = app(&root);
    app.enter_list();
    app.session_id = "session-1".into();
    app.toggle_view(View::Driva);
    let claimed = crate::presentation::view_window(View::Driva).claims(press(&GLOBAL_INTEGRATE));

    run_list_action(&mut app, &root, &GLOBAL_INTEGRATE);

    assert_eq!(
        app.integration_prompt.is_some(),
        !claimed,
        "details claims integration's key: {claimed}"
    );
}

/// Editing tags is global, so it works on the interaction's tags from the
/// reading views too, not only from the event list.
#[test]
fn tags_are_edited_from_a_reading_view() {
    let (_root_directory, root) = tree("tags-from-raw");
    let mut app = app(&root);
    app.enter_list();
    app.session_id = "session-1".into();
    app.toggle_view(View::Raw);

    run_list_action(&mut app, &root, &GLOBAL_TAGS);

    // With no server to answer, reaching the tag editor shows as its
    // failure to list the interactions.
    assert!(app
        .notices
        .iter()
        .any(|notice| notice.text.contains("could not list interactions")));
}

#[test]
fn control_w_removes_mount_path_parts() {
    let (_root_directory, root) = tree("mount-path-prompt");
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
}

/// The list under the mount prompt narrows with every letter typed; the
/// arrows move through it, Tab takes the highlighted entry and goes into
/// it when it is a directory, Shift-Tab climbs back out, and Esc closes
/// the prompt with its list.
#[test]
fn the_mount_prompt_narrows_a_live_list_of_paths() {
    let (_root_directory, root) = tree("mount-path-list");
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
}

/// Enter on a name that is still being narrowed mounts the row the list
/// has highlighted for it; a path that exists is mounted as typed, even
/// while the list shows its siblings.
#[test]
fn enter_mounts_the_highlighted_path_for_a_partial_name() {
    let (_root_directory, root) = tree("mount-path-enter");
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
}

/// Starting a new session, moving among actively working interactions
/// and following newly idle work first are distinct commands, as are the
/// other ways of moving between interactions and their checkouts.
#[test]
fn interaction_commands_distinguish_new_live_and_working() {
    let (_root_directory, root) = tree("live-step");
    let mut app = app(&root);
    app.enter_list();
    app.session_id = "session-1".into();

    for (command, request) in [
        (GLOBAL_NEW_SESSION, Request::NewSession),
        (GLOBAL_NEXT_LIVE, Request::NextLiveInteraction),
        (GLOBAL_NEXT_WORKING, Request::NextWorkingInteraction),
        (GLOBAL_JUMP_BACK, Request::JumpBack),
        (GLOBAL_JUMP_FORWARD, Request::JumpForward),
        (GLOBAL_INTERACTION_HISTORY, Request::InteractionHistory),
        (
            GLOBAL_SESSION_WORKTREE,
            Request::CreateWorktree { message: None },
        ),
        (GLOBAL_DIRECTORY, Request::OpenDirectory),
    ] {
        run_list_action(&mut app, &root, &command);
        assert_eq!(app.take_request(), Some(request), "{:?}", command.action());
    }
}

#[test]
fn the_launcher_opens_from_the_main_view() {
    let (_root_directory, root) = tree("launcher-shortcut");
    let mut app = app(&root);
    app.enter_list();

    run_list_action(&mut app, &root, &GLOBAL_LAUNCHER);

    assert!(app.launcher.is_some());
}

/// Moving right stays in the grid, whatever else shares its key; the next
/// and previous tile commands cycle the tiles, opening asks for the
/// tile's interaction, messaging for its message box, and the overview
/// toggles from both sides.
#[test]
fn the_overview_walks_its_grid_and_opens_the_chosen_tile() {
    use styra_protocol::InteractionActivity;
    let (_root_directory, root) = tree("overview-keys");
    let mut app = app(&root);
    app.enter_list();
    app.interactions.refresh(vec![
        crate::interactions::tests::interaction("2-left", InteractionActivity::Running),
        crate::interactions::tests::interaction("1-right", InteractionActivity::Pending),
    ]);
    let run = |app: &mut App, command: &Binding| run_list_action(app, &root, command);

    run(&mut app, &GLOBAL_OVERVIEW);
    assert_eq!(app.take_request(), Some(Request::Overview));

    app.view = View::Overview;
    run(&mut app, &GLOBAL_NEXT_LIVE);
    assert_eq!(app.take_request(), Some(Request::NextLiveInteraction));
    assert_eq!(app.view, View::Overview);
    run(&mut app, &OVERVIEW_RIGHT);
    assert!(app.launcher.is_none(), "moving right stays in the grid");
    run(&mut app, &OVERVIEW_OPEN);
    assert_eq!(
        app.take_request(),
        Some(Request::ShowInteraction("1-right".into()))
    );

    run(&mut app, &OVERVIEW_PREV);
    run(&mut app, &OVERVIEW_PREV);
    run(&mut app, &OVERVIEW_MESSAGE);
    assert_eq!(
        app.take_request(),
        Some(Request::MessageInteraction("1-right".into())),
        "the previous tile wraps back to the last, and messaging messages it"
    );
    run(&mut app, &OVERVIEW_NEXT);
    run(&mut app, &OVERVIEW_MESSAGE);
    assert_eq!(
        app.take_request(),
        Some(Request::MessageInteraction("2-left".into())),
        "the next tile wraps on to the first"
    );

    run(&mut app, &OVERVIEW_RUNNING_ONLY);
    assert!(app.overview.running_only);
    run(&mut app, &OVERVIEW_NEXT);
    run(&mut app, &OVERVIEW_OPEN);
    assert_eq!(
        app.take_request(),
        Some(Request::ShowInteraction("2-left".into()))
    );
    run(&mut app, &OVERVIEW_RUNNING_ONLY);
    assert!(!app.overview.running_only);

    // Searching opens the prompt over the grid; the event loop then hands
    // the typed keys to it. Esc clears a standing term before closing the
    // overview leaves the grid.
    run(&mut app, &OVERVIEW_SEARCH);
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
    press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Esc));
    assert_eq!(app.search.query(), None);
    assert_eq!(app.view, View::Overview);

    run(&mut app, &OVERVIEW_CLOSE);
    assert_eq!(app.view, View::Events);
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
    // Picking a tile by number takes the number from the digit pressed.
    let nth = |digit| KeyEvent::from(KeyCode::Char(digit));
    for (key, target) in [
        (press(&OVERVIEW_RIGHT), Some("two")),
        (press(&OVERVIEW_DOWN), Some("four")),
        (press(&OVERVIEW_LEFT), Some("three")),
        (press(&OVERVIEW_UP), None),
        (press(&OVERVIEW_LAST), Some("four")),
        (press(&OVERVIEW_NEXT), None),
        (press(&OVERVIEW_PREV), Some("four")),
        (press(&OVERVIEW_FIRST), None),
        (nth('3'), Some("three")),
        (nth('9'), Some("three")),
        (nth('2'), Some("two")),
        (nth('1'), None),
    ] {
        assert!(handle_overview_key(&mut app, key));
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
fn overview_scope_toggles_the_shared_interaction_scope() {
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
    let key = press(&OVERVIEW_SCOPE);
    assert!(handle_overview_key(&mut app, key));
    assert!(app.interactions.only_current_workspace);
    assert_eq!(app.interactions.visible_indices("", Some("here")).len(), 1);
    assert_eq!(
        app.overview
            .indices(&app.interactions, "current", Some("here"))
            .len(),
        1
    );
    assert!(app
        .interactions
        .next_active("current", Some("here"))
        .is_none());
    // Changing scope in the live list is immediately reflected in the grid.
    app.interactions.toggle_workspace_scope();
    assert_eq!(
        app.overview
            .indices(&app.interactions, "current", Some("here"))
            .len(),
        2
    );
    assert!(handle_overview_key(&mut app, key));
    assert!(handle_overview_key(&mut app, key));
    assert!(!app.interactions.only_current_workspace);
}

#[test]
fn global_view_commands_open_and_close_their_views() {
    for (name, command, view, request) in [
        ("messages", GLOBAL_MESSAGES, View::Messages, None),
        (
            "processes",
            GLOBAL_PROCESSES,
            View::Processes,
            Some(Request::Processes),
        ),
        ("log", GLOBAL_LOG, View::Log, None),
    ] {
        let (_root_directory, root) = tree(name);
        let mut app = app(&root);
        app.enter_list();
        for expected in [view, View::Events] {
            run_list_action(&mut app, &root, &command);
            assert_eq!(app.view, expected, "{name}");
            assert_eq!(app.take_request(), request, "{name}: {expected:?}");
        }
    }
}

#[test]
fn the_processes_pane_opens_in_the_entry_logs_place() {
    let (_root_directory, root) = tree("processes-pane-shortcut");
    let mut app = app(&root);
    app.enter_list();
    app.view = View::Quota;
    app.toggle_entry_log();
    assert!(app.entry_log.open);
    let toggle = |app: &mut App| run_list_action(app, &root, &GLOBAL_PROCESSES_PANE);
    app.view = View::Quota;
    toggle(&mut app);
    assert_eq!(app.view, View::Events);
    assert!(app.processes_pane && app.processes_visible());
    assert!(!app.entry_log.open);
    assert!(matches!(app.take_request(), Some(Request::Processes)));

    toggle(&mut app);
    assert!(!app.processes_pane && !app.processes_visible());

    toggle(&mut app);
    app.toggle_entry_log();
    assert!(app.entry_log.open && !app.processes_pane);
}

#[test]
fn going_back_returns_to_the_event_list_from_every_other_view() {
    let (_root_directory, root) = tree("esc-back");
    let mut app = app(&root);
    app.enter_list();

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
        run_list_action(&mut app, &root, &GLOBAL_BACK);
        assert_eq!(app.view, View::Events, "back from {view:?}");
    }
}

#[test]
fn link_highlight_temporarily_opens_the_file_preview() {
    let (_root_directory, root) = tree("links-preview-restore");
    // Esc and Enter end the highlight on the event list itself; opening
    // another view ends it too.
    let enter = KeyEvent::from(KeyCode::Enter);
    for open in [false, true] {
        for exit in [KeyEvent::from(KeyCode::Esc), enter, press(&GLOBAL_MESSAGES)] {
            let mut app = app(&root);
            app.preview.open = open;
            app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
                text: "[notes](notes.txt) and [summary](reports/summary.md)".into(),
            });
            // Re-entering link navigation must preserve the original choice.
            for _ in 0..2 {
                run_list_action(&mut app, &root, &EVENTS_LINKS);
                assert!(app.link_highlight.is_some());
                assert!(app.preview.open);
            }
            assert_eq!(
                app.highlighted_link_target().unwrap().1,
                Ok(root.join("notes.txt"))
            );
            run_list_action(&mut app, &root, &EVENTS_NEXT_LINE);
            assert_eq!(
                app.highlighted_link_target().unwrap().1,
                Ok(root.join("reports/summary.md"))
            );
            assert!(app.preview.open);
            run_list_action(&mut app, &root, &EVENTS_PREV_LINE);
            // An initially open preview is restored even if hidden here;
            // an initially closed one closes on exit without a manual toggle.
            if open {
                run_list_action(&mut app, &root, &EVENTS_PREVIEW_PANEL);
            }
            assert_eq!(app.preview.open, !open);
            press_list_key(&mut app, &root, exit);
            assert!(app.link_highlight.is_none());
            assert_eq!(app.preview.open, open);
            if exit == enter {
                assert_eq!(
                    app.take_request(),
                    Some(Request::OpenPath(root.join("notes.txt")))
                );
            }
        }
    }
}

/// With a preview open, scrolling it moves ten lines, in the event list's
/// pane as in the full-screen preview, and paging moves it by half of what
/// it shows. The full-screen preview still steps between entries; the
/// event list scrolls the interaction independently instead.
#[test]
fn an_open_preview_scrolls_by_lines_and_pages_by_half_of_it() {
    let (_root_directory, root) = tree("preview-arrows");
    let mut app = app(&root);
    app.enter_list();
    for index in 0..3 {
        app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
            text: format!("message {index}\n\nwith more below"),
        });
    }
    app.select_first();
    let run = |app: &mut App, command: &Binding| run_list_action(app, &root, command);

    for view in [View::Events, View::Preview] {
        let (down, up, page_down, page_up) = if view == View::Events {
            (
                EVENTS_PREVIEW_SCROLL_DOWN,
                EVENTS_PREVIEW_SCROLL_UP,
                EVENTS_PAGE_DOWN,
                EVENTS_PAGE_UP,
            )
        } else {
            (
                PREVIEW_SCROLL_DOWN,
                PREVIEW_SCROLL_UP,
                PREVIEW_PAGE_DOWN,
                PREVIEW_PAGE_UP,
            )
        };
        app.view = view;
        app.preview.show();
        app.preview.scroll.reset();
        app.preview.scroll.note_limit(100);
        app.preview.scroll.note_viewport(30);
        let selected = app.timeline.selected;

        run(&mut app, &down);
        assert_eq!(app.preview.scroll.offset, 10, "{view:?}: scrolls");
        assert_eq!(app.timeline.selected, selected, "{view:?}: and stays put");
        run(&mut app, &page_down);
        assert_eq!(app.preview.scroll.offset, 25, "{view:?}: half of 30");
        run(&mut app, &page_up);
        run(&mut app, &up);
        assert_eq!(app.preview.scroll.offset, 0, "{view:?}: back to the top");

        if view == View::Preview {
            run(&mut app, &PREVIEW_NEXT_ENTRY);
            assert_ne!(app.timeline.selected, selected, "{view:?}: moves entry");
        } else {
            run(&mut app, &EVENTS_SCROLL_DOWN);
            assert_eq!(app.timeline.selected, selected, "{view:?}: does not move");
            assert_eq!(
                app.timeline.list_scroll_delta, 10,
                "{view:?}: scrolls the interaction"
            );
            run(&mut app, &EVENTS_SCROLL_UP);
            assert_eq!(app.timeline.list_scroll_delta, 0, "{view:?}: scrolls back");
        }
        app.select_first();
    }
}

/// Only these sends are handed to the event loop to branch; every other
/// send goes out from the message box as it is.
#[test]
fn only_a_typed_message_sent_in_a_branch_branches() {
    let (_root_directory, root) = tree("creates-worktree");
    let mut app = app(&root);
    let in_branch = press(&EDITOR_SEND_IN_BRANCH);

    app.composer.set("start here".into());
    assert!(creates_worktree(&app, in_branch));
    assert!(
        !creates_worktree(&app, press(&EDITOR_SEND)),
        "a plain send goes into the current workspace"
    );

    app.composer.set("   ".into());
    assert!(
        !creates_worktree(&app, in_branch),
        "a blank box sends nothing, so it branches nothing"
    );

    app.composer.set("continue".into());
    app.session_id = "session-1".into();
    assert!(
        creates_worktree(&app, in_branch),
        "a Session already under way can still be branched"
    );
}

#[test]
fn sending_from_a_chosen_base_creates_its_worktree_like_ctrl_enter() {
    let (_root_directory, root) = tree("chosen-base-send");
    let mut app = app(&root);
    app.base_branch = Some("release".into());
    app.enter_input();
    app.composer.set("start on release".into());
    let client = Client::new(root.join("missing.sock"));
    let mut live = Attachment::Detached;

    handle_input_key(
        &mut app,
        &client,
        "workspace-1",
        &mut live,
        press(&EDITOR_SEND),
    );

    assert_eq!(
        app.take_request(),
        Some(Request::CreateWorktree {
            message: Some("start on release".into()),
        }),
        "plain Enter goes through the same worktree creation, and its notice"
    );
    assert_eq!(live, Attachment::Detached, "nothing launches from the box");
    assert_eq!(app.base_branch.as_deref(), Some("release"));
}

#[test]
fn sending_in_a_branch_carries_the_prompt_into_the_worktree() {
    // Mid-turn included: whether the interaction can move is the
    // server's to say, and its refusal puts the message back.
    for (name, session_id, status, attachment, message) in [
        (
            "first-prompt",
            "",
            Status::Pending,
            Attachment::Detached,
            "start here",
        ),
        (
            "later-message",
            "session-1",
            Status::Idle(crate::activity::IdleReason::TurnComplete),
            Attachment::Attached { cursor: 0 },
            "carry on in a branch",
        ),
        (
            "mid-turn",
            "session-1",
            Status::Running,
            Attachment::Attached { cursor: 0 },
            "carry on in a branch",
        ),
    ] {
        let (_root_directory, root) = tree(name);
        let mut app = app(&root);
        app.session_id = session_id.into();
        app.activity.status = status;
        app.enter_input();
        app.composer.set(message.into());
        let client = Client::new(root.join("missing.sock"));
        let mut live = match &attachment {
            Attachment::Detached => Attachment::Detached,
            Attachment::Attached { cursor } => Attachment::Attached { cursor: *cursor },
        };
        handle_input_key(
            &mut app,
            &client,
            "workspace-1",
            &mut live,
            press(&EDITOR_SEND_IN_BRANCH),
        );
        assert_eq!(
            app.take_request(),
            Some(Request::CreateWorktree {
                message: Some(message.into()),
            }),
            "{name}"
        );
        assert_eq!(app.composer.text(), "", "{name}");
        assert_eq!(live, attachment, "{name}: nothing launches from the box");
    }
}

#[test]
fn enter_on_a_branch_marker_opens_the_linked_interaction() {
    use styra_protocol::event::BranchDirection;

    for (fixture, direction, session) in [
        ("follow-branch-enter", BranchDirection::To, "branch-2"),
        (
            "follow-branch-enter-source",
            BranchDirection::From,
            "source-1",
        ),
    ] {
        let (_root_directory, root) = tree(fixture);
        let mut app = app(&root);
        app.enter_list();
        app.push_event(styra_protocol::event::AgentEvent::Branched {
            direction,
            session: session.into(),
            name: None,
        });
        app.select_last();

        press_list_key(&mut app, &root, KeyCode::Enter.into());

        assert_eq!(
            app.take_request(),
            Some(Request::OpenSession(session.into()))
        );
    }
}

/// Moving by line walks the branches drawn beneath a `branched to` marker
/// as rows of their own, and Enter opens the one the cursor is on.
#[test]
fn moving_by_line_selects_a_markers_child_branches_and_enter_opens_one() {
    use crate::interactions::tests::branched;
    use styra_protocol::event::{AgentEvent, BranchDirection};
    use styra_protocol::InteractionActivity::Running;

    let (_root_directory, root) = tree("follow-child-branch");
    let mut app = app(&root);
    app.enter_list();
    let source = app.session_id.clone();
    app.interactions.refresh(vec![
        branched("branch-2", Running, &source),
        branched("child-3", Running, "branch-2"),
        branched("grandchild-4", Running, "child-3"),
    ]);
    app.push_event(AgentEvent::AgentMessage {
        text: "before".into(),
    });
    app.push_event(AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "branch-2".into(),
        name: None,
    });
    app.push_event(AgentEvent::AgentMessage {
        text: "after".into(),
    });
    app.select_last();
    let marker = app.timeline.selected - 1;

    // Up onto the marker lands on its deepest branch, then on up.
    run_list_action(&mut app, &root, &EVENTS_PREV_LINE);
    assert_eq!(
        (app.timeline.selected, app.selected_branch()),
        (marker, Some(1))
    );
    run_list_action(&mut app, &root, &EVENTS_PREV_LINE);
    assert_eq!(app.selected_branch(), Some(0));
    press_list_key(&mut app, &root, KeyCode::Enter.into());
    assert_eq!(
        app.take_request(),
        Some(Request::OpenSession("child-3".into()))
    );
    run_list_action(&mut app, &root, &EVENTS_PREV_LINE);
    assert_eq!(
        (app.timeline.selected, app.selected_branch()),
        (marker, None)
    );
    press_list_key(&mut app, &root, KeyCode::Enter.into());
    assert_eq!(
        app.take_request(),
        Some(Request::OpenSession("branch-2".into()))
    );

    // And back down through them and past.
    for expected in [Some(0), Some(1)] {
        run_list_action(&mut app, &root, &EVENTS_NEXT_LINE);
        assert_eq!(
            (app.timeline.selected, app.selected_branch()),
            (marker, expected)
        );
    }
    press_list_key(&mut app, &root, KeyCode::Enter.into());
    assert_eq!(
        app.take_request(),
        Some(Request::OpenSession("grandchild-4".into()))
    );
    run_list_action(&mut app, &root, &EVENTS_NEXT_LINE);
    assert_eq!(
        (app.timeline.selected, app.selected_branch()),
        (marker + 1, None)
    );
}

/// Moving down past a final marker's last branch stays on that branch
/// instead of jumping back up to the marker's own row.
#[test]
fn moving_down_past_the_last_branch_of_a_final_marker_stays_put() {
    use crate::interactions::tests::branched;
    use styra_protocol::event::{AgentEvent, BranchDirection};
    use styra_protocol::InteractionActivity::Running;

    let (_root_directory, root) = tree("last-child-branch");
    let mut app = app(&root);
    app.enter_list();
    let source = app.session_id.clone();
    app.interactions.refresh(vec![
        branched("branch-2", Running, &source),
        branched("child-3", Running, "branch-2"),
    ]);
    app.push_event(AgentEvent::Branched {
        direction: BranchDirection::To,
        session: "branch-2".into(),
        name: None,
    });
    app.select_last();
    let marker = app.timeline.selected;

    run_list_action(&mut app, &root, &EVENTS_NEXT_LINE);
    assert_eq!(app.selected_branch(), Some(0));
    run_list_action(&mut app, &root, &EVENTS_NEXT_LINE);
    assert_eq!(
        (app.timeline.selected, app.selected_branch()),
        (marker, Some(0))
    );
}

/// ctrl-p opens the Session this one was branched off, and only warns
/// when there is none.
#[test]
fn ctrl_p_opens_the_parent_branch() {
    use crate::interactions::tests::branched;
    use styra_protocol::InteractionActivity::Running;

    let (_root_directory, root) = tree("open-parent");
    let mut app = app(&root);
    app.enter_list();
    let source = app.session_id.clone();

    run_list_action(&mut app, &root, &EVENTS_PARENT);
    assert!(app.take_request().is_none());

    app.interactions
        .refresh(vec![branched("branch-2", Running, &source)]);
    app.session_id = "branch-2".into();
    run_list_action(&mut app, &root, &EVENTS_PARENT);
    assert_eq!(app.take_request(), Some(Request::OpenSession(source)));
}

/// On the blank screen alt-b lists the repository's branches to start
/// from; once there is a Session the same key is retarget's.
#[test]
fn alt_b_on_the_blank_screen_lists_base_branches() {
    let (_root_directory, root) = tree("base-branch");
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "start"]);
    git(&["branch", "release"]);
    let mut app = app(&root);
    app.enter_list();
    app.workspace_branch = Some("main".into());

    run_list_action(&mut app, &root, &GLOBAL_BASE_BRANCH);
    let picker = app.branch_picker.take().expect("the picker opens");
    assert_eq!(picker.rows[0], "Default (main (Workspace))");
    let mut branches = picker.rows[1..].to_vec();
    branches.sort();
    assert_eq!(branches, vec!["main", "release"]);

    app.session_id = "styra-7".into();
    run_list_action(&mut app, &root, &GLOBAL_BASE_BRANCH);
    assert!(app.branch_picker.is_none());
}

/// Unlike a branch marker, a regular event toggles its expansion.
#[test]
fn a_regular_event_still_toggles_its_expansion() {
    let (_root_directory, root) = tree("regular-enter");
    let mut app = app(&root);
    app.enter_list();
    app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
        text: "ordinary reply".into(),
    });
    app.select_last();

    run_list_action(&mut app, &root, &EVENTS_TOGGLE_EXPAND);

    assert!(app.timeline.selected_entry().unwrap().expanded);
    assert!(app.take_request().is_none());
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

/// Run `binding`'s command from the list, with whatever keys the
/// configuration gives it — a chord's prefix included, the list carrying
/// the pending prefix from one key to the next as the event loop does.
fn run_list_action(app: &mut App, root: &Path, binding: &Binding) {
    let client = Client::new(root.join("missing.sock"));
    let mut live = Attachment::Detached;
    let mut pending_fold = false;
    for key in presses(binding) {
        handle_list_key(
            app,
            &client,
            &mut live,
            key,
            &mut pending_fold,
            &root.join("preferences.toml"),
        );
    }
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
fn the_diff_carries_the_selected_file_or_reference_location() {
    let (_root_directory, root) = tree("diff-selected-location");
    let mut app = app_with_diffable_interaction(&root);
    app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
        text: "see `notes.txt`".into(),
    });
    app.view = View::Files;
    run_list_action(&mut app, &root, &GLOBAL_DIFF);
    assert_eq!(
        app.take_request(),
        Some(Request::ShowDiffAt {
            id: "styra-7".into(),
            path: root.join("notes.txt"),
            line: None,
        })
    );

    app.view = View::Events;
    app.push_event(styra_protocol::event::AgentEvent::AgentMessage {
        text: "see [here](notes.txt:3)".into(),
    });
    app.select_last();
    app.highlight_first_link();
    run_list_action(&mut app, &root, &GLOBAL_DIFF);
    assert_eq!(
        app.take_request(),
        Some(Request::ShowDiffAt {
            id: "styra-7".into(),
            path: root.join("notes.txt"),
            line: Some(3),
        })
    );
}

/// The diff command closes an open diff pane from the log, as the diff's
/// own `d` does from the pane.
#[test]
fn the_diff_closes_from_the_log_and_from_the_pane() {
    let (_root_directory, root) = tree("diff-toggle");
    let mut app = app_with_diffable_interaction(&root);
    for log_focus in [true, false] {
        app.checkout_diff = Some(styra_diff::CheckoutDiff::read(diffable_target()));
        app.diff_preview = true;
        app.diff_interaction_focus = log_focus;
        assert_eq!(app.diff_focused(), !log_focus);
        if log_focus {
            run_list_action(&mut app, &root, &GLOBAL_DIFF);
        } else {
            // The pane's keys are styra-diff's own.
            press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Char('d')));
        }
        assert!(!app.diff_visible(), "log focus: {log_focus}");
        assert_eq!(app.take_request(), None);
    }
    run_list_action(&mut app, &root, &GLOBAL_DIFF);
    assert_eq!(
        app.take_request(),
        Some(Request::ShowDiff("styra-7".into()))
    );
}

/// Opening the diff asks the event loop to resolve this Session's diff
/// base.
#[test]
fn the_diff_asks_for_the_checkout_diff() {
    let (_root_directory, root) = tree("diff-checkout");
    let mut app = app_with_diffable_interaction(&root);
    run_list_action(&mut app, &root, &GLOBAL_DIFF);
    assert_eq!(
        app.take_request(),
        Some(Request::ShowDiff("styra-7".into()))
    );
}

#[test]
fn preview_diff_can_be_scrolled_and_closed() {
    let (_root_directory, root) = tree("diff-preview");
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
}

/// Switching to the turn swaps the diff between the checkout's and the
/// selected turn's, the diff's keys then acting on the turn's; closing the
/// diff leaves the next one opened on the checkout's diff again.
#[test]
fn the_turn_command_switches_the_diff_to_the_turn_diff_and_back() {
    let (_root_directory, root) = tree("diff-turn");
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
    // The diff's own keys are styra-diff's; switching turns is Styra's.
    let press = |app: &mut App, code| press_list_key(app, &root, KeyEvent::from(code));
    let switch_turn = |app: &mut App| run_list_action(app, &root, &DIFF_TURN);

    switch_turn(&mut app);
    assert!(app.turn_diff_shown());
    let turn = app.shown_diff().unwrap();
    assert!(turn.target().is_none());
    assert!(turn.diff.as_deref().unwrap().contains("+b"));
    press(&mut app, KeyCode::Char('l'));
    assert!(app.shown_diff().unwrap().per_file);
    assert!(!app.checkout_diff.as_ref().unwrap().per_file);
    press(&mut app, KeyCode::Char('E'));
    assert!(app.take_request().is_none(), "no report for a turn diff");

    switch_turn(&mut app);
    assert!(!app.turn_diff_shown());
    assert!(app.shown_diff().unwrap().target().is_some());
    switch_turn(&mut app);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    assert!(!app.diff_visible());
    assert!(!app.diff_turn);
}

#[test]
fn turn_diff_commands_move_between_operator_turns() {
    use styra_protocol::event::AgentEvent;

    let (_root_directory, root) = tree("diff-turn-navigation");
    let mut app = app_with_diffable_interaction(&root);
    for turn in 1..=3 {
        app.push_event(AgentEvent::UserMessage {
            text: format!("turn {turn}"),
        });
        app.push_event(AgentEvent::FileChanged {
            id: String::new(),
            paths: vec!["a.txt".into()],
            diff: Some(format!("@@ -1 +1 @@\n-old\n+turn {turn}")),
            checkpoint: None,
            checkpoint_error: None,
        });
    }
    app.select_first();
    app.checkout_diff = Some(styra_diff::CheckoutDiff::read(diffable_target()));
    app.diff_preview = true;
    // The diff's own keys are styra-diff's; moving between turns is Styra's.
    let press = |app: &mut App, code| press_list_key(app, &root, KeyEvent::from(code));
    let run = |app: &mut App, command: &Binding| run_list_action(app, &root, command);

    run(&mut app, &DIFF_NEXT_TURN);
    assert_eq!(app.selected_turn(), Some(1), "full diff keeps its keys");
    run(&mut app, &DIFF_TURN);
    for (command, expected) in [
        (DIFF_PREV_TURN, 1),
        (DIFF_NEXT_TURN, 2),
        (DIFF_NEXT_TURN, 3),
        (DIFF_NEXT_TURN, 3),
        (DIFF_PREV_TURN, 2),
    ] {
        run(&mut app, &command);
        assert_eq!(app.selected_turn(), Some(expected));
        assert!(app
            .shown_diff()
            .unwrap()
            .diff
            .as_deref()
            .unwrap()
            .contains(&format!("+turn {expected}")));
    }
    assert!(
        !app.timeline.follow,
        "new events must not move the chosen turn"
    );

    // From work inside a turn, previous skips to the previous operator message.
    app.timeline.selected += 1;
    assert_eq!(app.selected_turn(), Some(2));
    run(&mut app, &DIFF_PREV_TURN);
    assert_eq!(app.selected_turn(), Some(1));

    // The file filter keeps the turn commands' keys while typing.
    press(&mut app, KeyCode::Char('l'));
    press(&mut app, KeyCode::Char('/'));
    assert!(app.shown_diff().unwrap().typing());
    run(&mut app, &DIFF_NEXT_TURN);
    assert_eq!(app.selected_turn(), Some(1));
    press(&mut app, KeyCode::Esc);

    run(&mut app, &DIFF_NEXT_TURN);
    assert_eq!(app.selected_turn(), Some(2));
    run(&mut app, &DIFF_PREV_TURN);
    assert_eq!(app.selected_turn(), Some(1));
}

#[test]
fn diff_prompt_enter_reveals_latest_matching_message() {
    let (_root_directory, root) = tree("diff-prompt-jump");
    let mut app = app_with_diffable_interaction(&root);
    let mut diff = styra_diff::CheckoutDiff::read(diffable_target());
    diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n".into());
    diff.per_file = true;
    diff.line_cursor = Some(3);
    app.checkout_diff = Some(diff);
    app.diff_preview = true;
    handle_diff_key(&mut app, KeyEvent::from(KeyCode::Enter));
    assert!(app.diff_focused(), "unmarked lines stay in the diff");
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
    assert!(app.diff_focused(), "Enter accepts the filter first");
    handle_diff_key(&mut app, KeyEvent::from(KeyCode::Enter));
    assert!(app.diff_interaction_focus, "side pane returns keys to log");
    assert_eq!(app.timeline.selected, expected);
    assert!(!app.timeline.follow);
    assert_ne!(app.timeline.list_anchor, ListAnchor::Free);
    assert!(app.timeline.entries[expected].expanded);

    app.diff_interaction_focus = false;
    app.checkout_diff.as_mut().unwrap().line_cursor = None;
    handle_diff_key(&mut app, KeyEvent::from(KeyCode::Enter));
    assert_eq!(app.timeline.selected, expected - 2, "file prompt");
    assert!(app.diff_interaction_focus, "side pane returns keys to log");
}

/// Starting a message from the diff makes it about what the diff is on —
/// the file it shows, and the line cursor's line — and sending puts that
/// under it.
#[test]
fn a_message_started_from_the_diff_notes_its_file_and_line() {
    let (_root_directory, root) = tree("diff-message-source");
    let mut app = app_with_diffable_interaction(&root);
    app.diff_preview = true;
    let mut diff = styra_diff::CheckoutDiff::read(diffable_target());
    diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
                        diff --git a/b.txt b/b.txt\n@@ -4,2 +4,2 @@\n c\n-d\n+e\n"
        .into());
    diff.scroll.note_limit(100);
    app.checkout_diff = Some(diff);
    // The diff's own keys are styra-diff's; starting a message is Styra's.
    let press = |app: &mut App, character| {
        press_list_key(app, &root, KeyEvent::from(KeyCode::Char(character)))
    };
    let start_message = |app: &mut App| run_list_action(app, &root, &GLOBAL_FOCUS_MESSAGE);

    start_message(&mut app);
    assert_eq!(app.focus, crate::app::Focus::Input);
    assert_eq!(
        app.message_source, None,
        "the combined diff is on no one file"
    );
    app.enter_list();

    press(&mut app, 'l');
    press(&mut app, 'j');
    start_message(&mut app);
    assert_eq!(app.message_source.as_deref(), Some("b.txt"));
    app.enter_list();
    assert_eq!(app.message_source, None, "leaving the box forgets it");

    press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Enter));
    press(&mut app, 'j');
    press(&mut app, 'j');
    assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 1);
    start_message(&mut app);
    assert_eq!(app.message_source.as_deref(), Some("b.txt:5"));
    app.composer.char('x');
    assert_eq!(app.take_message().as_deref(), Some("x\n\nSource: b.txt:5"));
    assert_eq!(app.message_source, None);

    app.enter_input();
    assert_eq!(app.message_source, None, "a message started elsewhere");
}

#[test]
fn per_file_diff_navigation_and_scrolling_use_separate_keys() {
    let (_root_directory, root) = tree("diff-files");
    let mut app = app_with_diffable_interaction(&root);
    app.enter_list();
    app.diff_preview = true;
    app.checkout_diff = Some(styra_diff::CheckoutDiff {
            source: styra_diff::Source::Checkout(diffable_target()),
            diff: Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\ndiff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n".into()),
            ..Default::default()
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
    assert!(
        !app.diff_interaction_focus,
        "Tab belongs to the open commit list"
    );
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
    assert!(app.diff_focused(), "r is the diff's here");
    press(&mut app, KeyCode::Char('r'));
    assert!(!app.checkout_diff.as_ref().unwrap().hide_removed);
    press(&mut app, KeyCode::Char('l'));
    assert!(app.checkout_diff.as_ref().unwrap().per_file);
    // As drawing the per-file view would.
    let diff = app.checkout_diff.as_mut().unwrap();
    diff.scroll.note_limit(100);
    diff.scroll.note_viewport(30);
    press(&mut app, KeyCode::Char('C'));
    assert!(!app.checkout_diff.as_ref().unwrap().compact_dirs);
    assert!(app.diff_focused(), "C is the diff's here");
    press(&mut app, KeyCode::Char('C'));
    assert!(app.checkout_diff.as_ref().unwrap().compact_dirs);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(
        app.checkout_diff.as_ref().unwrap().tint,
        styra_diff::Tint::Half
    );
    assert!(app.diff_focused(), "t is the diff's here");
    // `e` puts the edit report away and back; `E` asks for a new one.
    press(&mut app, KeyCode::Char('e'));
    assert!(app.checkout_diff.as_ref().unwrap().report.hidden);
    assert!(app.diff_focused(), "e is the diff's here");
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
    assert!(app.diff_focused());
    assert_eq!(app.checkout_diff.as_ref().unwrap().search.query(), None);
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(app.checkout_diff.as_ref().unwrap().selected_file, 0);

    press(&mut app, KeyCode::Char('/'));
    for character in "dq?".chars() {
        press(&mut app, KeyCode::Char(character));
    }
    assert!(app.diff_focused());
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
}

#[test]
fn tab_switches_navigation_between_the_side_diff_and_interaction() {
    let (_root_directory, root) = tree("diff-tab-focus");
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
        run_list_action(&mut app, &root, &EVENTS_NEXT_LINE);
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
}

/// `v` asks for the file at the top of the diff, at the line there, and
/// says why when that file is gone rather than asking for it.
#[test]
fn v_in_the_diff_opens_the_file_at_the_top_at_its_line() {
    let (_root_directory, root) = tree("diff-open");
    let mut app = app_with_diffable_interaction(&root);
    app.diff_preview = true;
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
    assert!(app.diff_focused(), "v is the diff's here");

    app.checkout_diff.as_mut().unwrap().scroll.offset = 5;
    press_list_key(&mut app, &root, v);
    assert_eq!(app.take_request(), None);
}

/// Starting a message from the diff keeps the diff in view, the keys
/// typed then being text, and leaving the message returns to the diff.
#[test]
fn a_message_started_from_the_diff_returns_to_it_on_leaving() {
    let (_root_directory, root) = tree("diff-message-focus");
    for view in [View::Events, View::Files] {
        let mut app = app_with_diffable_interaction(&root);
        app.view = view;
        app.diff_preview = true;
        run_list_action(&mut app, &root, &GLOBAL_FOCUS_MESSAGE);
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
            press(&GLOBAL_LEAVE_MESSAGE),
        );
        assert!(app.diff_focused());
        assert_eq!(app.view, view);
    }
}

/// While the diff's filter is being typed, the keys for starting a
/// message and for quitting go to the filter rather than acting.
#[test]
fn starting_a_message_and_quitting_wait_while_typing_the_diff_filter() {
    let (_root_directory, root) = tree("diff-filter-i");
    let mut app = app_with_diffable_interaction(&root);
    app.diff_preview = true;
    let mut diff = styra_diff::CheckoutDiff::read(diffable_target());
    diff.search.open();
    app.checkout_diff = Some(diff);
    run_list_action(&mut app, &root, &GLOBAL_FOCUS_MESSAGE);
    run_list_action(&mut app, &root, &GLOBAL_QUIT);
    press_list_key(&mut app, &root, KeyEvent::from(KeyCode::Tab));
    assert!(app.diff_focused());
    assert_eq!(app.focus, crate::app::Focus::List);
    assert!(app.checkout_diff.as_ref().unwrap().search.typing());
    assert_eq!(app.take_request(), None);
}

#[test]
fn quitting_from_the_diff_quits_styra_in_either_view() {
    let (_root_directory, root) = tree("diff-quit");
    for view in [View::Events, View::Files] {
        let mut app = app_with_diffable_interaction(&root);
        app.view = view;
        app.diff_preview = true;
        app.checkout_diff = Some(styra_diff::CheckoutDiff::read(diffable_target()));
        run_list_action(&mut app, &root, &GLOBAL_QUIT);
        assert_eq!(app.take_request(), Some(Request::Quit));
    }
}

/// Apart from starting a message and quitting, global commands do nothing
/// in the diff, and the diff's own d or Esc returns to events.
#[test]
fn d_and_esc_close_the_diff_view() {
    let (_root_directory, root) = tree("diff-exclusive");
    let mut app = app_with_diffable_interaction(&root);
    app.diff_preview = true;

    for command in [
        GLOBAL_LAUNCHER,
        GLOBAL_INTERACTIONS,
        GLOBAL_WORKTREES,
        GLOBAL_NEXT_LIVE,
        GLOBAL_SESSIONS,
        GLOBAL_QUOTA,
        GLOBAL_PREVIEW,
        GLOBAL_ANSWER,
        GLOBAL_ENTRY_LOG,
    ] {
        let action = command.action();
        run_list_action(&mut app, &root, &command);
        assert!(app.diff_focused(), "{action:?} left the diff");
        assert_eq!(app.take_request(), None, "{action:?} asked for something");
        assert_eq!(
            app.focus,
            crate::app::Focus::List,
            "{action:?} moved the focus"
        );
    }
    for loaded in [false, true] {
        for code in [KeyCode::Char('d'), KeyCode::Esc] {
            for view in [View::Events, View::Files] {
                app.view = view;
                app.diff_preview = true;
                app.checkout_diff =
                    loaded.then(|| styra_diff::CheckoutDiff::read(diffable_target()));
                press_list_key(&mut app, &root, KeyEvent::from(code));
                assert_eq!(app.view, view);
                assert!(!app.diff_visible());
                assert_eq!(app.take_request(), None);
            }
        }
    }
}

/// A Session needs no branch point to open its snapshot diff.
#[test]
fn the_diff_without_a_start_commit_requests_a_snapshot_diff() {
    let (_root_directory, root) = tree("diff-no-branch-point");
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

    run_list_action(&mut app, &root, &GLOBAL_DIFF);

    assert_eq!(
        app.take_request(),
        Some(Request::ShowDiff("styra-7".into()))
    );
}

/// What the prompt decides reaches the message: a path the sandbox already
/// carries goes in under the name the agent knows it by.
#[test]
fn a_decided_path_goes_into_the_message_being_composed() {
    let (_root_directory, root) = tree("mounted");
    let mut app = app(&root);

    typed(&mut app, "reports/summary.md");

    assert!(app.insert.is_none());
    assert_eq!(app.composer.text(), "/workspace/reports/summary.md");
    assert!(app.launch.interaction.mounts.is_empty());
}

/// And a granted one also reaches the launch policy, as this
/// interaction's own mount.
#[test]
fn a_granted_path_is_added_to_this_interactions_mounts() {
    let (_root_directory, root) = tree("granted");
    let (_outside_directory, outside) = tree("granted-elsewhere");
    let mut app = app(&root);
    let host = outside.join("notes.txt");

    typed(&mut app, &host.display().to_string());
    handle_insert_key(&mut app, press(&EDITOR_MOUNT_WRITABLE));

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

fn run_link_menu_action(app: &mut App, binding: &Binding) {
    handle_link_menu_key(app, press(binding));
}

/// Confirming the menu's first action opens the link, as opening it from
/// the event list does.
#[test]
fn the_link_menus_first_action_opens_the_link() {
    let (_root_directory, root) = tree("link-open");
    let mut app = app(&root);
    link_menu_on(&mut app, &root.join("notes.txt").display().to_string());

    run_link_menu_action(&mut app, &LINK_MENU_CONFIRM);

    assert!(app.link_menu.is_none());
    assert_eq!(
        app.take_request(),
        Some(Request::OpenPath(root.join("notes.txt")))
    );
}

#[test]
fn the_link_menu_mounts_a_file_outside_the_sandbox_for_this_interaction() {
    let (_root_directory, root) = tree("link-mount");
    let (_outside_directory, outside) = tree("link-mount-elsewhere");
    let host = outside.join("notes.txt");
    let mut app = app(&root);
    link_menu_on(&mut app, &host.display().to_string());

    run_link_menu_action(&mut app, &LINK_MENU_NEXT);
    run_link_menu_action(&mut app, &LINK_MENU_NEXT);
    run_link_menu_action(&mut app, &LINK_MENU_CONFIRM);

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
}

/// An idle interaction takes the mount by restarting, as it does when the
/// mount is added in the Driva view; a working one refuses it.
#[test]
fn the_link_menu_restarts_an_idle_interaction_to_mount() {
    let (_root_directory, root) = tree("link-mount-idle");
    let (_outside_directory, outside) = tree("link-mount-idle-elsewhere");
    let host = outside.join("notes.txt");

    let mut working = app(&root);
    link_menu_on(&mut working, &host.display().to_string());
    working.activity.status = crate::activity::Status::Running;
    run_link_menu_action(&mut working, &LINK_MENU_NEXT);
    run_link_menu_action(&mut working, &LINK_MENU_CONFIRM);
    assert!(working.launch.interaction.mounts.is_empty());
    assert!(!working.restart_for_mounts);

    let mut idle = app(&root);
    link_menu_on(&mut idle, &host.display().to_string());
    idle.activity.status = crate::activity::Status::Idle(crate::activity::IdleReason::TurnComplete);
    assert!(!idle.can_edit_launch());
    run_link_menu_action(&mut idle, &LINK_MENU_NEXT);
    run_link_menu_action(&mut idle, &LINK_MENU_CONFIRM);
    assert_eq!(idle.launch.interaction.mounts.len(), 1);
    assert!(idle.restart_for_mounts);
    assert!(idle.notices.iter().any(|message| message
        .text
        .contains("restarting the interaction to apply it")));
}

#[test]
fn the_link_menu_does_not_mount_what_the_sandbox_already_carries() {
    let (_root_directory, root) = tree("link-mounted");
    let mut app = app(&root);
    link_menu_on(&mut app, "notes.txt");

    run_link_menu_action(&mut app, &LINK_MENU_NEXT);
    run_link_menu_action(&mut app, &LINK_MENU_CONFIRM);

    assert!(app.launch.interaction.mounts.is_empty());
    assert!(app.notices.iter().any(|message| message
        .text
        .contains("already in the sandbox at /workspace")));
}
