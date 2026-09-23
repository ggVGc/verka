//! The live Interactions navigator: the list of Interactions the server is
//! running, and the operator's place in it.
//!
//! Held apart from [`App`](crate::app::App) because none of it depends on the
//! Interaction currently on screen. [`crate::presentation::interactions`] renders it.

use std::time::{Duration, Instant};

use styra_protocol::{InteractionSummary, WorkspaceSummary};

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
    /// Completed interactions stay in the server's list, but are normally out
    /// of the navigator until the operator asks to see them. Hiding them is
    /// the listing's own business: the server keeps completion as the reason
    /// an interaction is stopped, and says so about every row.
    pub show_completed: bool,
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
                ((!self.only_current_workspace
                    || workspace_id.is_some_and(|id| interaction.workspace_id == id))
                    && (self.show_completed || !interaction.completed))
                    .then_some(index)
            })
            .collect()
    }

    /// The visible indices in the order [`crate::presentation::interactions`] draws
    /// them: in All scope the entries are grouped under their Workspace
    /// heading, so j/k has to walk that order rather than the raw item order.
    ///
    /// A stopped entry stays where it belongs, in its own Workspace's group and
    /// in item order: its row says that it stopped and why, which is enough to
    /// tell it apart from the work that can still be talked to.
    pub fn display_indices(&self, workspace_id: Option<&str>) -> Vec<usize> {
        let visible = self.visible_indices(workspace_id);
        if self.only_current_workspace {
            return visible;
        }
        grouped_by_workspace(&self.items, visible)
    }

    /// The next Interaction that went idle unseen, from `from` onward in
    /// display order, wrapping past the end of the list: repeated presses walk
    /// every one of them and come back rather than stopping at the last.
    ///
    /// Deliberately not filtered by the navigator's Workspace scope. The
    /// notification count is of every unseen one on the server, so every one it
    /// counts has to be reachable from it.
    pub fn next_idle_unseen(&self, from: &str) -> Option<InteractionSummary> {
        let order = grouped_by_workspace(&self.items, (0..self.items.len()).collect());
        let start = order
            .iter()
            .position(|index| self.items[*index].id == from)
            .map(|position| position + 1)
            .unwrap_or_default();
        order
            .iter()
            .cycle()
            .skip(start)
            .take(order.len())
            .map(|index| &self.items[*index])
            .find(|interaction| {
                interaction.id != from && is_idle(interaction) && interaction.idle_unseen
            })
            .cloned()
    }

    /// The next Interaction that is still live — one the agent can still be
    /// handed a turn — from `from` in display order, wrapping past the end of
    /// the list so repeated presses walk the whole working set and come back.
    ///
    /// Like [`Self::next_idle_unseen`] this ignores the navigator's Workspace
    /// scope: the operator is asking for the work that is running, wherever it
    /// happens to be running.  Completed Interactions are not candidates, for
    /// the same reason the navigator hides them.
    pub fn next_live(&self, from: &str) -> Option<InteractionSummary> {
        let order = grouped_by_workspace(&self.items, (0..self.items.len()).collect());
        let start = order
            .iter()
            .position(|index| self.items[*index].id == from)
            .map(|position| position + 1)
            .unwrap_or_default();
        order
            .iter()
            .cycle()
            .skip(start)
            .take(order.len())
            .map(|index| &self.items[*index])
            .find(|interaction| {
                interaction.id != from && interaction.activity.accepting() && !interaction.completed
            })
            .cloned()
    }

    /// Move the cursor onto the next live Interaction, as a j/k move does, so
    /// the jump loads only where it comes to rest. Reveals All scope for the
    /// same reason [`Self::cursor_to_next_idle`] does: live work must not be
    /// unreachable because of the filter the navigator happens to be showing.
    pub fn cursor_to_next_live(
        &mut self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let next = self.next_live(current)?;
        if self.only_current_workspace && Some(next.workspace_id.as_str()) != workspace_id {
            self.only_current_workspace = false;
        }
        self.move_cursor_to(next.id.clone(), current);
        Some(next)
    }

    /// The next Interaction that is actually working — Running or Background —
    /// from `from` in display order, wrapping past the end of the list. Unlike
    /// [`Self::next_live`] this skips ones idle and waiting on the operator, so
    /// the step only ever lands on work in progress.
    pub fn next_active(&self, from: &str) -> Option<InteractionSummary> {
        let order = grouped_by_workspace(&self.items, (0..self.items.len()).collect());
        let start = order
            .iter()
            .position(|index| self.items[*index].id == from)
            .map(|position| position + 1)
            .unwrap_or_default();
        order
            .iter()
            .cycle()
            .skip(start)
            .take(order.len())
            .map(|index| &self.items[*index])
            .find(|interaction| {
                interaction.id != from
                    && matches!(
                        interaction.activity,
                        styra_protocol::InteractionActivity::Running
                            | styra_protocol::InteractionActivity::Background
                    )
            })
            .cloned()
    }

    /// Move the cursor onto the next actively working Interaction, as a j/k
    /// move does, so the jump loads only where it comes to rest. Reveals All
    /// scope for the same reason [`Self::cursor_to_next_live`] does.
    pub fn cursor_to_next_active(
        &mut self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let next = self.next_active(current)?;
        if self.only_current_workspace && Some(next.workspace_id.as_str()) != workspace_id {
            self.only_current_workspace = false;
        }
        self.move_cursor_to(next.id.clone(), current);
        Some(next)
    }

    /// Move the cursor onto the next Interaction that went idle unseen, as a
    /// j/k move does — so the jump loads only where it comes to rest.
    ///
    /// Landing on another Workspace's Interaction reveals All scope: a
    /// notification must not be unreachable because of the filter the navigator
    /// happens to be showing. `None` when nothing is waiting.
    pub fn cursor_to_next_idle(
        &mut self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let next = self.next_idle_unseen(current)?;
        if self.only_current_workspace && Some(next.workspace_id.as_str()) != workspace_id {
            self.only_current_workspace = false;
        }
        self.move_cursor_to(next.id.clone(), current);
        Some(next)
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

    pub fn toggle_completed(&mut self) {
        self.show_completed = !self.show_completed;
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
            // Whatever the caller does next, the cursor is not left on the
            // entry that no longer exists.
            self.rest();
            return None;
        }
        self.select_from(removed, workspace_id)
    }

    /// Select the entry that takes the place of one still listed but no longer
    /// shown — the interaction the operator just completed, with completed
    /// rows hidden. The row itself stays: it is the listing that filters it,
    /// so nothing here removes it from the list.
    pub fn select_past_hidden(
        &mut self,
        id: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let hidden = self
            .items
            .iter()
            .position(|interaction| interaction.id == id)?;
        self.select_from(hidden, workspace_id)
    }

    /// The first visible entry at or after `from`, falling back to the last
    /// one before it. If the current Workspace has no visible entries left,
    /// reveal All rather than leave the navigator with nothing to select.
    fn select_from(
        &mut self,
        from: usize,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        // The cursor is not left on an entry the navigator no longer offers.
        self.rest();
        if self.visible_indices(workspace_id).is_empty() {
            self.only_current_workspace = false;
        }
        let visible = self.visible_indices(workspace_id);
        visible
            .iter()
            .copied()
            .find(|index| *index >= from)
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
        .filter(|interaction| {
            interaction.activity.accepting() && interaction.workspace_id == workspace_id
        })
        .cloned()
        .collect::<Vec<_>>();
    sort_interactions(&mut live);
    live.into_iter().next()
}

/// `visible` re-ordered so each Workspace's entries are contiguous, in the
/// order the Workspaces themselves first appear: what [`crate::presentation::interactions`]
/// draws under its Workspace headings, and so what walking the list has to
/// follow rather than the activity-sorted item order.
fn grouped_by_workspace(interactions: &[InteractionSummary], visible: Vec<usize>) -> Vec<usize> {
    let mut ordered = Vec::with_capacity(visible.len());
    for leader in &visible {
        if ordered.contains(leader) {
            continue;
        }
        let workspace_id = &interactions[*leader].workspace_id;
        ordered.extend(
            visible
                .iter()
                .copied()
                .filter(|index| interactions[*index].workspace_id == *workspace_id),
        );
    }
    ordered
}

fn sort_interactions(interactions: &mut [InteractionSummary]) {
    interactions.sort_by_key(|interaction| match interaction.activity {
        styra_protocol::InteractionActivity::Pending => 0,
        styra_protocol::InteractionActivity::Running
        | styra_protocol::InteractionActivity::Background => 1,
        styra_protocol::InteractionActivity::Stopped => 2,
    });
}

/// `Pending` is the server summary's name for a live interaction waiting for
/// input (the TUI calls that state `Idle`).
fn is_idle(interaction: &InteractionSummary) -> bool {
    interaction.activity == styra_protocol::InteractionActivity::Pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_protocol::{DrivaOptions, InteractionActivity};

    fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            auto_retry: false,
            id: id.into(),
            name: None,
            tags: Vec::new(),
            workspace_id: "workspace".into(),
            selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            driva: DrivaOptions {
                isolation_backend: "none".into(),
                command: vec![],
                working_directory: PathBuf::from("/workspace"),
                network: false,
                base: Vec::new(),
                mounts: vec![],
                ..Default::default()
            },
            activity,
            activity_reason: None,
            activity_since_ms: 0,
            idle_unseen: false,
            uncommitted_changes: false,
            checkout: None,
            last_message: None,
            events: 0,
            completed: false,
        }
    }

    /// An interaction the operator finished with: stopped, and marked
    /// complete on the Session it serves.
    fn completed(id: &str) -> InteractionSummary {
        let mut interaction = interaction(id, InteractionActivity::Stopped);
        interaction.completed = true;
        interaction
    }

    #[test]
    fn live_interactions_open_on_the_current_session_in_status_order() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("stopped", InteractionActivity::Stopped),
                interaction("running", InteractionActivity::Running),
                interaction("idle", InteractionActivity::Pending),
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
                interaction("one", InteractionActivity::Pending),
                interaction("two", InteractionActivity::Running),
            ],
            vec![],
        );
        let next = live.next("one", Some("workspace")).unwrap();
        assert_eq!(next.id, "two");
        let mut refreshed_two = interaction("two", InteractionActivity::Pending);
        refreshed_two.last_message = Some("new response".into());
        live.refresh(vec![
            refreshed_two,
            interaction("one", InteractionActivity::Running),
        ]);

        assert_eq!(live.current("two").unwrap().id, "two");
        assert_eq!(
            live.current("two").unwrap().last_message.as_deref(),
            Some("new response")
        );
    }

    #[test]
    fn completed_interactions_are_hidden_until_toggled() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                completed("completed"),
                interaction("active", InteractionActivity::Pending),
            ],
            vec![],
        );

        assert_eq!(
            live.visible_indices(Some("workspace")),
            vec![0],
            "completed rows are hidden by default"
        );
        live.toggle_completed();
        assert_eq!(live.visible_indices(Some("workspace")), vec![0, 1]);
    }

    /// Completion is a property of the Session, cleared by the server when it
    /// is resumed, so an interaction started again arrives already not
    /// completed — and the listing shows it without anything here having to
    /// clear a mark.
    #[test]
    fn an_interaction_started_again_is_no_longer_completed() {
        let mut live = LiveInteractions::default();
        live.open(vec![completed("finished")], vec![]);
        assert!(live.visible_indices(Some("workspace")).is_empty());

        live.refresh(vec![interaction("finished", InteractionActivity::Running)]);

        assert_eq!(live.visible_indices(Some("workspace")), vec![0]);
    }

    /// Completing the interaction on screen leaves it listed — the navigator
    /// is only filtering it — so the move is onto the next visible row.
    #[test]
    fn completing_an_interaction_selects_the_next_visible_one() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                completed("finished"),
                interaction("next", InteractionActivity::Pending),
            ],
            vec![],
        );

        let next = live
            .select_past_hidden("finished", Some("workspace"))
            .unwrap();

        assert_eq!(next.id, "next");
        assert_eq!(live.items.len(), 2, "the completed row is still listed");
    }

    #[test]
    fn refresh_keeps_the_servers_unseen_idle_notification() {
        let mut live = LiveInteractions::default();
        let mut other = interaction("other", InteractionActivity::Pending);
        other.idle_unseen = true;
        live.refresh(vec![other]);

        assert_eq!(live.idle_notification_count(), 1);
        live.close();
        assert_eq!(live.idle_notification_count(), 1);
    }

    /// The jump exists to answer the footer's count, so it walks the unseen
    /// ones in display order and wraps: pressing it repeatedly visits each one
    /// rather than stopping at the last of them.
    #[test]
    fn the_idle_jump_walks_every_unseen_interaction_and_wraps() {
        let mut unseen = interaction("unseen", InteractionActivity::Pending);
        unseen.idle_unseen = true;
        let mut later = interaction("later", InteractionActivity::Pending);
        later.idle_unseen = true;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                unseen,
                interaction("busy", InteractionActivity::Running),
                later,
            ],
            vec![],
        );

        assert_eq!(live.next_idle_unseen("current").unwrap().id, "unseen");
        assert_eq!(live.next_idle_unseen("unseen").unwrap().id, "later");
        assert_eq!(live.next_idle_unseen("later").unwrap().id, "unseen");
    }

    /// The step between running interactions walks every live one — waiting on
    /// the operator or mid-turn alike — in display order, and wraps, so the
    /// operator can go round the working set with one key.
    #[test]
    fn the_live_step_walks_every_running_interaction_and_wraps() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("waiting", InteractionActivity::Pending),
                interaction("background", InteractionActivity::Background),
                completed("finished"),
                interaction("stopped", InteractionActivity::Stopped),
            ],
            vec![],
        );

        // The list is held in activity order — one waiting on the operator
        // first — and the step follows that order, wrapping at its end.
        assert_eq!(live.next_live("waiting").unwrap().id, "current");
        assert_eq!(live.next_live("current").unwrap().id, "background");
        assert_eq!(live.next_live("background").unwrap().id, "waiting");
    }

    /// The step between actively working interactions skips ones idle and
    /// waiting on the operator, unlike [`LiveInteractions::next_live`].
    #[test]
    fn the_active_step_skips_idle_interactions_and_wraps() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("waiting", InteractionActivity::Pending),
                interaction("background", InteractionActivity::Background),
                completed("finished"),
                interaction("stopped", InteractionActivity::Stopped),
            ],
            vec![],
        );

        assert_eq!(live.next_active("current").unwrap().id, "background");
        assert_eq!(live.next_active("background").unwrap().id, "current");
    }

    /// With nothing else actively working there is nowhere to step to: an
    /// idle interaction is not a destination even though it is still live.
    #[test]
    fn the_active_step_has_nowhere_to_go_without_another_working_interaction() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("waiting", InteractionActivity::Pending),
                interaction("stopped", InteractionActivity::Stopped),
            ],
            vec![],
        );

        assert!(live.next_active("current").is_none());
    }

    /// With nothing else running there is nowhere to step to: the interaction
    /// on screen is not a destination, and a stopped one is not either.
    #[test]
    fn the_live_step_has_nowhere_to_go_without_another_running_interaction() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("stopped", InteractionActivity::Stopped),
                completed("finished"),
            ],
            vec![],
        );

        assert!(live.next_live("current").is_none());
    }

    /// Running work elsewhere is still running work, so the step reveals All
    /// rather than refusing to leave the Workspace being shown.
    #[test]
    fn the_live_step_reveals_all_workspaces_to_reach_a_running_interaction() {
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Running);
        elsewhere.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                elsewhere,
            ],
            vec![],
        );
        live.toggle_workspace_scope();

        let next = live
            .cursor_to_next_live("current", Some("workspace"))
            .unwrap();

        assert_eq!(next.id, "elsewhere");
        assert!(!live.only_current_workspace);
        // Moved like a j/k step, so the load waits for the cursor to rest.
        assert_eq!(live.cursor("current"), "elsewhere");
    }

    /// An idle Interaction a client has already been shown is not what the
    /// jump is for, and neither is one that stopped rather than went idle.
    #[test]
    fn the_idle_jump_has_nowhere_to_go_without_an_unseen_interaction() {
        let mut stopped = interaction("stopped", InteractionActivity::Stopped);
        stopped.idle_unseen = true;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("seen", InteractionActivity::Pending),
                stopped,
            ],
            vec![],
        );

        assert!(live.next_idle_unseen("current").is_none());
    }

    /// The count the jump answers is of every unseen Interaction on the
    /// server, so the Workspace filter cannot be allowed to hide one of them:
    /// the jump reveals All rather than refusing to move.
    #[test]
    fn the_idle_jump_reveals_all_workspaces_to_reach_an_unseen_interaction() {
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other-workspace".into();
        elsewhere.idle_unseen = true;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                elsewhere,
            ],
            vec![],
        );
        live.toggle_workspace_scope();

        let next = live
            .cursor_to_next_idle("current", Some("workspace"))
            .unwrap();

        assert_eq!(next.id, "elsewhere");
        assert!(!live.only_current_workspace);
        // Moved like a j/k step, so the interaction is loaded once the cursor
        // has come to rest rather than from inside the key handler.
        assert_eq!(live.cursor("current"), "elsewhere");
        assert_eq!(
            live.pending("current")
                .map(|interaction| interaction.id.as_str()),
            Some("elsewhere")
        );
    }

    /// Loading an Interaction replaces the whole screen, so a cursor crossing
    /// the list must not ask for one row's load per row it passes over.
    #[test]
    fn a_moving_cursor_defers_its_load_until_it_comes_to_rest() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", InteractionActivity::Pending),
                interaction("two", InteractionActivity::Pending),
                interaction("three", InteractionActivity::Pending),
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
                interaction("one", InteractionActivity::Pending),
                interaction("two", InteractionActivity::Pending),
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
                interaction("one", InteractionActivity::Pending),
                interaction("two", InteractionActivity::Pending),
            ],
            vec![],
        );
        live.cursor_next("one", Some("workspace"));

        live.refresh(vec![interaction("one", InteractionActivity::Pending)]);

        assert_eq!(live.cursor("one"), "one");
        assert!(live.pending("one").is_none());
    }

    #[test]
    fn workspace_scope_filters_navigation_and_can_return_to_all() {
        let mut other = interaction("other", InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Pending),
                other,
                interaction("next", InteractionActivity::Running),
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
        let mut other_pending = interaction("other-pending", InteractionActivity::Pending);
        other_pending.workspace_id = "other-workspace".into();
        let mut other_running = interaction("other-running", InteractionActivity::Running);
        other_running.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("pending", InteractionActivity::Pending),
                other_pending,
                interaction("running", InteractionActivity::Running),
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
        let mut other_pending = interaction("other-pending", InteractionActivity::Pending);
        other_pending.workspace_id = "other-workspace".into();
        let mut other_running = interaction("other-running", InteractionActivity::Running);
        other_running.workspace_id = "other-workspace".into();
        let mut third = interaction("third", InteractionActivity::Running);
        third.workspace_id = "third-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("pending", InteractionActivity::Pending),
                other_pending,
                interaction("running", InteractionActivity::Running),
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
        let mut other = interaction("other-pending", InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut third = interaction("third", InteractionActivity::Pending);
        third.workspace_id = "third-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("pending", InteractionActivity::Pending),
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

    /// A stopped Interaction stays in its own Workspace's group, in item
    /// order: its row says that it stopped, so it needs no place of its own.
    #[test]
    fn stopped_interactions_stay_in_their_workspace_group() {
        let mut other_stopped = interaction("other-stopped", InteractionActivity::Stopped);
        other_stopped.workspace_id = "other-workspace".into();
        let mut other_running = interaction("other-running", InteractionActivity::Running);
        other_running.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("pending", InteractionActivity::Pending),
                other_stopped,
                interaction("stopped", InteractionActivity::Stopped),
                other_running,
            ],
            vec![],
        );

        let ordered = |live: &LiveInteractions| {
            live.display_indices(Some("workspace"))
                .into_iter()
                .map(|index| live.items[index].id.to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ordered(&live),
            ["pending", "stopped", "other-running", "other-stopped"]
        );

        // And in Workspace scope, where only this Workspace's entries are
        // listed, in item order.
        live.toggle_workspace_scope();
        assert_eq!(ordered(&live), ["pending", "stopped"]);
    }

    #[test]
    fn workspace_jumps_do_nothing_in_workspace_scope() {
        let mut other = interaction("other", InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Pending),
                other,
                interaction("next", InteractionActivity::Running),
            ],
            vec![],
        );
        live.toggle_workspace_scope();

        assert!(live.next_workspace("current", Some("workspace")).is_none());
        assert!(live.previous_workspace("next", Some("workspace")).is_none());
    }

    #[test]
    fn first_live_in_workspace_prefers_the_one_waiting_on_the_operator() {
        let mut other = interaction("other", InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let interactions = vec![
            other,
            interaction("stopped", InteractionActivity::Stopped),
            interaction("running", InteractionActivity::Running),
            interaction("idle", InteractionActivity::Pending),
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
            interaction("stopped", InteractionActivity::Stopped),
            interaction("also-stopped", InteractionActivity::Stopped),
        ];

        assert!(first_live_in_workspace(&interactions, "workspace").is_none());
        assert!(first_live_in_workspace(&interactions, "unknown").is_none());
    }

    #[test]
    fn deleting_an_interaction_selects_the_entry_that_replaces_it() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("one", InteractionActivity::Pending),
                interaction("two", InteractionActivity::Running),
                interaction("stopped", InteractionActivity::Stopped),
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
        let mut other = interaction("other", InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![interaction("current", InteractionActivity::Stopped), other],
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
