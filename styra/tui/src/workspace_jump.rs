//! The Workspace jump: the Workspaces with live work, in a list that narrows
//! as it is typed at, for landing the live-interactions navigator on the first
//! Interaction of one.
//!
//! Unlike the Workspace picker it does not leave the session: choosing a
//! Workspace only moves the navigator's cursor, so the Interaction it lands on
//! loads the way a j/k step would.

use crate::keybindings as keys;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use styra_protocol::{InteractionSummary, WorkspaceSummary};
use styra_ui::fuzzy_list::FuzzyList;

/// What a key did to the open list.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The list stays open.
    Stay,
    /// Closed without choosing.
    Cancel,
    /// Closed on the Workspace with this id.
    Open(String),
}

pub struct WorkspaceJump {
    /// The Workspace each row stands for.
    ids: Vec<String>,
    /// Each Workspace's name, with how many of its Interactions are live.
    rows: Vec<String>,
    pub list: FuzzyList,
}

impl WorkspaceJump {
    /// The Workspaces with at least one Interaction whose agent still takes
    /// messages — what the overview would tile — in the order `workspaces`
    /// lists them.
    pub fn new(workspaces: &[WorkspaceSummary], interactions: &[InteractionSummary]) -> Self {
        let mut ids = Vec::new();
        let mut rows = Vec::new();
        for workspace in workspaces {
            let live = interactions
                .iter()
                .filter(|interaction| is_live_in(interaction, &workspace.id))
                .count();
            if live == 0 {
                continue;
            }
            ids.push(workspace.id.clone());
            rows.push(format!(
                "{} ({live} live)",
                crate::workspace::display_name(workspace)
            ));
        }
        Self {
            ids,
            rows,
            list: FuzzyList::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key {
            k if keys::WORKSPACE_JUMP_CANCEL.matches(k) => return Outcome::Cancel,
            k if keys::WORKSPACE_JUMP_OPEN.matches(k) => {
                return match self.list.selected_row(&self.rows) {
                    Some(index) => Outcome::Open(self.ids[index].clone()),
                    // An over-narrow query is not a reason to lose it.
                    None => Outcome::Stay,
                };
            }
            k if keys::WORKSPACE_JUMP_NEXT.matches(k) => self.list.next(&self.rows),
            k if keys::WORKSPACE_JUMP_PREV.matches(k) => self.list.prev(&self.rows),
            k if keys::WORKSPACE_JUMP_DELETE_WORD.matches(k) => self.list.delete_word(),
            k if k.code == KeyCode::Backspace => self.list.backspace(),
            KeyEvent {
                code: KeyCode::Char(character),
                modifiers,
                ..
            } if !character.is_control()
                && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.list.push(character)
            }
            _ => {}
        }
        Outcome::Stay
    }
}

/// Whether `workspace_id` has an Interaction the jump would land on: one whose
/// agent still takes messages, as the overview tiles. The Workspace picker
/// asks the same question, so choosing a Workspace there lands where `V`
/// would.
pub fn has_live_work(interactions: &[InteractionSummary], workspace_id: &str) -> bool {
    interactions
        .iter()
        .any(|interaction| is_live_in(interaction, workspace_id))
}

fn is_live_in(interaction: &InteractionSummary, workspace_id: &str) -> bool {
    interaction.workspace_id == workspace_id
        && interaction.activity.accepting()
        && !interaction.completed.is_done()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactions::tests::interaction;
    use std::path::PathBuf;
    use styra_protocol::InteractionActivity;

    fn workspace(id: &str, name: &str) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: Some(name.into()),
            host_path: PathBuf::from(format!("/src/{id}")),
            git_repository: None,
            path: PathBuf::from(format!("/workspaces/{id}")),
            session_count: 0,
            age: String::new(),
            created_at_ms: 0,
            last_accessed_at_ms: 0,
            launch: Default::default(),
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
        assert_eq!(jump.rows(), ["alpha (2 live)", "gamma (1 live)"]);
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
            jump.handle_key(KeyEvent::from(KeyCode::Enter)),
            Outcome::Open("c".into())
        );
    }
}
