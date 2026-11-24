use super::*;

#[test]
fn message_paste_follows_editor_focus_and_modal_ownership() {
    let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
    app.focus = Focus::List;
    assert!(!message_paste_active(&app));
    app.focus = Focus::Input;
    assert!(message_paste_active(&app));
    app.rename_prompt = Some(String::new());
    assert!(!message_paste_active(&app));
    assert!(!reference_available(&app));
    app.rename_prompt = None;
    app.branch_picker = Some(crate::branch_picker::BranchPicker::retarget(
        &[],
        vec!["main".into()],
        None,
    ));
    assert!(!message_paste_active(&app));
    assert!(!reference_available(&app));
    app.branch_picker = None;
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
    app.diff_preview = true;
    assert!(!action_menu_available(&app), "the diff owns the keyboard");
    app.diff_preview = false;
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
            input::handle_command_prompt_key(&mut app, KeyEvent::from(KeyCode::Char(character)));
        }
        assert_eq!(
            app.launch.command_prompt.as_ref().unwrap().1,
            "cargo run -- --help ?"
        );
        app.focus = Focus::Input;
        assert!(!message_paste_active(&app));
    }
}
