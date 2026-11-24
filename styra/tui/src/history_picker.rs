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
mod tests;
