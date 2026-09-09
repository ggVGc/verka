//! The live Interactions navigator: the list of Interactions the server is
//! running, and the operator's place in it.
//!
//! Held apart from [`App`](crate::app::App) because none of it depends on the
//! Interaction currently on screen. [`crate::ui::interactions`] renders it.

use std::time::{Duration, Instant};

use styra_server::{InteractionSummary, WorkspaceSummary};

/// How long the cursor must rest on an entry before that Interaction is
/// loaded, matching the Session and Workspace pickers' settle: short enough to
/// feel immediate once the cursor stops, long enough that walking the list
/// costs no loads at all.
const LOAD_SETTLE: Duration = Duration::from_millis(120);

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
    /// Where the cursor is while that is not the Interaction on screen.
    /// `None` — the resting state — means the cursor is on the current
    /// Interaction, so there is no move outstanding.
    cursor: Option<String>,
    /// Set while the cursor has moved but the Interaction under it has not
    /// been loaded yet. Loading is a blocking round-trip for a whole screen,
    /// so holding `j` must not queue one load per row it passes over; the load
    /// waits for the cursor to settle.
    settle_from: Option<Instant>,
}

impl LiveInteractions {
    pub fn open(&mut self, mut items: Vec<InteractionSummary>, workspaces: Vec<WorkspaceSummary>) {
        sort_interactions(&mut items);
        self.items = items;
        self.workspaces = workspaces;
        self.open = true;
    }

    /// Incorporate a periodic server snapshot. Idle acknowledgement belongs to
    /// the server: listing a row cannot accidentally count as seeing it.
    pub fn refresh(&mut self, mut items: Vec<InteractionSummary>) {
        sort_interactions(&mut items);
        self.items = items;
        // An entry another client closed cannot be loaded, and a cursor left
        // pointing at one would keep asking for it every frame.
        if self
            .cursor
            .as_deref()
            .is_some_and(|id| !self.items.iter().any(|interaction| interaction.id == id))
        {
            self.rest();
        }
    }

    /// Number of idle interactions that have not actually been focused since
    /// becoming idle. The server owns this acknowledgement state.
    pub fn idle_notification_count(&self) -> usize {
        self.items
            .iter()
            .filter(|interaction| is_idle(interaction) && interaction.idle_unseen)
            .count()
    }

    pub fn close(&mut self) {
        self.open = false;
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

    /// The entry the cursor rests on. That is the Interaction on screen except
    /// while a move is waiting to settle, or while its load is running.
    pub fn cursor<'a>(&'a self, current: &'a str) -> &'a str {
        self.cursor.as_deref().unwrap_or(current)
    }

    /// The entry under the cursor while it is not yet the one on screen: what
    /// a settled cursor loads, and what the navigator marks as loading.
    pub fn pending(&self, current: &str) -> Option<&InteractionSummary> {
        let cursor = self.cursor.as_deref()?;
        if cursor == current {
            return None;
        }
        self.items
            .iter()
            .find(|interaction| interaction.id == cursor)
    }

    /// The Interaction a rested cursor is due to load, once the move it
    /// followed has settled.
    pub fn due(&self, current: &str) -> Option<&InteractionSummary> {
        if self
            .settle_from
            .is_none_or(|since| since.elapsed() < LOAD_SETTLE)
        {
            return None;
        }
        self.pending(current)
    }

    /// Put the cursor back on the Interaction the screen shows. Called once a
    /// load lands — or fails — so no cursor can ask for the same entry twice.
    pub fn rest(&mut self) {
        self.cursor = None;
        self.settle_from = None;
    }

    pub fn cursor_next(&mut self, current: &str, workspace_id: Option<&str>) {
        let from = self.cursor(current).to_owned();
        if let Some(next) = self.next(&from, workspace_id) {
            self.move_cursor_to(next.id, current);
        }
    }

    pub fn cursor_previous(&mut self, current: &str, workspace_id: Option<&str>) {
        let from = self.cursor(current).to_owned();
        if let Some(previous) = self.previous(&from, workspace_id) {
            self.move_cursor_to(previous.id, current);
        }
    }

    /// The ctrl-j/ctrl-k group skips move the cursor exactly as j/k do, so
    /// crossing several Workspaces costs the one load the cursor comes to rest
    /// on rather than one per group passed through.
    pub fn cursor_next_workspace(&mut self, current: &str, workspace_id: Option<&str>) {
        let from = self.cursor(current).to_owned();
        if let Some(next) = self.next_workspace(&from, workspace_id) {
            self.move_cursor_to(next.id, current);
        }
    }

    pub fn cursor_previous_workspace(&mut self, current: &str, workspace_id: Option<&str>) {
        let from = self.cursor(current).to_owned();
        if let Some(previous) = self.previous_workspace(&from, workspace_id) {
            self.move_cursor_to(previous.id, current);
        }
    }

    /// A cursor that lands back on the Interaction already on screen — by
    /// walking off the end of the list, or straight back to where it started —
    /// has nothing to load, so it cancels the move rather than timing one out.
    fn move_cursor_to(&mut self, id: String, current: &str) {
        if id == current {
            self.rest();
            return;
        }
        self.cursor = Some(id);
        self.settle_from = Some(Instant::now());
    }

    fn next(&self, current: &str, workspace_id: Option<&str>) -> Option<InteractionSummary> {
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

    fn previous(&self, current: &str, workspace_id: Option<&str>) -> Option<InteractionSummary> {
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
    fn next_workspace(
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
    fn previous_workspace(
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
        // Whatever the caller does with the entry chosen here, the cursor is
        // not left on the one that no longer exists.
        self.rest();
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

/// `Pending` is the server summary's name for a live interaction waiting for
/// input (the TUI calls that state `Idle`).
fn is_idle(interaction: &InteractionSummary) -> bool {
    interaction.accepting && interaction.activity == styra_server::InteractionActivity::Pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_server::{DrivaOptions, InteractionActivity};

    fn interaction(id: &str, accepting: bool, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            auto_retry: false,
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
            idle_unseen: false,
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
    fn refresh_keeps_the_servers_unseen_idle_notification() {
        let mut live = LiveInteractions::default();
        let mut other = interaction("other", true, InteractionActivity::Pending);
        other.idle_unseen = true;
        live.refresh(vec![other]);

        assert_eq!(live.idle_notification_count(), 1);
        live.close();
        assert_eq!(live.idle_notification_count(), 1);
    }

    /// Loading an Interaction replaces the whole screen, so a cursor crossing
    /// the list must not ask for one row's load per row it passes over.
    #[test]
    fn a_moving_cursor_defers_its_load_until_it_comes_to_rest() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", true, InteractionActivity::Pending),
                interaction("two", true, InteractionActivity::Pending),
                interaction("three", true, InteractionActivity::Pending),
            ],
            vec![],
        );

        live.cursor_next("one", Some("workspace"));
        live.cursor_next("one", Some("workspace"));

        // Two rows crossed, and neither of the interactions passed over — nor
        // the one now under the cursor — has been asked for yet.
        assert_eq!(live.cursor("one"), "three");
        assert_eq!(
            live.pending("one")
                .map(|interaction| interaction.id.as_str()),
            Some("three")
        );
        assert!(live.due("one").is_none());

        std::thread::sleep(LOAD_SETTLE + Duration::from_millis(20));
        assert_eq!(
            live.due("one").map(|interaction| interaction.id.as_str()),
            Some("three")
        );

        // Once the load has landed the cursor is home again, so the settled
        // move is not made a second time.
        live.rest();
        assert_eq!(live.cursor("three"), "three");
        assert!(live.due("three").is_none());
    }

    #[test]
    fn a_cursor_walked_back_onto_the_current_interaction_loads_nothing() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", true, InteractionActivity::Pending),
                interaction("two", true, InteractionActivity::Pending),
            ],
            vec![],
        );

        live.cursor_next("one", Some("workspace"));
        live.cursor_previous("one", Some("workspace"));

        assert_eq!(live.cursor("one"), "one");
        assert!(live.pending("one").is_none());
        assert!(live.due("one").is_none());
    }

    #[test]
    fn a_cursored_interaction_that_leaves_the_list_releases_the_cursor() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", true, InteractionActivity::Pending),
                interaction("two", true, InteractionActivity::Pending),
            ],
            vec![],
        );
        live.cursor_next("one", Some("workspace"));

        live.refresh(vec![interaction("one", true, InteractionActivity::Pending)]);

        assert_eq!(live.cursor("one"), "one");
        assert!(live.pending("one").is_none());
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

    /// The group jump crosses more of the list per press than j/k, so it has
    /// all the more reason to load only where the cursor stops.
    #[test]
    fn workspace_jumps_defer_their_load_like_the_row_moves_do() {
        let mut other = interaction("other-pending", true, InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut third = interaction("third", true, InteractionActivity::Pending);
        third.workspace_id = "third-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("pending", true, InteractionActivity::Pending),
                other,
                third,
            ],
            vec![],
        );
        let workspace = Some("workspace");

        live.cursor_next_workspace("pending", workspace);
        live.cursor_next_workspace("pending", workspace);

        assert_eq!(live.cursor("pending"), "third");
        assert!(live.due("pending").is_none());

        // Back the way it came, onto the interaction already on screen: two
        // groups crossed in each direction, and nothing to load at the end.
        live.cursor_previous_workspace("pending", workspace);
        live.cursor_previous_workspace("pending", workspace);
        assert_eq!(live.cursor("pending"), "pending");
        assert!(live.pending("pending").is_none());
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
