//! The worktree picker: which linked checkouts exist, which Sessions work in
//! each, and a way to jump straight to one of them.
//!
//! A row is a Session in a worktree rather than a worktree, because a Session
//! is what Enter can open: a checkout several Sessions were launched into is
//! as many rows, each carrying the checkout's name so a query typed at either
//! half finds it. A checkout no Session records still gets a row, so the list
//! accounts for every directory, but there is nothing for Enter to open there.

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

use styra_protocol::{CompletionState, WorkspaceSummary, WorktreeSession, WorktreeSummary};
use styra_server::Client;
use styra_ui::fuzzy_list::FuzzyList;
use styra_ui::worktrees::WorktreePickerView;
use styra_ui::Ui;

use crate::help::Help;
use crate::keybindings::{self as keys, Window};
use crate::picker::{handle_help_key, render_help};

/// The Session the operator chose, and the Workspace it belongs to — which,
/// with every Workspace listed, need not be the one being viewed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeChoice {
    pub session_id: String,
    pub workspace_id: String,
}

/// How long a checkout's name may grow in the row before the Session's name
/// is pushed off the screen. Longer names are still shown, unpadded.
const LABEL_WIDTH: usize = 48;

/// One row: a worktree, and the Session in it if any.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    worktree: usize,
    session: Option<usize>,
}

/// The rows for `worktrees` and the text each one is matched and drawn as.
///
/// `workspaces` is given when the list spans every Workspace, and each row
/// then ends with the name of the one it is in: two Workspaces on the same
/// repository give their Sessions the same names, and the Workspace is what
/// tells them apart — and what a query can pick one out by.
fn rows(
    worktrees: &[WorktreeSummary],
    workspaces: Option<&[WorkspaceSummary]>,
) -> (Vec<Row>, Vec<String>) {
    let labels = worktrees.iter().map(worktree_label).collect::<Vec<_>>();
    let width = labels
        .iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0)
        .min(LABEL_WIDTH);
    let mut rows = Vec::new();
    let mut text = Vec::new();
    for (index, (worktree, label)) in worktrees.iter().zip(&labels).enumerate() {
        let suffix = workspaces
            .map(|workspaces| {
                format!(
                    " · in {}",
                    workspace_name(workspaces, &worktree.workspace_id)
                )
            })
            .unwrap_or_default();
        if worktree.sessions.is_empty() {
            rows.push(Row {
                worktree: index,
                session: None,
            });
            text.push(format!("{label:<width$}  no session records it{suffix}"));
        }
        for (session_index, session) in worktree.sessions.iter().enumerate() {
            rows.push(Row {
                worktree: index,
                session: Some(session_index),
            });
            text.push(format!(
                "{label:<width$}  {} · {}{suffix}",
                session_label(session),
                state(session)
            ));
        }
    }
    (rows, text)
}

/// What a worktree is called in its rows: its directory's name, which is the
/// topic and id Styra named it after, or its branch once it has no directory.
fn worktree_label(worktree: &WorktreeSummary) -> String {
    match (&worktree.worktree, &worktree.branch) {
        (Some(path), _) => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            if worktree.exists {
                name
            } else {
                format!("{name} (missing)")
            }
        }
        (None, Some(branch)) => format!("{branch} (branch only)"),
        (None, None) => "(unnamed)".to_owned(),
    }
}

/// What a Workspace is called: its name, or its directory when it has none.
fn workspace_name(workspaces: &[WorkspaceSummary], id: &str) -> String {
    match workspaces.iter().find(|workspace| workspace.id == id) {
        Some(workspace) => workspace
            .name
            .clone()
            .unwrap_or_else(|| workspace.host_path.display().to_string()),
        None => id.to_owned(),
    }
}

fn session_label(session: &WorktreeSession) -> String {
    match &session.name {
        Some(name) => name.clone(),
        None => session.id.clone(),
    }
}

fn state(session: &WorktreeSession) -> &'static str {
    match (session.live, session.completed) {
        (true, _) => "live",
        (false, CompletionState::Active) => "active",
        (false, CompletionState::Completed) => "completed",
        (false, CompletionState::Abandoned) => "abandoned",
        (false, CompletionState::Sealed) => "sealed",
    }
}

/// The lines beneath the list for the row under the cursor: everything about
/// it a row has no room for.
fn detail(
    worktree: &WorktreeSummary,
    session: Option<&WorktreeSession>,
    workspaces: &[WorkspaceSummary],
) -> Vec<(&'static str, String)> {
    let location = match &worktree.worktree {
        Some(path) if worktree.exists => path.display().to_string(),
        Some(path) => format!("{} — not on disk any more", path.display()),
        None => "none; opening the session checks its branch out again".to_owned(),
    };
    let mut lines = vec![
        ("worktree", location),
        (
            "branch",
            worktree
                .branch
                .clone()
                .unwrap_or_else(|| "unknown; no session records this checkout".to_owned()),
        ),
    ];
    lines.push((
        "Workspace",
        workspace_name(workspaces, &worktree.workspace_id),
    ));
    if let Some(session) = session {
        let mut value = session_label(session);
        if session.name.is_some() {
            value.push_str(&format!(" · {}", session.id));
        }
        value.push_str(&format!(" · {}", state(session)));
        lines.push(("session", value));
    }
    if worktree.sessions.len() > 1 {
        lines.push((
            "shared",
            format!("{} sessions work in this checkout", worktree.sessions.len()),
        ));
    }
    lines
}

/// Which row the picker opens on: the Session being viewed when it is in the
/// list, since "where is the checkout I am in" is the first thing to look for.
fn initial_row(worktrees: &[WorktreeSummary], rows: &[Row], current_id: &str) -> usize {
    rows.iter()
        .position(|row| {
            row.session
                .is_some_and(|session| worktrees[row.worktree].sessions[session].id == current_id)
        })
        .unwrap_or(0)
}

/// Run the picker over `worktrees`, which the caller has already fetched for
/// `workspace_id` so that a server unable to list them is reported where the
/// picker was asked for rather than here. The scope key fetches the other
/// scope from `client`.
pub fn run_worktree_picker(
    terminal: &mut dyn Ui,
    client: &Client,
    mut worktrees: Vec<WorktreeSummary>,
    workspace_id: &str,
    current_id: &str,
) -> Result<Option<WorktreeChoice>> {
    let workspaces = client.list_workspaces().unwrap_or_default();
    let mut every_workspace = false;
    let (mut rows, mut text) = rows(&worktrees, None);
    let mut list = FuzzyList::at(&text, initial_row(&worktrees, &rows, current_id));
    let mut notice: Option<String> = None;
    let mut help = Help::default();
    loop {
        let selected = list.selected_row(&text).map(|index| &rows[index]);
        let detail = selected
            .map(|row| {
                let worktree = &worktrees[row.worktree];
                detail(
                    worktree,
                    row.session.map(|session| &worktree.sessions[session]),
                    &workspaces,
                )
            })
            .unwrap_or_default();
        if help.is_open() {
            render_help(terminal, Window::WorktreePicker, &mut help)?;
        } else {
            terminal.render_worktree_picker(&WorktreePickerView {
                scope: if every_workspace {
                    "every Workspace"
                } else {
                    "this Workspace"
                },
                rows: &text,
                list: &list,
                detail: &detail,
                notice: notice.as_deref(),
            })?;
        }
        let Some(Event::Key(key)) = terminal.poll_event(Duration::from_millis(100))? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if handle_help_key(&mut help, key) {
            continue;
        }
        notice = None;
        match key {
            k if keys::WORKTREES_CANCEL.matches(k) => {
                if !list.is_filtering() {
                    return Ok(None);
                }
                list.clear(&text);
            }
            k if keys::WORKTREES_HELP.matches(k) => help.open(),
            k if keys::WORKTREES_NEXT.matches(k) => list.next(&text),
            k if keys::WORKTREES_PREV.matches(k) => list.prev(&text),
            k if keys::WORKTREES_PAGE_DOWN.matches(k) => list.page_down(&text),
            k if keys::WORKTREES_PAGE_UP.matches(k) => list.page_up(&text),
            k if keys::WORKTREES_DELETE_WORD.matches(k) => list.delete_word(),
            k if keys::WORKTREES_SCOPE.matches(k) => {
                // The scope being switched to: back to this Workspace from
                // every one, or out to every one from this.
                let scope = every_workspace.then_some(workspace_id);
                match client.list_worktrees(scope) {
                    Ok(fetched) => {
                        every_workspace = !every_workspace;
                        worktrees = fetched;
                        (rows, text) = self::rows(
                            &worktrees,
                            every_workspace.then_some(workspaces.as_slice()),
                        );
                        list = FuzzyList::at(&text, initial_row(&worktrees, &rows, current_id));
                    }
                    Err(error) => notice = Some(format!("could not list worktrees: {error:#}")),
                }
            }
            k if keys::WORKTREES_OPEN.matches(k) => {
                let Some(row) = list.selected_row(&text).map(|index| &rows[index]) else {
                    continue;
                };
                let worktree = &worktrees[row.worktree];
                match row.session {
                    Some(session) => {
                        return Ok(Some(WorktreeChoice {
                            session_id: worktree.sessions[session].id.clone(),
                            workspace_id: worktree.workspace_id.clone(),
                        }))
                    }
                    None => notice = Some("no session works in this worktree".to_owned()),
                }
            }
            _ => match key.code {
                KeyCode::Backspace => list.backspace(),
                KeyCode::Char(character)
                    if !character.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    list.push(character)
                }
                _ => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
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
        WorkspaceSummary {
            id: id.into(),
            name: None,
            host_path: format!("/home/op/{id}").into(),
            git_repository: None,
            path: format!("/state/workspaces/{id}").into(),
            session_count: 0,
            age: "now".into(),
            created_at_ms: 1,
            last_accessed_at_ms: 1,
            launch: Default::default(),
        }
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
        let gone = detail(&worktrees[1], Some(&worktrees[1].sessions[0]), &workspaces);
        assert_eq!(
            gone[0],
            (
                "worktree",
                "none; opening the session checks its branch out again".to_owned()
            )
        );
    }
}
