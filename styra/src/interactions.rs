//! The live Interactions navigator: the list of Interactions the server is
//! running, and the operator's place in it.
//!
//! Held apart from [`App`](crate::app::App) because none of it depends on the
//! Interaction currently on screen. [`crate::ui::interactions`] renders it.

use styra_server::{InteractionSummary, WorkspaceSummary};

/// The live interactions navigator embedded above the main event list.
///
/// It is deliberately only navigation state: the selected interaction's full
/// history lives in the ordinary [`App`](crate::app::App) fields, so there is
/// no second preview cache or interaction screen to keep in sync.
#[derive(Clone, Debug, Default)]
pub struct LiveInteractions {
    pub open: bool,
    pub only_current_workspace: bool,
    pub items: Vec<InteractionSummary>,
    pub workspaces: Vec<WorkspaceSummary>,
}

impl LiveInteractions {
    pub fn open(&mut self, mut items: Vec<InteractionSummary>, workspaces: Vec<WorkspaceSummary>) {
        sort_interactions(&mut items);
        self.items = items;
        self.workspaces = workspaces;
        self.open = true;
    }

    pub fn refresh(&mut self, mut items: Vec<InteractionSummary>) {
        sort_interactions(&mut items);
        self.items = items;
    }

    pub fn current(&self, current: &str) -> Option<&InteractionSummary> {
        self.items
            .iter()
            .find(|interaction| interaction.id == current)
    }

    pub fn visible_indices(&self, workspace_id: Option<&str>) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, interaction)| {
                (!self.only_current_workspace
                    || workspace_id.is_some_and(|id| interaction.workspace_id == id))
                .then_some(index)
            })
            .collect()
    }

    pub fn next(&self, current: &str, workspace_id: Option<&str>) -> Option<InteractionSummary> {
        let visible = self.visible_indices(workspace_id);
        let index = visible
            .iter()
            .position(|index| self.items[*index].id == current)
            .map(|position| (position + 1).min(visible.len().saturating_sub(1)))
            .unwrap_or(0);
        visible
            .get(index)
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    pub fn previous(
        &self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let visible = self.visible_indices(workspace_id);
        let position = visible
            .iter()
            .position(|index| self.items[*index].id == current)
            .unwrap_or(0);
        visible
            .get(position.saturating_sub(1))
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    pub fn toggle_workspace_scope(&mut self) {
        self.only_current_workspace = !self.only_current_workspace;
    }

    /// Remove an interaction and select the entry now occupying its place.
    /// If the current Workspace has no entries left, reveal All so the next
    /// interaction can still become current without closing the navigator.
    pub fn remove_and_select_next(
        &mut self,
        id: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let removed = self
            .items
            .iter()
            .position(|interaction| interaction.id == id)?;
        self.items.remove(removed);
        if self.items.is_empty() {
            return None;
        }

        if self.visible_indices(workspace_id).is_empty() {
            self.only_current_workspace = false;
        }
        let visible = self.visible_indices(workspace_id);
        visible
            .iter()
            .copied()
            .find(|index| *index >= removed)
            .or_else(|| visible.last().copied())
            .and_then(|index| self.items.get(index))
            .cloned()
    }
}

fn sort_interactions(interactions: &mut [InteractionSummary]) {
    interactions.sort_by_key(|interaction| {
        if !interaction.accepting {
            2
        } else {
            match interaction.activity {
                styra_server::InteractionActivity::Pending => 0,
                styra_server::InteractionActivity::Running
                | styra_server::InteractionActivity::Background => 1,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_server::{DrivaOptions, InteractionActivity};

    fn interaction(id: &str, accepting: bool, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            id: id.into(),
            name: None,
            workspace_id: "workspace".into(),
            selection: styra_server::agent::Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            driva: DrivaOptions {
                isolation_backend: "none".into(),
                command: vec![],
                working_directory: PathBuf::from("/workspace"),
                network: false,
                mounts: vec![],
            },
            accepting,
            activity,
            last_message: None,
        }
    }

    #[test]
    fn live_interactions_open_on_the_current_session_in_status_order() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("stopped", false, InteractionActivity::Running),
                interaction("running", true, InteractionActivity::Running),
                interaction("idle", true, InteractionActivity::Pending),
            ],
            vec![],
        );

        assert!(live.open);
        assert_eq!(
            live.items
                .iter()
                .map(|interaction| interaction.id.as_str())
                .collect::<Vec<_>>(),
            ["idle", "running", "stopped"]
        );
        assert_eq!(live.current("running").unwrap().id, "running");
    }

    #[test]
    fn refreshing_live_interactions_keeps_current_lookup_by_identity() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", true, InteractionActivity::Pending),
                interaction("two", true, InteractionActivity::Running),
            ],
            vec![],
        );
        let next = live.next("one", Some("workspace")).unwrap();
        assert_eq!(next.id, "two");
        let mut refreshed_two = interaction("two", true, InteractionActivity::Pending);
        refreshed_two.last_message = Some("new response".into());
        live.refresh(vec![
            refreshed_two,
            interaction("one", true, InteractionActivity::Running),
        ]);

        assert_eq!(live.current("two").unwrap().id, "two");
        assert_eq!(
            live.current("two").unwrap().last_message.as_deref(),
            Some("new response")
        );
    }

    #[test]
    fn workspace_scope_filters_navigation_and_can_return_to_all() {
        let mut other = interaction("other", true, InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", true, InteractionActivity::Pending),
                other,
                interaction("next", true, InteractionActivity::Running),
            ],
            vec![],
        );

        live.toggle_workspace_scope();
        assert!(live.only_current_workspace);
        assert_eq!(live.visible_indices(Some("workspace")), vec![0, 2]);
        assert_eq!(live.next("current", Some("workspace")).unwrap().id, "next");

        live.toggle_workspace_scope();
        assert!(!live.only_current_workspace);
        assert_eq!(live.visible_indices(Some("workspace")), vec![0, 1, 2]);
    }

    #[test]
    fn deleting_an_interaction_selects_the_entry_that_replaces_it() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", true, InteractionActivity::Pending),
                interaction("two", true, InteractionActivity::Running),
                interaction("stopped", false, InteractionActivity::Running),
            ],
            vec![],
        );

        let next = live
            .remove_and_select_next("stopped", Some("workspace"))
            .unwrap();

        assert_eq!(next.id, "two");
        assert!(live.open);
    }

    #[test]
    fn deleting_the_last_scoped_interaction_falls_back_to_all() {
        let mut other = interaction("other", true, InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", false, InteractionActivity::Running),
                other,
            ],
            vec![],
        );
        live.toggle_workspace_scope();

        let next = live
            .remove_and_select_next("current", Some("workspace"))
            .unwrap();

        assert_eq!(next.id, "other");
        assert!(!live.only_current_workspace);
        assert!(live.open);
    }
}
