use super::*;

pub(crate) fn workspace(id: &str, name: Option<&str>, host_path: &str) -> WorkspaceSummary {
    WorkspaceSummary {
        id: id.into(),
        name: name.map(str::to_owned),
        host_path: host_path.into(),
        git_repository: None,
        path: format!("/state/workspaces/{id}").into(),
        session_count: 0,
        age: "now".into(),
        created_at_ms: 1,
        last_accessed_at_ms: 1,
        launch: Default::default(),
    }
}

#[test]
fn a_named_workspace_is_shown_under_its_name() {
    assert_eq!(
        display_name(&workspace("w-1", Some("payments"), "/home/op/svc")),
        "payments"
    );
}

/// Most Workspaces are never named, so the directory they stand for is
/// what the operator recognises them by.
#[test]
fn an_unnamed_workspace_is_shown_under_its_directory() {
    assert_eq!(display_name(&workspace("w-1", None, "/home/op/svc")), "svc");
}

/// Showing a Workspace says which one and what to call it, and leaves
/// where the agent is working alone — a live interaction may have been
/// told to work somewhere other than the root since it started.
#[test]
fn showing_a_workspace_does_not_move_the_agent() {
    let mut location = Location::default();
    location.enter(PathBuf::from("/home/op/svc"));
    location.change_directory(PathBuf::from("/home/op/svc/crates/inner"));

    location.show(&workspace("w-1", Some("payments"), "/home/op/svc"));

    assert_eq!(location.id.as_deref(), Some("w-1"));
    assert_eq!(location.name.as_deref(), Some("payments"));
    assert_eq!(
        location.working_directory_or_current(),
        Some(PathBuf::from("/home/op/svc/crates/inner"))
    );
}

/// A session starts working at the root of the Workspace it was launched
/// in, so recording one sets both. Changing directory then moves only
/// the working directory; reported paths still resolve against the root.
#[test]
fn entering_and_changing_directory_keep_the_workspace_root() {
    let mut location = Location::default();

    location.enter(PathBuf::from("/home/op/project"));

    assert_eq!(location.root(), Some(Path::new("/home/op/project")));
    assert_eq!(
        location.working_directory_or_current(),
        Some(PathBuf::from("/home/op/project"))
    );

    location.change_directory(PathBuf::from("/home/op/project/crates/inner"));

    assert_eq!(location.root(), Some(Path::new("/home/op/project")));
    assert_eq!(
        location.working_directory_or_current(),
        Some(PathBuf::from("/home/op/project/crates/inner"))
    );
}

/// A replayed journal has no live Workspace, but the paths it mentions are
/// still worth resolving against wherever the operator is.
#[test]
fn without_a_workspace_both_questions_fall_back_to_this_directory() {
    let location = Location::default();
    let current = std::env::current_dir().ok();

    assert_eq!(location.root(), None);
    assert_eq!(location.root_or_current_directory(), current);
    assert_eq!(location.working_directory_or_current(), current);
}
