use super::*;
use crate::keybindings::{self as keys, press};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use styra_protocol::agent::Selection;

fn workspace(id: &str, host_path: &str) -> WorkspaceSummary {
    WorkspaceSummary {
        path: format!("/store/{id}").into(),
        created_at_ms: 0,
        last_accessed_at_ms: 0,
        ..crate::workspace::tests::workspace(id, None, host_path)
    }
}

#[test]
fn shell_subcommand_captures_an_explicit_session() {
    let cli = Cli::try_parse_from(["styra", "shell", "--session", "styra-123"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(CliCommand::Shell { session: Some(session) }) if session == "styra-123"
    ));
}

#[test]
fn shell_subcommand_without_a_session_opens_the_browser() {
    let cli = Cli::try_parse_from(["styra", "shell"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(CliCommand::Shell { session: None })
    ));
}

#[test]
fn trailing_prompt_launch_parses_without_a_subcommand() {
    let cli = Cli::try_parse_from(["styra", "hello"]).unwrap();
    assert!(cli.command.is_none());
    assert_eq!(cli.prompt, vec!["hello"]);
}

#[test]
fn skipping_the_workspace_list_does_not_combine_with_an_explicit_target() {
    let cli = Cli::try_parse_from(["styra", "--skip-workspace-list"]).unwrap();
    assert!(cli.skip_workspace_list);
    assert!(
        Cli::try_parse_from(["styra", "--skip-workspace-list", "--workspace", "/tmp"]).is_err()
    );
    assert!(Cli::try_parse_from(["styra", "--skip-workspace-list", "--view"]).is_err());
}

#[test]
fn profile_is_not_a_command_line_option() {
    assert!(Cli::try_parse_from(["styra", "--profile", "codex"]).is_err());
}

#[test]
fn quitting_and_switching_detach_but_reset_stops_the_running_interaction() {
    let live = Attachment::Attached { cursor: 7 };

    assert!(!event_loop::stops_current_interaction(
        &RunOutcome::Quit,
        &live
    ));
    assert!(!event_loop::stops_current_interaction(
        &RunOutcome::OpenSession("styra-other".into()),
        &live,
    ));
    assert!(event_loop::stops_current_interaction(
        &RunOutcome::Reset,
        &live
    ));
    assert!(!event_loop::stops_current_interaction(
        &RunOutcome::NewSession,
        &live
    ));
}

#[test]
fn new_session_uses_the_workspace_of_the_interaction_being_viewed() {
    let active = workspace("first", "/work/first");
    let viewed = workspace("viewed", "/work/viewed");
    let selection = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
    let mut app = App::pending(selection.clone());
    app.workspace.id = Some(viewed.id.clone());
    app.session_id = "source-session".into();

    let inherited = workspace_for_new_session(&app, &active, &[active.clone(), viewed.clone()]);
    let pending = pending_app_from_session(&app, &inherited);

    assert_eq!(inherited, viewed);
    assert_eq!(pending.workspace.id.as_deref(), Some("viewed"));
    assert_eq!(pending.workspace.root(), Some(Path::new("/work/viewed")));
    assert_eq!(pending.selection, selection);
    assert_eq!(pending.checkout_from.as_deref(), Some("source-session"));
}

/// `n` in a Session working in a linked worktree opens a blank screen
/// standing in that worktree, not at the Workspace root: the checkout it
/// asks to share is where the next launch will actually work, and the
/// footer has to say so before the first message is sent. Pressing `n`
/// again on that blank screen returns to the Workspace root, since it
/// has no Session of its own to lend a checkout.
#[test]
fn new_sessions_share_the_checkout_then_return_to_the_root_from_a_blank_screen() {
    let viewed = workspace("viewed", "/work/viewed");
    let checkout = Path::new("/state/worktrees/viewed/rename-the-picker-styra-7");
    let selection = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
    let mut app = App::pending(selection);
    app.workspace.id = Some(viewed.id.clone());
    app.session_id = "styra-7".into();
    app.workspace.enter(checkout.to_path_buf());

    let pending = pending_app_from_session(&app, &viewed);

    assert_eq!(pending.checkout_from.as_deref(), Some("styra-7"));
    assert_eq!(pending.workspace.root(), Some(checkout));
    assert_eq!(
        pending.workspace.working_directory_or_current(),
        Some(checkout.to_path_buf())
    );

    let second = pending_app_from_session(&pending, &viewed);
    assert_eq!(second.checkout_from, None);
    assert_eq!(second.workspace.root(), Some(Path::new("/work/viewed")));
}

/// The Workspace being viewed can be gone by the time `n` is pressed, in
/// which case the blank screen lands somewhere else. It starts there
/// plainly — at that Workspace's own root, and without asking to share a
/// checkout across Workspaces, which the server refuses.
#[test]
fn a_new_session_landing_in_another_workspace_does_not_inherit_the_checkout() {
    let active = workspace("first", "/work/first");
    let selection = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
    let mut app = App::pending(selection);
    app.workspace.id = Some("since-removed".into());
    app.session_id = "source-session".into();
    app.workspace
        .enter(PathBuf::from("/state/worktrees/since-removed/styra-3"));

    let landed_in = workspace_for_new_session(&app, &active, std::slice::from_ref(&active));
    let pending = pending_app_from_session(&app, &landed_in);

    assert_eq!(landed_in, active);
    assert_eq!(pending.checkout_from, None);
    assert_eq!(pending.workspace.root(), Some(Path::new("/work/first")));
}

/// A new session started from a Workspace begins on that Workspace's own
/// standing policy, while the operator's own inputs travel with them.
#[test]
fn a_new_session_starts_from_the_policy_of_the_workspace_it_lands_in() {
    let selection = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
    let mut landed_in = workspace("second", "/work/second");
    landed_in.launch = styra_protocol::LaunchPolicy {
        templates: vec!["rust".into()],
        ..styra_protocol::LaunchPolicy::default()
    };
    let carried = styra_protocol::LaunchPolicy {
        templates: vec!["browser".into()],
        ..styra_protocol::LaunchPolicy::default()
    };

    let pending = pending_app(selection, carried.clone(), &landed_in);

    assert_eq!(pending.launch.workspace, landed_in.launch);
    assert_eq!(pending.launch.interaction, carried);
    assert_eq!(
        pending.launch.effective().templates,
        vec!["rust".to_owned(), "browser".to_owned()]
    );
}

fn launcher_fixture(selection: &str) -> (tempfile::TempDir, App, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("defaults.json");
    let mut app = App::pending(Selection::parse(selection).expect("valid test selection"));
    app.open_launcher();
    (directory, app, path)
}

fn press_launcher(app: &mut App, path: &Path, key: KeyEvent) {
    launcher::handle_key(app, key, path);
}

fn type_launcher(app: &mut App, path: &Path, text: &str) {
    for character in text.chars() {
        press_launcher(app, path, KeyEvent::from(KeyCode::Char(character)));
    }
}

#[test]
fn confirming_the_launcher_selects_for_this_workspace_without_saving_a_default() {
    let (_directory, mut app, path) = launcher_fixture("claude:claude-sonnet-5/max");
    let selection = app.selection.clone();

    press_launcher(&mut app, &path, press(&keys::LAUNCHER_SELECT));

    assert_eq!(app.selection, selection);
    assert!(!path.exists());
}

/// Moving the picker and confirming remembers the model, so the next
/// picker lists it first — without touching the saved defaults, which only
/// saving a default writes.
#[test]
fn moving_the_launcher_and_confirming_remembers_the_model() {
    let (_directory, mut app, path) = launcher_fixture("claude");
    let opened_on = app.launcher.as_ref().unwrap().selection();

    // Two rows down and one back up.
    for binding in [
        &keys::LAUNCHER_NEXT,
        &keys::LAUNCHER_NEXT,
        &keys::LAUNCHER_PREV,
    ] {
        press_launcher(&mut app, &path, press(binding));
    }
    let moved_to = app.launcher.as_ref().unwrap().selection();
    assert_ne!(moved_to, opened_on, "the cursor moved");

    press_launcher(&mut app, &path, press(&keys::LAUNCHER_SELECT));
    assert_eq!(app.selection, moved_to);
    assert!(
        !path.exists(),
        "confirming does not save a default selection"
    );
    assert_eq!(
        preferences::load_recent_models(&path),
        vec![moved_to.model.clone()]
    );

    // A picker opened with that ordering puts the model at the top.
    app.recent_models = preferences::load_recent_models(&path);
    app.open_launcher();
    let launcher = app.launcher.as_ref().unwrap();
    assert!(
        launcher
            .labels()
            .first()
            .is_some_and(|label| label.contains(&moved_to.model)),
        "the model just used leads the list"
    );
}

/// The list is typed at directly — no key opens a query first, because
/// every printable key is already a letter of one. Selecting then launches on
/// what the query found.
#[test]
fn typing_at_the_launcher_narrows_it_and_selecting_launches_on_the_row() {
    let (_directory, mut app, path) = launcher_fixture("claude");

    type_launcher(&mut app, &path, "haiku");
    let narrowed = app.launcher.as_ref().unwrap().selection().model;
    assert!(narrowed.contains("haiku"), "{narrowed}");

    press_launcher(&mut app, &path, press(&keys::LAUNCHER_SELECT));
    assert!(app.launcher.is_none());
    assert_eq!(app.selection.model, narrowed);
}

/// Typing a triple is typing an identifier, so ctrl-W takes back one part
/// of one — and an unbound chord is not mistaken for a letter of a name.
#[test]
fn ctrl_w_deletes_a_word_of_the_launcher_query() {
    let (_directory, mut app, path) = launcher_fixture("claude");
    type_launcher(&mut app, &path, "claude-opus-5-5/mox");

    press_launcher(
        &mut app,
        &path,
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
    );
    let launcher = app.launcher.as_ref().expect("still open");
    assert_eq!(launcher.list.query, "claude-opus-5-5/");
    assert_eq!(launcher.selection().model, "claude-opus-5-5");

    // A chord the picker has no command for types nothing.
    press_launcher(
        &mut app,
        &path,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
    );
    assert_eq!(
        app.launcher.as_ref().unwrap().list.query,
        "claude-opus-5-5/",
        "an unbound chord is not a letter of the query"
    );
}

/// Cancelling widens an over-narrowed list before it abandons the picker:
/// the first cancel is the one the operator meant, the second closes.
#[test]
fn cancelling_clears_the_query_before_it_closes_the_launcher() {
    let (_directory, mut app, path) = launcher_fixture("claude");
    type_launcher(&mut app, &path, "haiku");

    press_launcher(&mut app, &path, press(&keys::LAUNCHER_CANCEL));
    let launcher = app.launcher.as_ref().expect("still open");
    assert!(!launcher.is_filtering(), "the query is gone");
    assert!(
        launcher.selection().model.contains("haiku"),
        "and the row it found is still selected"
    );

    press_launcher(&mut app, &path, press(&keys::LAUNCHER_CANCEL));
    assert!(app.launcher.is_none(), "the second cancel closes it");
}

#[test]
fn the_launcher_can_explicitly_save_the_selection_as_the_default() {
    let (_directory, mut app, path) = launcher_fixture("claude:claude-sonnet-5/max");
    let selection = app.selection.clone();

    press_launcher(&mut app, &path, press(&keys::LAUNCHER_DEFAULT));

    assert_eq!(
        preferences::load_or_default(&path).unwrap().selection,
        selection
    );
}
