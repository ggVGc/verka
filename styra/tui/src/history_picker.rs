//! A fuzzy list of visits, newest first. Only Enter leaves the current view;
//! a choice names an existing history position rather than recording a visit.

use anyhow::Result;
use crossterm::event::{Event, KeyEventKind};
use std::time::Duration;
use styra_protocol::{SessionSummary, WorkspaceSummary};
use styra_ui::fuzzy_list::{FuzzyList, FuzzyListView};
use styra_ui::query::Edit;
use styra_ui::Ui;

use crate::interaction_history::InteractionHistory;
use crate::interactions::LiveInteractions;
use crate::keybindings as keys;

pub struct Rows {
    indices: Vec<usize>,
    text: Vec<String>,
}

pub fn rows(
    history: &InteractionHistory,
    interactions: &LiveInteractions,
    workspaces: &[WorkspaceSummary],
    sessions: &[SessionSummary],
) -> Rows {
    let mut rows = Rows {
        indices: Vec::new(),
        text: Vec::new(),
    };
    for (index, id) in history.entries().iter().enumerate().rev() {
        let live = interactions.current(id);
        let stored = sessions.iter().find(|session| session.id == *id);
        let name = interactions
            .name(id)
            .or_else(|| live.and_then(|interaction| interaction.name.as_deref()))
            .or_else(|| stored.and_then(|session| session.name.as_deref()));
        let workspace_id = live
            .map(|interaction| interaction.workspace_id.as_str())
            .or_else(|| stored.map(|session| session.workspace_id.as_str()));
        let workspace = workspace_id.map(|id| {
            workspaces
                .iter()
                .find(|workspace| workspace.id == id)
                .map(crate::workspace::display_name)
                .unwrap_or_else(|| id.to_owned())
        });
        let mut text = match name {
            Some(name) => format!("{name} · {id}"),
            None => id.clone(),
        };
        if let Some(workspace) = workspace {
            text.push_str(&format!(" · {workspace}"));
        }
        rows.indices.push(index);
        rows.text.push(text);
    }
    rows
}

pub fn run(terminal: &mut dyn Ui, rows: &Rows) -> Result<Option<usize>> {
    let mut list = FuzzyList::default();
    loop {
        terminal.render_fuzzy_picker(&FuzzyListView {
            title: " styra · interaction history · newest first ",
            rows: &rows.text,
            list: &list,
            focused: true,
            empty_note: "no interactions in history",
            hint: " type to filter · ↑/↓ move · Enter open · Esc cancel ",
        })?;
        let Some(Event::Key(key)) = terminal.poll_event(Duration::from_millis(100))? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        // The same typed-at list controls as the action menu: letters search,
        // while arrows, paging, and Ctrl-J/K move through the matches.
        match key {
            k if keys::ACTION_MENU_CANCEL.matches(k) => return Ok(None),
            k if keys::ACTION_MENU_RUN.matches(k) => {
                if let Some(row) = list.selected_row(&rows.text) {
                    return Ok(Some(rows.indices[row]));
                }
            }
            k if keys::ACTION_MENU_NEXT.matches(k) => list.next(&rows.text),
            k if keys::ACTION_MENU_PREV.matches(k) => list.prev(&rows.text),
            k if keys::ACTION_MENU_PAGE_DOWN.matches(k) => list.page_down(&rows.text),
            k if keys::ACTION_MENU_PAGE_UP.matches(k) => list.page_up(&rows.text),
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    list.edit(edit);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    fn history() -> InteractionHistory {
        let mut history = InteractionHistory::default();
        for id in ["session-a", "session-b", "session-c"] {
            history.record(id);
        }
        history
    }

    fn run_keys(rows: &Rows, keys: impl IntoIterator<Item = KeyCode>) -> Option<usize> {
        let mut terminal = styra_ui::TestUi::new(100, 20).unwrap();
        terminal.script(keys.into_iter().map(|key| Event::Key(KeyEvent::from(key))));
        run(&mut terminal, rows).unwrap()
    }

    #[test]
    fn visits_are_newest_first_and_names_are_fuzzy_searchable() {
        let history = history();
        let mut interactions = LiveInteractions::default();
        interactions.note_name("session-b", Some("Fix retry timeout".into()));
        let rows = rows(&history, &interactions, &[], &[]);
        assert_eq!(rows.indices, [2, 1, 0]);
        assert_eq!(run_keys(&rows, [KeyCode::Enter]), Some(2));
        assert_eq!(
            run_keys(
                &rows,
                "frt".chars().map(KeyCode::Char).chain([KeyCode::Enter])
            ),
            Some(1)
        );
        assert_eq!(history.target("session-c", false), Some((1, "session-b")));
    }

    #[test]
    fn stored_sessions_remain_searchable_by_workspace_name_and_id() {
        let history = history();
        let workspaces = [WorkspaceSummary {
            id: "workspace-id".into(),
            name: Some("Compiler tools".into()),
            host_path: "/compiler".into(),
            git_repository: None,
            path: "/workspaces/compiler".into(),
            session_count: 1,
            age: String::new(),
            created_at_ms: 0,
            last_accessed_at_ms: 0,
            launch: Default::default(),
        }];
        let sessions = [SessionSummary {
            id: "session-a".into(),
            name: Some("Repair parser".into()),
            first_prompt: None,
            tags: Vec::new(),
            workspace_id: "workspace-id".into(),
            path: "/workspaces/compiler/session-a".into(),
            selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
            age: String::new(),
            created_at_ms: None,
            last_event_at_ms: None,
            last_event_age: String::new(),
            origin: None,
            branched_from: None,
            completed: Default::default(),
            integration: None,
        }];
        let rows = rows(&history, &LiveInteractions::default(), &workspaces, &sessions);
        for query in ["cptl", "rpp", "session-a"] {
            assert_eq!(
                run_keys(
                    &rows,
                    query.chars().map(KeyCode::Char).chain([KeyCode::Enter])
                ),
                Some(0),
                "{query}"
            );
        }
        // Entries without server metadata are still selectable by stable id.
        assert_eq!(
            run_keys(
                &rows,
                "session-b"
                    .chars()
                    .map(KeyCode::Char)
                    .chain([KeyCode::Enter])
            ),
            Some(1)
        );
    }

    #[test]
    fn cancel_and_empty_matches_do_not_choose_or_change_history() {
        let history = history();
        let rows = rows(&history, &LiveInteractions::default(), &[], &[]);
        assert_eq!(run_keys(&rows, [KeyCode::Down, KeyCode::Esc]), None);
        assert_eq!(
            run_keys(&rows, [KeyCode::Char('z'), KeyCode::Enter, KeyCode::Esc]),
            None
        );
        assert_eq!(history.target("session-c", false), Some((1, "session-b")));
    }

    #[test]
    fn choosing_a_visit_keeps_the_forward_trail() {
        let mut history = history();
        let rows = rows(&history, &LiveInteractions::default(), &[], &[]);
        let index = run_keys(&rows, [KeyCode::Down, KeyCode::Enter]).unwrap();
        history.arrive(index);
        assert_eq!(history.target("session-b", true), Some((2, "session-c")));
        assert_eq!(history.entries().len(), 3);
    }
}
