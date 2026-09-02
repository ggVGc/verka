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
    pub selected: usize,
}

impl LiveInteractions {
    pub fn open(
        &mut self,
        mut items: Vec<InteractionSummary>,
        workspaces: Vec<WorkspaceSummary>,
        current: &str,
    ) {
        sort_interactions(&mut items);
        self.selected = items
            .iter()
            .position(|interaction| interaction.id == current)
            .unwrap_or(0);
        self.items = items;
        self.workspaces = workspaces;
        self.open = true;
    }

    pub fn refresh(
        &mut self,
        mut items: Vec<InteractionSummary>,
        current: &str,
        workspace_id: Option<&str>,
    ) {
        let selected = self
            .items
            .get(self.selected)
            .map(|interaction| interaction.id.as_str())
            .unwrap_or(current);
        sort_interactions(&mut items);
        self.selected = items
            .iter()
            .position(|interaction| interaction.id == selected)
            .or_else(|| {
                items
                    .iter()
                    .position(|interaction| interaction.id == current)
            })
            .unwrap_or(0);
        self.items = items;
        self.ensure_visible(current, workspace_id);
    }

    pub fn selected(&self) -> Option<&InteractionSummary> {
        self.items.get(self.selected)
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

    pub fn select_next(&mut self, workspace_id: Option<&str>) {
        let visible = self.visible_indices(workspace_id);
        let position = visible
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        self.selected = visible
            .get((position + 1).min(visible.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0);
    }

    pub fn select_previous(&mut self, workspace_id: Option<&str>) {
        let visible = self.visible_indices(workspace_id);
        let position = visible
            .iter()
            .position(|index| *index == self.selected)
            .unwrap_or(0);
        self.selected = visible
            .get(position.saturating_sub(1))
            .copied()
            .unwrap_or(0);
    }

    pub fn toggle_workspace_scope(&mut self, current: &str, workspace_id: Option<&str>) {
        self.only_current_workspace = !self.only_current_workspace;
        self.ensure_visible(current, workspace_id);
    }

    pub fn select_id(&mut self, id: &str) {
        if let Some(selected) = self
            .items
            .iter()
            .position(|interaction| interaction.id == id)
        {
            self.selected = selected;
        }
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
            self.selected = 0;
            return None;
        }

        self.selected = removed.min(self.items.len() - 1);
        if self.visible_indices(workspace_id).is_empty() {
            self.only_current_workspace = false;
        }
        let visible = self.visible_indices(workspace_id);
        if !visible.contains(&self.selected) {
            self.selected = visible.first().copied().unwrap_or(0);
        }
        self.selected().cloned()
    }

    fn ensure_visible(&mut self, current: &str, workspace_id: Option<&str>) {
        let visible = self.visible_indices(workspace_id);
        if !visible.contains(&self.selected) {
            self.select_id(current);
        }
        if !visible.contains(&self.selected) {
            self.selected = visible.first().copied().unwrap_or(0);
        }
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
            "running",
        );

        assert!(live.open);
        assert_eq!(
            live.items
                .iter()
                .map(|interaction| interaction.id.as_str())
                .collect::<Vec<_>>(),
            ["idle", "running", "stopped"]
        );
        assert_eq!(live.selected().unwrap().id, "running");
    }

    #[test]
    fn refreshing_live_interactions_keeps_the_highlight_by_identity() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", true, InteractionActivity::Pending),
                interaction("two", true, InteractionActivity::Running),
            ],
            vec![],
            "one",
        );
        live.select_next(Some("workspace"));
        let mut refreshed_two = interaction("two", true, InteractionActivity::Pending);
        refreshed_two.last_message = Some("new response".into());
        live.refresh(
            vec![
                refreshed_two,
                interaction("one", true, InteractionActivity::Running),
            ],
            "one",
            Some("workspace"),
        );

        assert_eq!(live.selected().unwrap().id, "two");
        assert_eq!(
            live.selected().unwrap().last_message.as_deref(),
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
            "current",
        );

        live.toggle_workspace_scope("current", Some("workspace"));
        assert!(live.only_current_workspace);
        assert_eq!(live.visible_indices(Some("workspace")), vec![0, 2]);
        live.select_next(Some("workspace"));
        assert_eq!(live.selected().unwrap().id, "next");

        live.toggle_workspace_scope("next", Some("workspace"));
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
            "stopped",
        );

        let next = live
            .remove_and_select_next("stopped", Some("workspace"))
            .unwrap();

        assert_eq!(next.id, "two");
        assert_eq!(live.selected().unwrap().id, "two");
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
            "current",
        );
        live.toggle_workspace_scope("current", Some("workspace"));

        let next = live
            .remove_and_select_next("current", Some("workspace"))
            .unwrap();

        assert_eq!(next.id, "other");
        assert!(!live.only_current_workspace);
        assert!(live.open);
    }
}
