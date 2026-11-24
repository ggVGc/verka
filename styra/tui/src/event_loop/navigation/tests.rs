use super::*;
use crate::workspace_jump::WorkspaceJump;
use crossterm::event::KeyCode;
use styra_protocol::{InteractionActivity, WorkspaceSummary};

fn workspaces(ids: &[&str]) -> Vec<WorkspaceSummary> {
    ids.iter()
        .map(|id| WorkspaceSummary {
            path: (*id).into(),
            age: String::new(),
            created_at_ms: 0,
            last_accessed_at_ms: 0,
            ..crate::workspace::tests::workspace(id, Some(id), id)
        })
        .collect()
}

/// Opening the jump over the navigator keeps it open, listing only the
/// Workspace the jump highlights; closing the jump lists them all.
#[test]
fn the_workspace_jump_narrows_the_open_navigator() {
    let press = |character| crossterm::event::KeyEvent::from(KeyCode::Char(character));
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        "one",
    );
    let mut other = crate::interactions::tests::interaction("two", InteractionActivity::Running);
    other.workspace_id = "other".into();
    let interactions = vec![
        crate::interactions::tests::interaction("one", InteractionActivity::Running),
        other,
    ];
    let workspaces = workspaces(&["workspace", "other"]);
    let jump = WorkspaceJump::new(&workspaces, &interactions);
    app.interactions.open(interactions, workspaces);
    app.workspace_jump = Some(jump);
    narrow_navigator_to_jump(&mut app);
    assert_eq!(app.interactions.visible_indices("", None), vec![0]);

    let jump = app.workspace_jump.as_mut().unwrap();
    jump.handle_key(crossterm::event::KeyEvent::from(KeyCode::Down));
    narrow_navigator_to_jump(&mut app);
    assert_eq!(app.interactions.visible_indices("", None), vec![1]);

    for character in "zzz".chars() {
        app.workspace_jump
            .as_mut()
            .unwrap()
            .handle_key(press(character));
    }
    narrow_navigator_to_jump(&mut app);
    assert!(app.interactions.visible_indices("", None).is_empty());

    app.workspace_jump = None;
    narrow_navigator_to_jump(&mut app);
    assert!(app.interactions.open);
    assert_eq!(app.interactions.visible_indices("", None), vec![0, 1]);
}

/// In Workspace scope the jump's highlight is the current Workspace, so
/// highlighting another Workspace lists that one's rows rather than none.
#[test]
fn the_workspace_jump_lists_its_highlight_in_workspace_scope() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        "one",
    );
    app.workspace.id = Some("workspace".into());
    let mut other = crate::interactions::tests::interaction("two", InteractionActivity::Running);
    other.workspace_id = "other".into();
    let interactions = vec![
        crate::interactions::tests::interaction("one", InteractionActivity::Running),
        other,
    ];
    let workspaces = workspaces(&["workspace", "other"]);
    let jump = WorkspaceJump::new(&workspaces, &interactions);
    app.interactions.open(interactions, workspaces);
    app.interactions.only_current_workspace = true;
    app.workspace_jump = Some(jump);
    narrow_navigator_to_jump(&mut app);
    assert_eq!(
        app.interactions.visible_indices("one", Some("workspace")),
        vec![0]
    );

    app.workspace_jump
        .as_mut()
        .unwrap()
        .handle_key(crossterm::event::KeyEvent::from(KeyCode::Down));
    narrow_navigator_to_jump(&mut app);
    assert_eq!(
        app.interactions.visible_indices("one", Some("workspace")),
        vec![1]
    );

    app.workspace_jump = None;
    narrow_navigator_to_jump(&mut app);
    assert_eq!(
        app.interactions.visible_indices("one", Some("workspace")),
        vec![0]
    );
}

/// Over the grid, the jump narrows the grid to its highlighted Workspace
/// and moves only the grid's cursor; nothing is scheduled to load.
#[test]
fn the_workspace_jump_previews_its_highlight_on_the_grid() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        "one",
    );
    app.workspace.id = Some("workspace".into());
    let mut other = crate::interactions::tests::interaction("two", InteractionActivity::Running);
    other.workspace_id = "other".into();
    let interactions = vec![
        crate::interactions::tests::interaction("one", InteractionActivity::Running),
        other,
    ];
    let workspaces = workspaces(&["workspace", "other"]);
    let mut jump = WorkspaceJump::new(&workspaces, &interactions);
    app.interactions.open(interactions, workspaces);
    app.view = View::Overview;
    jump.grid = Some((app.overview.clone(), false));
    app.workspace_jump = Some(jump);

    app.workspace_jump
        .as_mut()
        .unwrap()
        .handle_key(crossterm::event::KeyEvent::from(KeyCode::Down));
    preview_workspace_jump(&mut app);

    assert!(app.interactions.only_current_workspace);
    assert_eq!(
        app.overview
            .indices(&app.interactions, &app.session_id, Some("workspace"))
            .len(),
        1,
        "only the highlighted Workspace's tiles are shown"
    );
    assert_eq!(
        app.overview
            .selected_id(&app.interactions, "one", Some("workspace")),
        Some("two")
    );
    assert!(app.interactions.pending("one").is_none());
}

#[test]
fn a_failed_grid_selection_load_rests_the_shared_cursor_and_keeps_the_grid() {
    let root = tempfile::tempdir().unwrap();
    let client = Client::new(root.path().join("missing.sock"));
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        "one",
    );
    app.interactions.refresh(
        ["one", "two"]
            .into_iter()
            .map(|id| crate::interactions::tests::interaction(id, InteractionActivity::Running))
            .collect(),
    );
    app.view = View::Overview;
    app.overview.running_only = true;
    app.overview.note_rows(&[2]);
    app.overview.right(&app.interactions, "one", None);
    app.interactions.move_cursor_to("two".into(), "one");
    let mut live = Attachment::Detached;

    load_cursored_interaction(&mut app, &mut live, &client, &LaunchPolicy::default());

    assert_eq!(app.session_id, "one");
    assert_eq!(app.view, View::Overview);
    assert!(app.overview.running_only);
    assert_eq!(
        app.overview.selected_id(&app.interactions, "one", None),
        Some("two")
    );
    assert!(app.interactions.pending("one").is_none());
    assert!(!cursored_interaction_is_loadable(&app));
}
