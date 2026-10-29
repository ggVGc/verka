//! The Workspace jump: the Workspaces with live work (`v`), or with anything
//! the live-interactions navigator lists (`V`), in a list that narrows as it
//! is typed at, for landing the navigator on the first Interaction of one.
//!
//! Unlike the Workspace picker it does not leave the session: choosing a
//! Workspace only moves the navigator's cursor, so the Interaction it lands on
//! loads the way a j/k step would. Ctrl-N leaves it for a new Interaction in
//! the highlighted Workspace instead.

use crate::keybindings as keys;
use crossterm::event::KeyEvent;
use styra_protocol::{InteractionSummary, WorkspaceSummary};
use styra_ui::fuzzy_list::FuzzyList;
use styra_ui::query::Edit;

/// What a key did to the open list.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The list stays open.
    Stay,
    /// Closed without choosing.
    Cancel,
    /// Closed on the Workspace with this id.
    Open(String),
    /// Closed asking for a new Interaction in the Workspace with this id.
    New(String),
}

pub struct WorkspaceJump {
    /// The Workspace each row stands for.
    ids: Vec<String>,
    /// Each Workspace's name, with how many of its Interactions are live.
    rows: Vec<String>,
    pub list: FuzzyList,
    /// The grid as it stood when the jump opened over it, with the list's
    /// Workspace scope: the highlighted Workspace is previewed on the grid,
    /// and backing out puts this back.
    pub grid: Option<(crate::overview::Overview, bool)>,
}

impl WorkspaceJump {
    /// The Workspaces with at least one Interaction whose agent still takes
    /// messages — what the overview would tile — in the order `workspaces`
    /// lists them.
    pub fn new(workspaces: &[WorkspaceSummary], interactions: &[InteractionSummary]) -> Self {
        Self::counting(
            workspaces,
            interactions,
            "live",
            crate::interactions::is_live,
        )
    }

    /// The Workspaces with at least one Interaction for which `listed` holds —
    /// the navigator's own listing criteria — in the order `workspaces` lists
    /// them.
    pub fn listed(
        workspaces: &[WorkspaceSummary],
        interactions: &[InteractionSummary],
        listed: impl Fn(&InteractionSummary) -> bool,
    ) -> Self {
        Self::counting(workspaces, interactions, "listed", listed)
    }

    fn counting(
        workspaces: &[WorkspaceSummary],
        interactions: &[InteractionSummary],
        label: &str,
        include: impl Fn(&InteractionSummary) -> bool,
    ) -> Self {
        let mut ids = Vec::new();
        let mut rows = Vec::new();
        for workspace in workspaces {
            let count = interactions
                .iter()
                .filter(|interaction| interaction.workspace_id == workspace.id)
                .filter(|interaction| include(interaction))
                .count();
            if count == 0 {
                continue;
            }
            ids.push(workspace.id.clone());
            rows.push(format!(
                "{} ({count} {label})",
                crate::workspace::search_text(workspace)
            ));
        }
        Self {
            ids,
            rows,
            list: FuzzyList::default(),
            grid: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    /// The Workspaces whose rows the query leaves standing.
    pub fn matching(&self) -> Vec<String> {
        self.list
            .matches(&self.rows)
            .into_iter()
            .map(|found| self.ids[found.index].clone())
            .collect()
    }

    /// The Workspace the highlighted row stands for, if the query leaves one.
    pub fn selected(&self) -> Option<&str> {
        self.list
            .selected_row(&self.rows)
            .map(|index| self.ids[index].as_str())
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
            k if keys::WORKSPACE_JUMP_NEW.matches(k) => {
                return match self.list.selected_row(&self.rows) {
                    Some(index) => Outcome::New(self.ids[index].clone()),
                    None => Outcome::Stay,
                };
            }
            k if keys::WORKSPACE_JUMP_NEXT.matches(k) => self.list.next(&self.rows),
            k if keys::WORKSPACE_JUMP_PREV.matches(k) => self.list.prev(&self.rows),
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    self.list.edit(edit);
                }
            }
        }
        Outcome::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactions::tests::interaction;
    use crossterm::event::{KeyCode, KeyModifiers};
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

    fn in_workspace(
        id: &str,
        workspace: &str,
        activity: InteractionActivity,
    ) -> InteractionSummary {
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
    fn ctrl_n_asks_for_a_new_interaction_in_the_highlighted_workspace() {
        let workspaces = [workspace("a", "alpha"), workspace("c", "gamma")];
        let interactions = [
            in_workspace("1", "a", InteractionActivity::Running),
            in_workspace("2", "c", InteractionActivity::Pending),
        ];
        let mut jump = WorkspaceJump::new(&workspaces, &interactions);
        jump.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(
            jump.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
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
            jump.handle_key(KeyEvent::from(KeyCode::Enter)),
            Outcome::Open("c".into())
        );
    }
}
