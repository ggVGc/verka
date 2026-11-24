use super::*;
use std::path::PathBuf;

fn session(
    id: &str,
    name: Option<&str>,
    completed: CompletionState,
    live: bool,
) -> WorktreeSession {
    WorktreeSession {
        id: id.into(),
        name: name.map(str::to_owned),
        completed,
        live,
    }
}

fn workspace(id: &str) -> WorkspaceSummary {
    crate::workspace::tests::workspace(id, None, &format!("/home/op/{id}"))
}

fn worktrees() -> Vec<WorktreeSummary> {
    vec![
        WorktreeSummary {
            workspace_id: "w-1".into(),
            worktree: Some(PathBuf::from("/state/worktrees/fix-flaky-test-s-1")),
            branch: Some("styra/fix-flaky-test-s-1".into()),
            exists: true,
            sessions: vec![
                session("s-2", None, CompletionState::Completed, false),
                session(
                    "s-1",
                    Some("fix the flaky test"),
                    CompletionState::Active,
                    true,
                ),
            ],
        },
        WorktreeSummary {
            workspace_id: "w-1".into(),
            worktree: None,
            branch: Some("styra/tidy-up-s-3".into()),
            exists: false,
            sessions: vec![session("s-3", None, CompletionState::Sealed, false)],
        },
        WorktreeSummary {
            workspace_id: "w-1".into(),
            worktree: Some(PathBuf::from("/state/worktrees/stray")),
            branch: None,
            exists: true,
            sessions: Vec::new(),
        },
    ]
}

/// A shared checkout is a row per Session, each carrying the checkout's
/// name; a cleaned-up one is named by its branch; and one nothing records
/// still has a row, with no Session to open.
#[test]
fn every_session_in_a_worktree_is_a_row_under_the_worktrees_name() {
    let worktrees = worktrees();
    let (rows, text) = rows(&worktrees, None);

    assert_eq!(
        rows,
        vec![
            Row {
                worktree: 0,
                session: Some(0)
            },
            Row {
                worktree: 0,
                session: Some(1)
            },
            Row {
                worktree: 1,
                session: Some(0)
            },
            Row {
                worktree: 2,
                session: None
            },
        ]
    );
    let width = "styra/tidy-up-s-3 (branch only)".len();
    assert_eq!(
        text,
        vec![
            format!("{:<width$}  s-2 · completed", "fix-flaky-test-s-1"),
            format!(
                "{:<width$}  fix the flaky test · live",
                "fix-flaky-test-s-1"
            ),
            "styra/tidy-up-s-3 (branch only)  s-3 · sealed".to_owned(),
            format!("{:<width$}  no session records it", "stray"),
        ]
    );
}

/// Across every Workspace a row says which one it is in, by name.
#[test]
fn listing_every_workspace_names_each_rows_workspace() {
    let worktrees = worktrees();
    let workspaces = vec![WorkspaceSummary {
        name: Some("payments".into()),
        ..workspace("w-1")
    }];

    let (_, text) = rows(&worktrees, Some(&workspaces));

    assert!(
        text.iter().all(|row| row.ends_with(" · in payments")),
        "{text:#?}"
    );
}

/// The picker opens on the Session being viewed, wherever it sorts.
#[test]
fn the_picker_opens_on_the_session_being_viewed() {
    let worktrees = worktrees();
    let (rows, _) = rows(&worktrees, None);

    assert_eq!(initial_row(&worktrees, &rows, "s-1"), 1);
    assert_eq!(initial_row(&worktrees, &rows, "s-3"), 2);
    assert_eq!(initial_row(&worktrees, &rows, "elsewhere"), 0);
}

/// The pane under the list says where the checkout is and what it is on,
/// and says so plainly for a checkout that has gone.
#[test]
fn the_detail_names_the_path_branch_and_session() {
    let worktrees = worktrees();
    let workspaces = Vec::new();

    let shared = detail(&worktrees[0], Some(&worktrees[0].sessions[1]), &workspaces);
    assert_eq!(
        shared,
        vec![
            ("worktree", "/state/worktrees/fix-flaky-test-s-1".to_owned()),
            ("branch", "styra/fix-flaky-test-s-1".to_owned()),
            ("Workspace", "w-1".to_owned()),
            ("session", "fix the flaky test · s-1 · live".to_owned()),
            ("shared", "2 sessions work in this checkout".to_owned()),
        ]
    );
    let parked = detail(
        &worktrees[0],
        Some(&worktrees[0].sessions[1]),
        &[WorkspaceSummary {
            path: "/state".into(),
            ..workspace("w-1")
        }],
    );
    assert_eq!(
        parked[0],
        ("worktree", "<wt>/fix-flaky-test-s-1".to_owned())
    );
    assert_eq!(
        parked[1],
        ("worktrees", "/state/worktrees (<wt>)".to_owned())
    );
    let gone = detail(&worktrees[1], Some(&worktrees[1].sessions[0]), &workspaces);
    assert_eq!(
        gone[0],
        (
            "worktree",
            "none; opening the session checks its branch out again".to_owned()
        )
    );
}
