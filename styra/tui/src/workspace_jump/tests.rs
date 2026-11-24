use super::*;
use crate::interactions::tests::interaction;
use crate::keybindings::press;
use crossterm::event::KeyCode;
use styra_protocol::InteractionActivity;

fn workspace(id: &str, name: &str) -> WorkspaceSummary {
    WorkspaceSummary {
        path: format!("/workspaces/{id}").into(),
        age: String::new(),
        created_at_ms: 0,
        last_accessed_at_ms: 0,
        ..crate::workspace::tests::workspace(id, Some(name), &format!("/src/{id}"))
    }
}

fn in_workspace(id: &str, workspace: &str, activity: InteractionActivity) -> InteractionSummary {
    let mut interaction = interaction(id, activity);
    interaction.workspace_id = workspace.into();
    interaction
}

#[test]
fn only_workspaces_with_live_work_are_offered() {
    let workspaces = [
        workspace("a", "alpha"),
        workspace("b", "beta"),
        workspace("c", "gamma"),
    ];
    let interactions = [
        in_workspace("1", "a", InteractionActivity::Running),
        in_workspace("2", "a", InteractionActivity::Pending),
        in_workspace("3", "b", InteractionActivity::Stopped),
        in_workspace("4", "c", InteractionActivity::Pending),
    ];
    let jump = WorkspaceJump::new(&workspaces, &interactions);
    assert_eq!(
        jump.rows(),
        ["alpha · /src/a (2 live)", "gamma · /src/c (1 live)"]
    );
}

#[test]
fn finished_work_does_not_offer_a_workspace_in_the_live_jump() {
    let workspaces = [workspace("a", "alpha")];
    let mut finished = in_workspace("1", "a", InteractionActivity::Pending);
    finished.completed = styra_protocol::CompletionState::Completed;
    assert!(WorkspaceJump::new(&workspaces, &[finished]).is_empty());
}

#[test]
fn a_named_workspace_can_be_found_by_its_directory_with_a_fuzzy_query() {
    let workspaces = [workspace("billing", "Payments")];
    let interactions = [in_workspace("1", "billing", InteractionActivity::Running)];
    let mut jump = WorkspaceJump::new(&workspaces, &interactions);
    jump.list.query = "/SRC/BLG".into();
    assert_eq!(jump.selected(), Some("billing"));
}

#[test]
fn the_listed_jump_offers_what_the_navigator_lists() {
    let workspaces = [workspace("a", "alpha"), workspace("b", "beta")];
    let interactions = [
        in_workspace("1", "a", InteractionActivity::Running),
        in_workspace("2", "b", InteractionActivity::Stopped),
        in_workspace("3", "b", InteractionActivity::Stopped),
    ];
    let jump = WorkspaceJump::listed(&workspaces, &interactions, |_| true);
    assert_eq!(
        jump.rows(),
        ["alpha · /src/a (1 listed)", "beta · /src/b (2 listed)"]
    );
    let jump = WorkspaceJump::listed(&workspaces, &interactions, |interaction| {
        interaction.activity.accepting()
    });
    assert_eq!(jump.rows(), ["alpha · /src/a (1 listed)"]);
}

#[test]
fn new_asks_for_a_new_interaction_in_the_highlighted_workspace() {
    let workspaces = [workspace("a", "alpha"), workspace("c", "gamma")];
    let interactions = [
        in_workspace("1", "a", InteractionActivity::Running),
        in_workspace("2", "c", InteractionActivity::Pending),
    ];
    let mut jump = WorkspaceJump::new(&workspaces, &interactions);
    jump.handle_key(press(&keys::WORKSPACE_JUMP_NEXT));
    assert_eq!(
        jump.handle_key(press(&keys::WORKSPACE_JUMP_NEW)),
        Outcome::New("c".into())
    );
}

#[test]
fn a_typed_query_chooses_a_workspace() {
    let workspaces = [workspace("a", "alpha"), workspace("c", "gamma")];
    let interactions = [
        in_workspace("1", "a", InteractionActivity::Running),
        in_workspace("2", "c", InteractionActivity::Pending),
    ];
    let mut jump = WorkspaceJump::new(&workspaces, &interactions);
    for character in "gam".chars() {
        jump.handle_key(KeyEvent::from(KeyCode::Char(character)));
    }
    assert_eq!(
        jump.handle_key(press(&keys::WORKSPACE_JUMP_OPEN)),
        Outcome::Open("c".into())
    );
}
