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

    /// The visible indices in the order [`crate::ui::interactions`] draws
    /// them: in All scope the entries are grouped under their Workspace
    /// heading, so j/k has to walk that order rather than the raw item order.
    pub fn display_indices(&self, workspace_id: Option<&str>) -> Vec<usize> {
        let visible = self.visible_indices(workspace_id);
        if self.only_current_workspace {
            return visible;
        }

        let mut ordered = Vec::with_capacity(visible.len());
        for leader in &visible {
            if ordered.contains(leader) {
                continue;
            }
            let workspace_id = &self.items[*leader].workspace_id;
            ordered.extend(
                visible
                    .iter()
                    .copied()
                    .filter(|index| self.items[*index].workspace_id == *workspace_id),
            );
        }
        ordered
    }

    pub fn next(&self, current: &str, workspace_id: Option<&str>) -> Option<InteractionSummary> {
        let visible = self.display_indices(workspace_id);
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
        let visible = self.display_indices(workspace_id);
        let position = visible
            .iter()
            .position(|index| self.items[*index].id == current)
            .unwrap_or(0);
        visible
            .get(position.saturating_sub(1))
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    /// The first entry of the Workspace group after the current one, for the
    /// ctrl-j jump. Only meaningful in All scope, where the display is grouped
    /// under Workspace headings; in Workspace scope there is a single group and
    /// no jump to make.
    pub fn next_workspace(
        &self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let leaders = self.workspace_leaders(workspace_id);
        let group = self.group_of(current, workspace_id)?;
        leaders
            .get(group + 1)
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    /// The counterpart to [`Self::next_workspace`]. From the middle of a group
    /// it lands on that group's own first entry, so ctrl-k always walks up to a
    /// heading before leaving it; from a group's first entry it moves to the
    /// group above.
    pub fn previous_workspace(
        &self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let leaders = self.workspace_leaders(workspace_id);
        let group = self.group_of(current, workspace_id)?;
        let leader = self.items.get(*leaders.get(group)?)?;
        if leader.id != current {
            return Some(leader.clone());
        }
        leaders
            .get(group.checked_sub(1)?)
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    /// The first visible entry of each Workspace group, in display order.
    /// Empty in Workspace scope: the entries are not grouped there.
    fn workspace_leaders(&self, workspace_id: Option<&str>) -> Vec<usize> {
        if self.only_current_workspace {
            return Vec::new();
        }

        let mut leaders: Vec<usize> = Vec::new();
        for index in self.display_indices(workspace_id) {
            let workspace_id = &self.items[index].workspace_id;
            if leaders
                .last()
                .is_none_or(|leader| self.items[*leader].workspace_id != *workspace_id)
            {
                leaders.push(index);
            }
        }
        leaders
    }

    /// Which Workspace group `current` sits in, counted over
    /// [`Self::workspace_leaders`].
    fn group_of(&self, current: &str, workspace_id: Option<&str>) -> Option<usize> {
        let current = self
            .items
            .iter()
            .find(|interaction| interaction.id == current)?;
        self.workspace_leaders(workspace_id)
            .iter()
            .position(|leader| self.items[*leader].workspace_id == current.workspace_id)
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

/// The Interaction the navigator would land on first within `workspace_id`:
/// the live ones in the order [`sort_interactions`] gives them, so one waiting
/// on the operator outranks one mid-turn.
///
/// Stopped Interactions are not candidates. Entering a Workspace should land on
/// work that can still be talked to, and a stopped Interaction is reached the
/// same way it always was, through the Session picker.
pub fn first_live_in_workspace(
    interactions: &[InteractionSummary],
    workspace_id: &str,
) -> Option<InteractionSummary> {
    let mut live = interactions
        .iter()
        .filter(|interaction| interaction.accepting && interaction.workspace_id == workspace_id)
        .cloned()
        .collect::<Vec<_>>();
    sort_interactions(&mut live);
    live.into_iter().next()
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
                base: Vec::new(),
                mounts: vec![],
            },
            accepting,
            activity,
            last_message: None,
            events: 0,
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
    fn navigation_follows_the_workspace_grouped_display_order() {
        // Sorting by activity interleaves the two Workspaces, so raw item
        // order and the grouped order the navigator draws disagree.
        let mut other_pending = interaction("other-pending", true, InteractionActivity::Pending);
        other_pending.workspace_id = "other-workspace".into();
        let mut other_running = interaction("other-running", true, InteractionActivity::Running);
        other_running.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("pending", true, InteractionActivity::Pending),
                other_pending,
                interaction("running", true, InteractionActivity::Running),
                other_running,
            ],
            vec![],
        );

        let ordered = live
            .display_indices(Some("workspace"))
            .into_iter()
            .map(|index| live.items[index].id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            ["pending", "running", "other-pending", "other-running"]
        );

        assert_eq!(
            live.next("pending", Some("workspace")).unwrap().id,
            "running"
        );
        assert_eq!(
            live.next("running", Some("workspace")).unwrap().id,
            "other-pending"
        );
        assert_eq!(
            live.previous("other-pending", Some("workspace"))
                .unwrap()
                .id,
            "running"
        );
        assert_eq!(
            live.previous("other-running", Some("workspace"))
                .unwrap()
                .id,
            "other-pending"
        );
    }

    #[test]
    fn workspace_jumps_walk_the_first_entry_of_each_group() {
        let mut other_pending = interaction("other-pending", true, InteractionActivity::Pending);
        other_pending.workspace_id = "other-workspace".into();
        let mut other_running = interaction("other-running", true, InteractionActivity::Running);
        other_running.workspace_id = "other-workspace".into();
        let mut third = interaction("third", true, InteractionActivity::Running);
        third.workspace_id = "third-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("pending", true, InteractionActivity::Pending),
                other_pending,
                interaction("running", true, InteractionActivity::Running),
                other_running,
                third,
            ],
            vec![],
        );
        let workspace = Some("workspace");

        // Display order: pending, running | other-pending, other-running | third
        assert_eq!(
            live.next_workspace("pending", workspace).unwrap().id,
            "other-pending"
        );
        assert_eq!(
            live.next_workspace("running", workspace).unwrap().id,
            "other-pending"
        );
        assert_eq!(
            live.next_workspace("other-running", workspace).unwrap().id,
            "third"
        );
        assert!(live.next_workspace("third", workspace).is_none());

        // From the middle of a group, ctrl-k stops at that group's own first
        // entry before leaving it.
        assert_eq!(
            live.previous_workspace("other-running", workspace)
                .unwrap()
                .id,
            "other-pending"
        );
        assert_eq!(
            live.previous_workspace("other-pending", workspace)
                .unwrap()
                .id,
            "pending"
        );
        assert_eq!(
            live.previous_workspace("running", workspace).unwrap().id,
            "pending"
        );
        assert!(live.previous_workspace("pending", workspace).is_none());
    }

    #[test]
    fn workspace_jumps_do_nothing_in_workspace_scope() {
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

        assert!(live.next_workspace("current", Some("workspace")).is_none());
        assert!(live.previous_workspace("next", Some("workspace")).is_none());
    }

    #[test]
    fn first_live_in_workspace_prefers_the_one_waiting_on_the_operator() {
        let mut other = interaction("other", true, InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let interactions = vec![
            other,
            interaction("stopped", false, InteractionActivity::Pending),
            interaction("running", true, InteractionActivity::Running),
            interaction("idle", true, InteractionActivity::Pending),
        ];

        assert_eq!(
            first_live_in_workspace(&interactions, "workspace")
                .unwrap()
                .id,
            "idle"
        );
        assert_eq!(
            first_live_in_workspace(&interactions, "other-workspace")
                .unwrap()
                .id,
            "other"
        );
    }

    #[test]
    fn a_workspace_whose_interactions_all_stopped_has_no_live_entry() {
        let interactions = vec![
            interaction("stopped", false, InteractionActivity::Pending),
            interaction("also-stopped", false, InteractionActivity::Running),
        ];

        assert!(first_live_in_workspace(&interactions, "workspace").is_none());
        assert!(first_live_in_workspace(&interactions, "unknown").is_none());
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
