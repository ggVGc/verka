//! The live Interactions navigator: the list of Interactions the server is
//! running, and the operator's place in it.
//!
//! Held apart from [`App`](crate::app::App) because none of it depends on the
//! Interaction currently on screen. [`crate::presentation::interactions`] renders it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use styra_protocol::{InteractionActivity, InteractionQuery, InteractionSummary, WorkspaceSummary};
use styra_ui::query::Edit;

/// How long the cursor must rest on an entry before that Interaction is
/// loaded, matching the Session and Workspace pickers' settle: short enough to
/// feel immediate once the cursor stops, long enough that walking the list
/// costs no loads at all.
const LOAD_SETTLE: Duration = Duration::from_millis(120);

/// How long a row stays lit after something new happens on it: long enough
/// to catch the eye at the edge of vision, short enough that a busy fleet
/// does not leave the whole list lit.
const FLASH: Duration = Duration::from_millis(400);

/// The live interactions navigator embedded above the main event list.
///
/// It is deliberately only navigation state: the selected interaction's full
/// history lives in the ordinary [`App`](crate::app::App) fields, so there is
/// no second preview cache or interaction screen to keep in sync.
#[derive(Clone, Debug, Default)]
pub struct LiveInteractions {
    pub open: bool,
    /// Global interaction scope shared by the live list, overview and n/N.
    /// Carried with operator state when the current interaction changes.
    pub only_current_workspace: bool,
    /// Completed interactions stay in the server's list, but are normally out
    /// of the navigator until the operator asks to see them. Hiding them is
    /// the listing's own business: the server keeps completion as the reason
    /// an interaction is stopped, and says so about every row.
    pub show_completed: bool,
    /// Lists only interactions whose agent still takes messages. Stopped rows
    /// include every completed one, so this hides those whatever
    /// `show_completed` says.
    pub hide_stopped: bool,
    /// Shows the line quoting each row's last response. Off by default so
    /// more rows fit in the list.
    pub show_responses: bool,
    pub items: Vec<InteractionSummary>,
    pub workspaces: Vec<WorkspaceSummary>,
    /// The one client-side index of operator-facing Session names. Interaction
    /// rows and the current screen both resolve through this map instead of
    /// retaining independent copies that a rename has to keep synchronized.
    names: HashMap<String, Option<String>>,
    /// Persisted ancestry, retained when a Session leaves the live roster.
    branch_origins: HashMap<
        String,
        (
            String,
            styra_protocol::agent::Provider,
            styra_protocol::agent::Provider,
        ),
    >,
    /// Where the cursor is while that is not the Interaction on screen.
    /// `None` — the resting state — means the cursor is on the current
    /// Interaction, so there is no move outstanding.
    cursor: Option<String>,
    /// Set while the cursor has moved but the Interaction under it has not
    /// been loaded yet. Loading is a blocking round-trip for a whole screen,
    /// so holding `j` must not queue one load per row it passes over; the load
    /// waits for the cursor to settle.
    settle_from: Option<Instant>,
    /// The `/` filter: only rows whose labels contain it, case-insensitively,
    /// are listed. `None` lists every row the scope and completion settings
    /// allow.
    filter: Option<String>,
    /// The `ctrl-t` tag filter. Plain entries are required and entries with a
    /// leading `-` are excluded. Chosen from a list rather than typed, it is a
    /// setting like the scope and stays when the list is reopened.
    tag_filter: Vec<String>,
    /// Set while the filter is being typed, when every printable key is part
    /// of the term rather than a command on the list.
    typing_filter: bool,
    /// While the Workspace jump is open over the navigator, the Workspaces its
    /// query leaves: only their rows are listed. `None` otherwise.
    pub jump_workspaces: Option<Vec<String>>,
    /// Whether a listing has been taken yet. The first one is what was already
    /// running when this client arrived, so nothing in it is newly arrived.
    listed: bool,
    /// An Interaction that arrived asking to be focused — see
    /// [`InteractionSummary::focus_requested`] — and has not been acted on.
    focus_claim: Option<InteractionSummary>,
    /// When each row last had something new happen on it — see
    /// [`has_news`] — for as long as it is drawn lit.
    flashed: HashMap<String, Instant>,
    /// The first line the navigator showed when last drawn. The cursor moves
    /// freely within the view and only scrolls it near an edge, so where the
    /// view sits has to be remembered between frames. `None` starts a fresh
    /// client at the bottom before keeping the current interaction in view.
    pub scroll_offset: Option<usize>,
}

/// The event list's tally of listed Interactions by activity, rendered on the
/// pane's bottom border.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityCounts {
    pub running: usize,
    pub idle: usize,
    pub stopped: usize,
}

impl LiveInteractions {
    /// What to ask the server for to fill this navigator with `current` on
    /// screen. Finished rows are left on the server unless the operator asked
    /// to see them: they are most of what a long-lived server lists and none
    /// of what the navigator shows. `current` is kept whatever its state,
    /// since the screen still describes it — its checkout, its tags — once it
    /// is finished. Stopped rows are still fetched, so `hide_stopped` stays a
    /// filter over a listing already held.
    pub fn query(&self, current: &str) -> InteractionQuery {
        InteractionQuery {
            skip_done: !self.show_completed,
            keep: (!current.is_empty())
                .then(|| current.to_owned())
                .into_iter()
                .collect(),
            ..InteractionQuery::default()
        }
    }

    pub fn open(&mut self, items: Vec<InteractionSummary>, workspaces: Vec<WorkspaceSummary>) {
        self.refresh(items);
        self.show(workspaces);
    }

    /// Open the navigator over the listing already held, which the event loop
    /// keeps fresh on its timer: asking the server again would only repeat
    /// the poll at the cost of a blocking round-trip.
    pub fn show(&mut self, workspaces: Vec<WorkspaceSummary>) {
        self.workspaces = workspaces;
        self.open = true;
        // A filter is a search for something particular; opening the list
        // again is a fresh look at all of it.
        self.filter = None;
        self.typing_filter = false;
    }

    /// Incorporate a periodic server snapshot. Idle acknowledgement belongs to
    /// the server: listing a row cannot accidentally count as seeing it.
    ///
    /// Reports whether the snapshot differed from the one it replaced. The
    /// fleet is listed several times a second and is usually exactly as it
    /// was, so this is what keeps the poll from counting as news.
    pub fn refresh(&mut self, mut items: Vec<InteractionSummary>) -> bool {
        self.note_news(&items);
        self.note_arrivals(&items);
        let names_changed = self.take_names(&mut items);
        sort_interactions(&mut items);
        let changed = self.items != items || names_changed;
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
        changed
    }

    /// Whether a listing has been taken yet, so the one held can stand in for
    /// asking the server.
    pub fn listed(&self) -> bool {
        self.listed
    }

    /// The listing as the server gave it, names included, for handing to a
    /// screen that does not share this one's state — a rebuilt App, a picker —
    /// instead of having it ask the server for the same answer.
    pub fn snapshot(&self) -> Vec<InteractionSummary> {
        self.items
            .iter()
            .map(|interaction| InteractionSummary {
                name: self.name(&interaction.id).map(str::to_owned),
                ..interaction.clone()
            })
            .collect()
    }

    /// Hold on to an Interaction in `items` that was not in the last listing
    /// and asks to be focused. Only arrival counts: the request stays on the
    /// row for as long as it is listed, and answering it on every refresh
    /// would pin the operator to it.
    fn note_arrivals(&mut self, items: &[InteractionSummary]) {
        if self.listed {
            if let Some(arrived) = items.iter().rev().find(|interaction| {
                interaction.focus_requested
                    && !self.items.iter().any(|listed| listed.id == interaction.id)
            }) {
                self.focus_claim = Some(arrived.clone());
            }
        }
        self.listed = true;
    }

    /// Light the rows in `items` that have something new to show since the
    /// last listing. The first listing is the fleet as this client found it,
    /// so nothing in it is news.
    fn note_news(&mut self, items: &[InteractionSummary]) {
        if !self.listed {
            return;
        }
        let now = Instant::now();
        for interaction in items {
            let previous = self.items.iter().find(|listed| listed.id == interaction.id);
            if previous.is_none_or(|previous| has_news(previous, interaction)) {
                self.flashed.insert(interaction.id.clone(), now);
            }
        }
    }

    /// Whether the row for `id` is still lit by something new on it.
    pub fn flashing(&self, id: &str) -> bool {
        self.flashed.get(id).is_some_and(|at| at.elapsed() < FLASH)
    }

    /// Put out the rows whose flash has run its course, reporting whether any
    /// did, so the frame that draws them unlit gets painted.
    pub fn expire_flashes(&mut self) -> bool {
        let before = self.flashed.len();
        self.flashed.retain(|_, at| at.elapsed() < FLASH);
        self.flashed.len() != before
    }

    /// The Interaction that most recently arrived asking to be focused, once.
    pub fn take_focus_claim(&mut self) -> Option<InteractionSummary> {
        self.focus_claim.take()
    }

    /// Record the name carried by a create, resume, load, or stored-session
    /// response before that Session necessarily appears in the live listing.
    pub fn note_name(&mut self, id: impl Into<String>, name: Option<String>) {
        self.names.insert(id.into(), name);
    }

    /// Cache the immutable origin and provider for branch-marker rendering.
    pub fn note_origin(
        &mut self,
        id: &str,
        provider: styra_protocol::agent::Provider,
        origin: Option<&styra_protocol::SessionOrigin>,
    ) {
        if let Some(origin) = origin {
            self.branch_origins.insert(
                id.to_owned(),
                (origin.session_id.clone(), origin.provider, provider),
            );
        }
    }

    pub fn provider_switch_from(
        &self,
        source: &str,
        destination: &str,
    ) -> Option<(
        styra_protocol::agent::Provider,
        styra_protocol::agent::Provider,
    )> {
        let mut destination = destination;
        // A copied marker can belong to an ancestor branch. Bound the walk
        // so malformed cyclic origins cannot keep rendering forever.
        for _ in 0..self.branch_origins.len() {
            let (parent, from, to) = self.branch_origins.get(destination)?;
            if parent == source {
                return (from != to).then_some((*from, *to));
            }
            destination = parent;
        }
        None
    }

    pub fn provider_switch_to(
        &self,
        destination: &str,
    ) -> Option<(
        styra_protocol::agent::Provider,
        styra_protocol::agent::Provider,
    )> {
        let (_, from, to) = self.branch_origins.get(destination)?;
        (from != to).then_some((*from, *to))
    }

    /// The current operator-facing name for `id`, shared by every TUI surface.
    pub fn name(&self, id: &str) -> Option<&str> {
        self.names.get(id).and_then(Option::as_deref)
    }

    /// Preserve names learned while constructing a newly loaded screen when
    /// the operator-owned navigator state is carried onto it.
    pub fn adopt_names_from(&mut self, newer: &Self) {
        self.names.extend(newer.names.clone());
        self.branch_origins.extend(newer.branch_origins.clone());
    }

    fn take_names(&mut self, items: &mut [InteractionSummary]) -> bool {
        let mut changed = false;
        for interaction in items {
            self.note_origin(
                &interaction.id,
                interaction.selection.provider,
                interaction.origin.as_ref(),
            );
            let name = interaction.name.take();
            changed |= self
                .names
                .insert(interaction.id.clone(), name.clone())
                .as_ref()
                != Some(&name);
        }
        changed
    }

    /// Number of idle interactions that have not actually been focused since
    /// becoming idle. The server owns this acknowledgement state.
    pub fn idle_notification_count(&self) -> usize {
        self.items
            .iter()
            .filter(|interaction| is_idle(interaction) && interaction.idle_unseen)
            .count()
    }

    /// How the listed Interactions divide between running, idle and stopped:
    /// the standing tally of the whole fleet, not just the unseen ones
    /// [`Self::idle_notification_count`] counts. `Background` is work in
    /// flight, so it counts as running.
    ///
    /// Completed Interactions are left out entirely. Completion is the reason
    /// the server stopped them, so counting them would fold work the operator
    /// has already signed off on into the stopped number — which is there to
    /// show what stopped without being finished.
    pub fn activity_counts(&self) -> ActivityCounts {
        let mut counts = ActivityCounts::default();
        for interaction in self
            .items
            .iter()
            .filter(|interaction| !interaction.completed.is_done())
        {
            match interaction.activity {
                styra_protocol::InteractionActivity::Pending => counts.idle += 1,
                styra_protocol::InteractionActivity::Running
                | styra_protocol::InteractionActivity::Background => counts.running += 1,
                styra_protocol::InteractionActivity::Stopped => counts.stopped += 1,
            }
        }
        counts
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn current(&self, current: &str) -> Option<&InteractionSummary> {
        self.items
            .iter()
            .find(|interaction| interaction.id == current)
    }

    /// The distinct tags carried by the server's current live-interaction
    /// snapshot. The filter picker deliberately uses this instead of the
    /// server-wide tag catalog, which also contains tags from stored Sessions.
    pub fn live_tags(&self) -> Vec<String> {
        let mut tags = self
            .items
            .iter()
            .flat_map(|interaction| interaction.tags.iter().cloned())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        tags
    }

    fn in_workspace_scope(
        &self,
        interaction: &InteractionSummary,
        workspace_id: Option<&str>,
    ) -> bool {
        !self.only_current_workspace
            || workspace_id.is_some_and(|id| interaction.workspace_id == id)
    }

    /// The rows the navigator lists, in item order. `current`, the
    /// Interaction on screen, is listed whatever its state: completing,
    /// integrating or stopping it must not take it, and with it its place
    /// among its branches, out of the list while it is still being looked at.
    pub fn visible_indices(&self, current: &str, workspace_id: Option<&str>) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, interaction)| {
                (self.in_workspace_scope(interaction, workspace_id)
                    && (self.lists(interaction)
                        || interaction.id == current && self.matches_tag_filter(interaction))
                    && self.matches_typed_filter(interaction)
                    && self
                        .jump_workspaces
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&interaction.workspace_id)))
                .then_some(index)
            })
            .collect()
    }

    /// Whether the navigator lists `interaction` once it is opened afresh in
    /// All scope: the completion, stopped and tag filters stand, while the
    /// typed filter is one that opening clears.
    pub fn lists(&self, interaction: &InteractionSummary) -> bool {
        (self.show_completed || !interaction.completed.is_done())
            && (!self.hide_stopped || interaction.activity.accepting())
            && self.matches_tag_filter(interaction)
    }

    /// The Interactions the overview tiles, in the order it lays them out:
    /// every one whose agent still takes turns — working or waiting on the
    /// operator — working ones first, then newly idle ones, then the rest of
    /// the idle ones, each grouped by Workspace as the navigator's All scope
    /// is.
    ///
    /// Shares the global Workspace scope with the navigator and n/N, while
    /// ignoring the navigator's search, tag and completion filters. Completed
    /// and stopped interactions are never tiled.
    pub fn overview_indices(&self, workspace_id: Option<&str>) -> Vec<usize> {
        let active = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, interaction)| {
                interaction.activity.accepting()
                    && !interaction.completed.is_done()
                    && self.in_workspace_scope(interaction, workspace_id)
            })
            .map(|(index, _)| index)
            .collect();
        let mut indices = grouped_by_workspace(&self.items, &self.workspaces, active);
        indices.sort_by_key(|index| {
            let interaction = &self.items[*index];
            if matches!(
                interaction.activity,
                InteractionActivity::Running | InteractionActivity::Background
            ) {
                0
            } else if is_idle(interaction) && interaction.idle_unseen {
                1
            } else {
                2
            }
        });
        indices
    }

    /// The visible indices in the order [`crate::presentation::interactions`] draws
    /// them: in All scope the entries are grouped under their Workspace
    /// heading, and in either scope those sharing a directory under its, so
    /// j/k has to walk that order rather than the raw item order.
    ///
    /// A stopped entry stays where it belongs, in its own Workspace's group and
    /// in item order: its row says that it stopped and why, which is enough to
    /// tell it apart from the work that can still be talked to.
    ///
    /// Within that, a branch follows the entry it was branched from, as the
    /// Session picker nests it: see [`Self::display_rows`].
    pub fn display_indices(&self, current: &str, workspace_id: Option<&str>) -> Vec<usize> {
        self.display_rows(current, workspace_id)
            .into_iter()
            .map(|row| row.index)
            .collect()
    }

    /// [`Self::display_indices`], each placed as the navigator draws it — see
    /// [`DisplayRow`]. A branch whose source is not shown — closed, hidden as
    /// completed, or out of scope — stands at the root of its checkout.
    pub fn display_rows(&self, current: &str, workspace_id: Option<&str>) -> Vec<DisplayRow> {
        display_tree(
            &self.items,
            &self.workspaces,
            self.visible_indices(current, workspace_id),
        )
    }

    /// Every Interaction the `n`/`N` steps may visit, in display order. In
    /// Workspace scope that is only the current Workspace's: the operator has
    /// narrowed the list to it, and stepping out of it would undo that. In All
    /// scope it is every one, grouped as the list draws them.
    ///
    /// Unlike [`Self::display_indices`] this ignores the completion setting
    /// and the `/` filter: each step picks its own candidates, and one it lands
    /// on behind the filter is revealed rather than skipped.
    fn step_order(&self, workspace_id: Option<&str>) -> Vec<usize> {
        let candidates = (0..self.items.len())
            .filter(|index| self.in_workspace_scope(&self.items[*index], workspace_id))
            .collect();
        display_tree(&self.items, &self.workspaces, candidates)
            .into_iter()
            .map(|row| row.index)
            .collect()
    }

    /// The first Interaction in `order` after `from` that `candidate` accepts,
    /// wrapping past the end, so repeated presses walk every one of them and
    /// come back rather than stopping at the last. `from` itself never is.
    fn step_from(
        &self,
        order: &[usize],
        from: &str,
        candidate: impl Fn(&InteractionSummary) -> bool,
    ) -> Option<InteractionSummary> {
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
            .find(|interaction| interaction.id != from && candidate(interaction))
            .cloned()
    }

    /// The next Interaction that went idle unseen, from `from` onward in
    /// display order, wrapping past the end of the list.
    ///
    /// In All scope this reaches every unseen one on the server, which is what
    /// the notification count counts; in Workspace scope only those in the
    /// current Workspace — see [`Self::step_order`].
    pub fn next_idle_unseen(
        &self,
        from: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        self.step_from(&self.step_order(workspace_id), from, |interaction| {
            is_idle(interaction) && interaction.idle_unseen
        })
    }

    /// The next Interaction that is still live — one the agent can still be
    /// handed a turn — from `from` in display order, wrapping past the end of
    /// the list so repeated presses walk the whole working set and come back.
    ///
    /// Scoped as [`Self::next_idle_unseen`] is. Completed Interactions are not
    /// candidates, for the same reason the navigator hides them.
    pub fn next_live(&self, from: &str, workspace_id: Option<&str>) -> Option<InteractionSummary> {
        self.step_from(&self.step_order(workspace_id), from, |interaction| {
            interaction.activity.accepting() && !interaction.completed.is_done()
        })
    }

    /// The next destination for the global `n` shortcut. An interaction that
    /// became idle while unseen takes precedence over the ordinary live-work
    /// walk; absent one, the walk retains its display-order behavior.
    pub fn next_attention_or_live(
        &self,
        from: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        self.next_idle_unseen(from, workspace_id)
            .or_else(|| self.next_live(from, workspace_id))
    }

    pub(crate) fn next_attention_or_live_in_order(
        &self,
        order: &[usize],
        from: &str,
    ) -> Option<InteractionSummary> {
        self.step_from(order, from, |interaction| {
            is_idle(interaction) && interaction.idle_unseen
        })
        .or_else(|| {
            self.step_from(order, from, |interaction| {
                interaction.activity.accepting() && !interaction.completed.is_done()
            })
        })
    }

    /// The next Interaction that is actually working — Running or Background —
    /// from `from` in display order, wrapping past the end of the list. Unlike
    /// [`Self::next_live`] this skips ones idle and waiting on the operator, so
    /// the step only ever lands on work in progress. Scoped as
    /// [`Self::next_live`] is.
    pub fn next_active(
        &self,
        from: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        self.step_active(from, workspace_id, false)
    }

    /// [`Self::next_active`] walking the other way: the previous working
    /// Interaction in display order, wrapping past the start of the list.
    pub fn previous_active(
        &self,
        from: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        self.step_active(from, workspace_id, true)
    }

    fn step_active(
        &self,
        from: &str,
        workspace_id: Option<&str>,
        backwards: bool,
    ) -> Option<InteractionSummary> {
        let mut order = self.step_order(workspace_id);
        if backwards {
            order.reverse();
        }
        self.step_from(&order, from, |interaction| {
            matches!(
                interaction.activity,
                styra_protocol::InteractionActivity::Running
                    | styra_protocol::InteractionActivity::Background
            )
        })
    }

    /// Move the cursor onto the next actively working Interaction, as a j/k
    /// move does, so the jump loads only where it comes to rest. The step stays
    /// within the navigator's scope, but reveals one hidden behind the `/`
    /// filter: work in progress must not be unreachable because of a search.
    ///
    /// The step starts from the cursor, so presses quicker than the settle
    /// keep walking rather than landing on the same row each time.
    pub fn cursor_to_next_active(
        &mut self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let next = self.next_active(self.cursor(current), workspace_id)?;
        self.cursor_to(next, current)
    }

    /// [`Self::cursor_to_next_active`] walking the other way.
    pub fn cursor_to_previous_active(
        &mut self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let previous = self.previous_active(self.cursor(current), workspace_id)?;
        self.cursor_to(previous, current)
    }

    fn cursor_to(
        &mut self,
        target: InteractionSummary,
        current: &str,
    ) -> Option<InteractionSummary> {
        self.reveal_past_filter(&target);
        self.move_cursor_to(target.id.clone(), current);
        Some(target)
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
        if let Some(next) = self.next(&from, current, workspace_id) {
            self.move_cursor_to(next.id, current);
        }
    }

    pub fn cursor_previous(&mut self, current: &str, workspace_id: Option<&str>) {
        let from = self.cursor(current).to_owned();
        if let Some(previous) = self.previous(&from, current, workspace_id) {
            self.move_cursor_to(previous.id, current);
        }
    }

    /// Page through the displayed interactions, keeping the usual cursor
    /// settle and stopping at either end of the filtered list.
    pub fn cursor_page(&mut self, current: &str, workspace_id: Option<&str>, down: bool) {
        let visible = self.display_indices(current, workspace_id);
        let position = visible
            .iter()
            .position(|index| self.items[*index].id == self.cursor(current));
        let target = match position {
            Some(position) if down => position.saturating_add(styra_ui::fuzzy_list::PAGE),
            Some(position) => position.saturating_sub(styra_ui::fuzzy_list::PAGE),
            None => 0,
        }
        .min(visible.len().saturating_sub(1));
        if let Some(index) = visible.get(target) {
            self.move_cursor_to(self.items[*index].id.clone(), current);
        }
    }

    pub fn cursor_first(&mut self, current: &str, workspace_id: Option<&str>) {
        if let Some(index) = self.display_indices(current, workspace_id).first() {
            self.move_cursor_to(self.items[*index].id.clone(), current);
        }
    }

    pub fn cursor_last(&mut self, current: &str, workspace_id: Option<&str>) {
        if let Some(index) = self.display_indices(current, workspace_id).last() {
            self.move_cursor_to(self.items[*index].id.clone(), current);
        }
    }

    /// The ctrl-j/ctrl-k group skips move the cursor exactly as j/k do, so
    /// crossing several Workspaces costs the one load the cursor comes to rest
    /// on rather than one per group passed through.
    pub fn cursor_next_workspace(&mut self, current: &str, workspace_id: Option<&str>) {
        let from = self.cursor(current).to_owned();
        if let Some(next) = self.next_workspace(&from, current, workspace_id) {
            self.move_cursor_to(next.id, current);
        }
    }

    pub fn cursor_previous_workspace(&mut self, current: &str, workspace_id: Option<&str>) {
        let from = self.cursor(current).to_owned();
        if let Some(previous) = self.previous_workspace(&from, current, workspace_id) {
            self.move_cursor_to(previous.id, current);
        }
    }

    /// Move the cursor to the first entry of `target`'s Workspace group, for
    /// the Workspace jump. A Workspace scope that leaves `target` out is
    /// widened to All first. Reports whether `target` had an entry to land on.
    pub fn cursor_to_workspace(
        &mut self,
        target: &str,
        current: &str,
        workspace_id: Option<&str>,
    ) -> bool {
        if workspace_id != Some(target) {
            self.only_current_workspace = false;
        }
        let Some(index) = self
            .display_indices(current, workspace_id)
            .into_iter()
            .find(|index| self.items[*index].workspace_id == target)
        else {
            return false;
        };
        self.move_cursor_to(self.items[index].id.clone(), current);
        true
    }

    /// A cursor that lands back on the Interaction already on screen — by
    /// walking off the end of the list, or straight back to where it started —
    /// has nothing to load, so it cancels the move rather than timing one out.
    pub(crate) fn move_cursor_to(&mut self, id: String, current: &str) {
        if id == current {
            self.rest();
            return;
        }
        self.cursor = Some(id);
        self.settle_from = Some(Instant::now());
    }

    fn next(
        &self,
        from: &str,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let visible = self.display_indices(current, workspace_id);
        let index = visible
            .iter()
            .position(|index| self.items[*index].id == from)
            .map(|position| (position + 1).min(visible.len().saturating_sub(1)))
            // Match the bottom-aligned viewport when nothing is selected.
            .unwrap_or_else(|| visible.len().saturating_sub(1));
        visible
            .get(index)
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    fn previous(
        &self,
        from: &str,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let visible = self.display_indices(current, workspace_id);
        let position = visible
            .iter()
            .position(|index| self.items[*index].id == from)
            .map(|position| position.saturating_sub(1))
            .unwrap_or_else(|| visible.len().saturating_sub(1));
        visible
            .get(position)
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    /// The first entry of the Workspace group after the current one, for the
    /// ctrl-j jump. Only meaningful in All scope, where the display is grouped
    /// under Workspace headings; in Workspace scope there is a single group and
    /// no jump to make.
    fn next_workspace(
        &self,
        from: &str,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let leaders = self.workspace_leaders(current, workspace_id);
        let group = self.group_of(from, current, workspace_id)?;
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
        from: &str,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let leaders = self.workspace_leaders(current, workspace_id);
        let group = self.group_of(from, current, workspace_id)?;
        let leader = self.items.get(*leaders.get(group)?)?;
        if leader.id != from {
            return Some(leader.clone());
        }
        leaders
            .get(group.checked_sub(1)?)
            .and_then(|index| self.items.get(*index))
            .cloned()
    }

    /// The first visible entry of each Workspace group, in display order.
    /// Empty in Workspace scope: the entries are not grouped there.
    fn workspace_leaders(&self, current: &str, workspace_id: Option<&str>) -> Vec<usize> {
        if self.only_current_workspace {
            return Vec::new();
        }

        let mut leaders: Vec<usize> = Vec::new();
        for index in self.display_indices(current, workspace_id) {
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

    /// Which Workspace group `from` sits in, counted over
    /// [`Self::workspace_leaders`].
    fn group_of(&self, from: &str, current: &str, workspace_id: Option<&str>) -> Option<usize> {
        let from = self
            .items
            .iter()
            .find(|interaction| interaction.id == from)?;
        self.workspace_leaders(current, workspace_id)
            .iter()
            .position(|leader| self.items[*leader].workspace_id == from.workspace_id)
    }

    /// Start typing the `/` filter, afresh.
    pub fn start_filter(&mut self) {
        self.filter = Some(String::new());
        self.typing_filter = true;
    }

    pub fn typing_filter(&self) -> bool {
        self.typing_filter
    }

    /// The filter in force, if anything has been typed into it.
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref().filter(|filter| !filter.is_empty())
    }

    /// Make a change typed into the filter, as every search field takes it
    /// (see [`styra_ui::query`]).
    pub fn type_filter(&mut self, edit: Edit, current: &str, workspace_id: Option<&str>) {
        edit.apply(self.filter.get_or_insert_with(String::new));
        self.follow_filter(current, workspace_id);
    }

    /// Stop typing and keep the filter, so the narrowed list can be walked
    /// with the list's own keys.
    pub fn finish_filter(&mut self) {
        self.typing_filter = false;
        if self.filter().is_none() {
            self.filter = None;
        }
    }

    pub fn clear_filter(&mut self) {
        self.filter = None;
        self.typing_filter = false;
    }

    /// The tag conditions in force. Plain tags are required and `-tag`
    /// excludes rows carrying `tag`; an empty list admits every row.
    pub fn tag_filter(&self) -> &[String] {
        &self.tag_filter
    }

    pub fn set_tag_filter(&mut self, tags: Vec<String>, current: &str, workspace_id: Option<&str>) {
        self.tag_filter = tags;
        self.follow_filter(current, workspace_id);
    }

    /// Keep the cursor on a listed row as the filter narrows: once the row
    /// under it is filtered out, it moves to the first that still matches,
    /// and loads there once it rests like any other move.
    fn follow_filter(&mut self, current: &str, workspace_id: Option<&str>) {
        let shown = self.display_indices(current, workspace_id);
        let cursor = self.cursor(current);
        if shown.iter().any(|index| self.items[*index].id == cursor) {
            return;
        }
        if let Some(first) = shown.first().map(|index| self.items[*index].id.clone()) {
            self.move_cursor_to(first, current);
        }
    }

    /// The jumps to live and working Interactions reach past the filter as
    /// they do past the Workspace scope: drop it when it hides their target.
    fn reveal_past_filter(&mut self, target: &InteractionSummary) {
        if !self.matches_filter(target) {
            self.clear_filter();
            self.tag_filter.clear();
        }
    }

    /// Whether `interaction` passes both filters: it carries every tag of the
    /// tag filter, and its name (or short id), a tag, its branch, its
    /// provider, or its Workspace's name contains the typed term. The last
    /// message is left out — it changes as the agent talks, and rows would
    /// come and go under the cursor with it.
    fn matches_filter(&self, interaction: &InteractionSummary) -> bool {
        self.matches_tag_filter(interaction) && self.matches_typed_filter(interaction)
    }

    fn matches_tag_filter(&self, interaction: &InteractionSummary) -> bool {
        self.tag_filter.iter().all(|filter| {
            if let Some(tag) = filter.strip_prefix('-') {
                !interaction.tags.iter().any(|candidate| candidate == tag)
            } else {
                interaction.tags.contains(filter)
            }
        })
    }

    fn matches_typed_filter(&self, interaction: &InteractionSummary) -> bool {
        let Some(filter) = self.filter() else {
            return true;
        };
        let filter = filter.to_lowercase();
        let name = self
            .name(&interaction.id)
            .unwrap_or_else(|| styra_ui::picker::short_id(&interaction.id));
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == interaction.workspace_id)
            .map(crate::workspace::display_name);
        std::iter::once(name)
            .chain(interaction.tags.iter().map(String::as_str))
            .chain(
                interaction
                    .checkout
                    .as_ref()
                    .and_then(|checkout| checkout.branch.as_deref()),
            )
            .chain(std::iter::once(interaction.selection.provider.as_str()))
            .chain(workspace.as_deref())
            .any(|text| text.to_lowercase().contains(&filter))
    }

    pub fn toggle_workspace_scope(&mut self) {
        self.only_current_workspace = !self.only_current_workspace;
    }

    pub fn toggle_completed(&mut self) {
        self.show_completed = !self.show_completed;
    }

    pub fn toggle_stopped(&mut self) {
        self.hide_stopped = !self.hide_stopped;
    }

    pub fn toggle_responses(&mut self) {
        self.show_responses = !self.show_responses;
    }

    /// Select the entry to move to after completing or abandoning an interaction.
    /// The finished row stays listed; the completion filter controls its visibility.
    ///
    /// Prefer live work in the same worktree, then in the same Workspace, then
    /// a stopped one left in the same Workspace — leaving the Workspace only
    /// when nothing unfinished remains in it. Within each group the nearest
    /// row as the navigator draws them wins: after it first, then before it.
    pub fn select_past_hidden(
        &mut self,
        id: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let hidden = self
            .items
            .iter()
            .position(|interaction| interaction.id == id)?;
        let own_workspace = &self.items[hidden].workspace_id;
        // The finished row is placed among the visible ones, even when it is
        // itself hidden, so "nearest" is measured from where it was drawn.
        let mut shown = self.visible_indices("", workspace_id);
        if !shown.contains(&hidden) {
            shown.push(hidden);
            shown.sort_unstable();
        }
        let order = display_tree(&self.items, &self.workspaces, shown)
            .into_iter()
            .map(|row| row.index)
            .collect::<Vec<_>>();
        let at = order.iter().position(|index| *index == hidden)?;
        let alongside = order
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, index)| {
                let interaction = &self.items[*index];
                *index != hidden
                    && interaction.workspace_id == *own_workspace
                    && !interaction.completed.is_done()
                    && (!self.hide_stopped || interaction.activity.accepting())
            })
            .collect::<Vec<_>>();
        let live_alongside = alongside
            .iter()
            .copied()
            .filter(|(_, index)| self.items[*index].activity.accepting())
            .collect::<Vec<_>>();
        let same_worktree = live_alongside
            .iter()
            .copied()
            .filter(|(_, index)| self.items[*index].workspace == self.items[hidden].workspace)
            .collect::<Vec<_>>();
        let nearest = |rows: &[(usize, usize)]| {
            rows.iter()
                .find(|(position, _)| *position > at)
                .or_else(|| rows.last())
                .map(|(_, index)| *index)
        };
        if let Some(index) = nearest(&same_worktree)
            .or_else(|| nearest(&live_alongside))
            .or_else(|| nearest(&alongside))
        {
            // The cursor is not left on an entry the navigator no longer offers.
            self.rest();
            return self.items.get(index).cloned();
        }
        // The finished row is stopped, so it stays in view only while
        // neither filter hides it.
        if self.show_completed && !self.hide_stopped {
            self.rest();
            return self.items.get(hidden).cloned();
        }
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
        if self.visible_indices("", workspace_id).is_empty() {
            self.only_current_workspace = false;
        }
        if self.visible_indices("", workspace_id).is_empty() {
            self.clear_filter();
            self.tag_filter.clear();
        }
        let visible = self.visible_indices("", workspace_id);
        visible
            .iter()
            .copied()
            .find(|index| *index >= from)
            .or_else(|| visible.last().copied())
            .and_then(|index| self.items.get(index))
            .cloned()
    }
}

/// Unfinished work whose agent can still receive messages. Shared by startup
/// selection, Workspace entry and the live Workspace jump.
pub fn is_live(interaction: &InteractionSummary) -> bool {
    interaction.activity.accepting() && !interaction.completed.is_done()
}

/// The Interaction to land on when entering `workspace_id`: the one an
/// operator most recently turned to, so arriving returns to the work they were
/// last on. Among those the server cannot tell apart — none turned to since it
/// started — one waiting on the operator outranks one mid-turn, and the newest
/// wins a tie. This also counts opening an Interaction, while the navigator
/// sorts by the most recent operator message.
///
/// Stopped and completed Interactions are not candidates. Entering a Workspace
/// should land on work that can still be talked to; a stopped Interaction is reached the
/// same way it always was, through the Session picker.
pub fn first_live_in_workspace(
    interactions: &[InteractionSummary],
    workspace_id: &str,
) -> Option<InteractionSummary> {
    first_live_where(interactions, |interaction| {
        interaction.workspace_id == workspace_id
    })
}

/// [`first_live_in_workspace`] across every Workspace: what a launch that
/// skips the Workspace list lands on when its directory has no Workspace.
pub fn first_live(interactions: &[InteractionSummary]) -> Option<InteractionSummary> {
    first_live_where(interactions, |_| true)
}

fn first_live_where(
    interactions: &[InteractionSummary],
    candidate: impl Fn(&InteractionSummary) -> bool,
) -> Option<InteractionSummary> {
    let mut live = interactions
        .iter()
        .filter(|interaction| is_live(interaction) && candidate(interaction))
        .cloned()
        .collect::<Vec<_>>();
    sort_interactions(&mut live);
    live.into_iter().min_by_key(|interaction| {
        (
            std::cmp::Reverse(interaction.interacted_at_ms),
            activity_rank(interaction),
        )
    })
}

/// `visible` re-ordered so each Workspace's entries are contiguous, in the
/// order the Workspaces themselves first appear. Within a Workspace they are
/// kept together by the checkout they work in — the main checkout counting as
/// one, like any worktree — in the order the checkouts first appear: the
/// groups [`display_tree`] lays out under the navigator's Workspace and
/// worktree headings, rather than the message-sorted item order.
fn grouped_by_workspace(
    interactions: &[InteractionSummary],
    workspaces: &[WorkspaceSummary],
    visible: Vec<usize>,
) -> Vec<usize> {
    let first = |same: &dyn Fn(&InteractionSummary) -> bool| {
        visible.iter().position(|index| same(&interactions[*index]))
    };
    let mut ordered = visible.clone();
    // The sort is stable, so each group keeps the item order.
    ordered.sort_by_cached_key(|index| {
        let interaction = &interactions[*index];
        let workspace = first(&|other| other.workspace_id == interaction.workspace_id);
        let checkout = first(&|other| same_checkout(interaction, other, workspaces));
        (workspace, checkout)
    });
    ordered
}

/// Whether `left` and `right` work in the same checkout of their Workspace:
/// both in its main checkout, or both in the same worktree.
fn same_checkout(
    left: &InteractionSummary,
    right: &InteractionSummary,
    workspaces: &[WorkspaceSummary],
) -> bool {
    left.workspace_id == right.workspace_id
        && match (
            in_main_checkout(left, workspaces),
            in_main_checkout(right, workspaces),
        ) {
            (true, true) => true,
            (false, false) => left.workspace == right.workspace,
            _ => false,
        }
}

/// One entry of the list, placed where the navigator draws it.
///
/// Two relations are drawn, each its own way. Where the work goes is the
/// structure: entries sharing a worktree sit under its heading, and a worktree
/// made from another one's branch — which is what it is integrated back into —
/// is headed beneath that one's heading. Where the conversation came from is
/// told within that: a branch working in the same checkout as its source hangs
/// beneath it, and one working anywhere else names its source instead, its
/// checkout following the source's — see [`follow_sources`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayRow {
    /// The entry's index in [`LiveInteractions::items`].
    pub index: usize,
    /// How many sources deep it hangs beneath the entry it was branched from,
    /// in the checkout they both work in.
    pub depth: usize,
    /// How many worktree headings it sits beneath: none in the main checkout
    /// and in a worktree drawn without a heading, one under its worktree's,
    /// and one more for each worktree that one is integrated into in turn.
    pub level: usize,
    /// The heading of the worktree this entry is the first of, drawn just
    /// above it one level further out.
    pub heading: Option<WorktreeHeading>,
    /// The Session this entry was branched from, when it does not hang beneath
    /// it: one working in another checkout, or not shown at all.
    pub branched_from: Option<String>,
}

/// A worktree heading, over the entries working in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeHeading {
    /// The branch the worktree was made from, which integrating its work
    /// merges it back into. `None` when no entry in it recorded one.
    pub into: Option<String>,
}

/// `visible` laid out as [`DisplayRow`]s, in the order the navigator draws
/// them: each Workspace's entries together, each checkout in the order it
/// first appears — the main checkout unheaded, each worktree followed by the
/// worktrees integrated into it, or following the entry it was branched from
/// — see [`follow_sources`] — and within each checkout the branch tree — see
/// [`branch_tree`].
///
/// The sort reads bottom up while the headings still head from above: the
/// item order is turned over, so the latest-messaged entry of each checkout
/// is its last, and the Workspace and checkout holding the latest of all come
/// last.
///
/// A worktree is headed when it holds more than one shown entry, or is
/// integrated into, or integrates into, another worktree: an entry drawn among
/// another worktree's rows has to say which one it works in, and one whose
/// target is not shown has to say where its work goes. A
/// worktree is integrated into another when the branch it was made from is
/// the one that other worktree has checked out. A malformed loop of those has
/// no root, so its worktrees are headed at the root rather than lost.
fn display_tree(
    interactions: &[InteractionSummary],
    workspaces: &[WorkspaceSummary],
    visible: Vec<usize>,
) -> Vec<DisplayRow> {
    let mut ordered = grouped_by_workspace(interactions, workspaces, visible);
    // Each group stays contiguous when the whole list is reversed.
    ordered.reverse();
    let mut out = Vec::with_capacity(ordered.len());
    for group in ordered.chunk_by(|left, right| {
        interactions[*left].workspace_id == interactions[*right].workspace_id
    }) {
        // `grouped_by_workspace` keeps each checkout's entries together.
        let worktrees: Vec<&[usize]> = group
            .chunk_by(|left, right| {
                same_checkout(&interactions[*left], &interactions[*right], workspaces)
            })
            .collect();
        let main: Vec<bool> = worktrees
            .iter()
            .map(|rows| in_main_checkout(&interactions[rows[0]], workspaces))
            .collect();
        let branch = |rows: &[usize]| {
            rows.iter()
                .find_map(|index| interactions[*index].checkout.as_ref()?.branch.clone())
        };
        let into = |rows: &[usize]| {
            rows.iter()
                .find_map(|index| interactions[*index].branched_from.as_ref()?.branch.clone())
        };
        let parents: Vec<Option<usize>> = worktrees
            .iter()
            .enumerate()
            .map(|(at, rows)| {
                if main[at] {
                    return None;
                }
                let into = into(rows)?;
                (0..worktrees.len()).find(|other| {
                    *other != at
                        && !main[*other]
                        && branch(worktrees[*other]).as_ref() == Some(&into)
                })
            })
            .collect();
        let mut children = vec![Vec::new(); worktrees.len()];
        for (at, parent) in parents.iter().enumerate() {
            if let Some(parent) = parent {
                children[*parent].push(at);
            }
        }
        // Whether the worktree was made from another one, shown or not: one
        // whose source is hidden — completed, or filtered out — stands at the
        // root, but keeps the heading that says where its work goes.
        let integrates_into_worktree = |rows: &[usize]| {
            let workspace_id = &interactions[rows[0]].workspace_id;
            into(rows).is_some_and(|into| {
                interactions.iter().any(|other| {
                    &other.workspace_id == workspace_id
                        && !in_main_checkout(other, workspaces)
                        && other
                            .checkout
                            .as_ref()
                            .is_some_and(|checkout| checkout.branch.as_ref() == Some(&into))
                })
            })
        };
        let roots = (0..worktrees.len())
            .filter(|at| parents[*at].is_none())
            .chain(0..worktrees.len());
        let mut placed = vec![false; worktrees.len()];
        // Each root worktree with the worktrees nested beneath it.
        let mut blocks = Vec::new();
        for root in roots {
            let mut block = Vec::new();
            let mut stack = vec![(root, 0)];
            while let Some((at, nesting)) = stack.pop() {
                if placed[at] {
                    continue;
                }
                placed[at] = true;
                let rows = worktrees[at];
                // The main checkout is never headed: its entries stand at the root.
                let headed = !main[at]
                    && (rows.len() > 1
                        || !children[at].is_empty()
                        || integrates_into_worktree(rows));
                let (level, heading) = if headed {
                    (nesting + 1, Some(WorktreeHeading { into: into(rows) }))
                } else {
                    (0, None)
                };
                branch_tree(interactions, rows, level, heading, &mut block);
                stack.extend(children[at].iter().rev().map(|child| (*child, nesting + 1)));
            }
            if !block.is_empty() {
                blocks.push(block);
            }
        }
        follow_sources(interactions, blocks, &mut out);
    }
    out
}

/// `blocks`, one Workspace's root worktrees each with those nested beneath it,
/// appended to `out` in their order, except that one whose first branched entry
/// came from an entry shown in another block is moved to follow that source:
/// directly after it and its branches when it stands unheaded at the root, as
/// the branch would hang beneath it in a shared checkout, and after its whole
/// block when it is headed, rather than splitting its worktree's rows. A
/// malformed loop of those sources has no root, so its blocks are placed in
/// their existing order rather than lost.
fn follow_sources(
    interactions: &[InteractionSummary],
    blocks: Vec<Vec<DisplayRow>>,
    out: &mut Vec<DisplayRow>,
) {
    let block_of = |id: &str| {
        blocks
            .iter()
            .position(|block| block.iter().any(|row| interactions[row.index].id == id))
    };
    let sources: Vec<Option<(usize, String)>> = blocks
        .iter()
        .enumerate()
        .map(|(at, block)| {
            block.iter().find_map(|row| {
                let source = row.branched_from.as_ref()?;
                block_of(source)
                    .filter(|other| *other != at)
                    .map(|other| (other, source.clone()))
            })
        })
        .collect();

    fn place(
        at: usize,
        blocks: &[Vec<DisplayRow>],
        sources: &[Option<(usize, String)>],
        interactions: &[InteractionSummary],
        placed: &mut [bool],
        out: &mut Vec<DisplayRow>,
    ) {
        if placed[at] {
            return;
        }
        placed[at] = true;
        let rows = &blocks[at];
        let following = |id: &str| -> Vec<usize> {
            (0..blocks.len())
                .filter(|other| {
                    sources[*other]
                        .as_ref()
                        .is_some_and(|(block, source)| *block == at && source == id)
                })
                .collect()
        };
        // The rows the source's followers come after, by their source.
        let mut pending: Vec<(usize, &str)> = Vec::new();
        for (row_at, row) in rows.iter().enumerate() {
            let id = interactions[row.index].id.as_str();
            let end = if row.level == 0 {
                row_at
                    + rows[row_at + 1..]
                        .iter()
                        .take_while(|after| after.level == 0 && after.depth > row.depth)
                        .count()
            } else {
                rows.len() - 1
            };
            pending.push((end, id));
            out.push(row.clone());
            let due: Vec<&str> = pending
                .iter()
                .filter(|(end, _)| *end == row_at)
                .map(|(_, id)| *id)
                .collect();
            pending.retain(|(end, _)| *end != row_at);
            for id in due {
                for follower in following(id) {
                    place(follower, blocks, sources, interactions, placed, out);
                }
            }
        }
    }

    let mut placed = vec![false; blocks.len()];
    let roots = (0..blocks.len())
        .filter(|at| sources[*at].is_none())
        .chain(0..blocks.len());
    for at in roots {
        place(at, &blocks, &sources, interactions, &mut placed, out);
    }
}

/// `rows`, the entries working in one checkout, laid out so each branch sits
/// directly beneath the entry it was branched from, siblings in the order they
/// already had — the same tree the Session picker draws — and appended to
/// `out` at `level`, the first of them under `heading`.
///
/// Only a source in the same checkout counts: one working elsewhere is drawn
/// elsewhere, under its own checkout, and moving the branch out from under
/// its own would put it among work it does not share. Those, and branches of
/// a source not shown, stand at the root and name their source instead. A
/// malformed loop of origins has no root, so its entries are placed at the
/// root in their existing order rather than lost.
fn branch_tree(
    interactions: &[InteractionSummary],
    rows: &[usize],
    level: usize,
    mut heading: Option<WorktreeHeading>,
    out: &mut Vec<DisplayRow>,
) {
    let parents: Vec<Option<usize>> = rows
        .iter()
        .enumerate()
        .map(|(at, index)| {
            let origin = interactions[*index].origin.as_ref()?;
            rows.iter()
                .position(|other| interactions[*other].id == origin.session_id)
                .filter(|parent| *parent != at)
        })
        .collect();
    let mut children = vec![Vec::new(); rows.len()];
    for (at, parent) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            children[*parent].push(at);
        }
    }

    fn walk(
        at: usize,
        depth: usize,
        children: &[Vec<usize>],
        placed: &mut [bool],
        out: &mut Vec<(usize, usize)>,
    ) {
        if placed[at] {
            return;
        }
        placed[at] = true;
        out.push((at, depth));
        for child in &children[at] {
            walk(*child, depth + 1, children, placed, out);
        }
    }

    let mut placed = vec![false; rows.len()];
    let mut tree = Vec::with_capacity(rows.len());
    for (at, parent) in parents.iter().enumerate() {
        if parent.is_none() {
            walk(at, 0, &children, &mut placed, &mut tree);
        }
    }
    for at in 0..rows.len() {
        walk(at, 0, &children, &mut placed, &mut tree);
    }
    out.extend(tree.into_iter().map(|(at, depth)| {
        let index = rows[at];
        DisplayRow {
            index,
            depth,
            level,
            heading: heading.take(),
            branched_from: interactions[index]
                .origin
                .as_ref()
                .filter(|_| depth == 0)
                .map(|origin| origin.session_id.clone()),
        }
    }));
}

/// Whether `interaction` works in its Workspace's own checkout — on its main
/// branch, typically — rather than in a worktree made for it. Before Git has
/// been asked where it works, that is whether it works in the Workspace
/// directory: Styra makes its worktrees outside it.
pub fn in_main_checkout(interaction: &InteractionSummary, workspaces: &[WorkspaceSummary]) -> bool {
    match &interaction.checkout {
        Some(checkout) => !checkout.linked(),
        None => workspaces
            .iter()
            .find(|workspace| workspace.id == interaction.workspace_id)
            .is_none_or(|workspace| interaction.workspace.starts_with(&workspace.host_path)),
    }
}

/// Whether `now` has something to show that `before`, the same row's last
/// listing, did not: it changed what it is doing, or its agent said something
/// new. Every event is not news — a working row's indicator already steps with
/// those, and lighting on each would keep a busy row lit throughout.
fn has_news(before: &InteractionSummary, now: &InteractionSummary) -> bool {
    before.activity != now.activity || before.last_message != now.last_message
}

fn sort_interactions(interactions: &mut [InteractionSummary]) {
    // Keep paused interactions below the others, latest operator message first.
    interactions.sort_by_key(|interaction| {
        (
            interaction.activity_reason == Some(styra_protocol::InteractionActivityReason::Paused),
            std::cmp::Reverse(if interaction.last_user_message_at_ms == 0 {
                created_at_ms(interaction)
            } else {
                interaction.last_user_message_at_ms
            }),
            std::cmp::Reverse(created_at_ms(interaction)),
        )
    });
}

/// The creation time the server embeds as the leading field of an Interaction
/// id (its Session id), in epoch milliseconds. An id of any other shape sorts
/// as oldest rather than failing the listing.
fn created_at_ms(interaction: &InteractionSummary) -> u64 {
    interaction
        .id
        .split('-')
        .next()
        .and_then(|millis| millis.parse().ok())
        .unwrap_or(0)
}

/// Waiting on the operator outranks mid-turn, which outranks stopped.
fn activity_rank(interaction: &InteractionSummary) -> u8 {
    match interaction.activity {
        styra_protocol::InteractionActivity::Pending => 0,
        styra_protocol::InteractionActivity::Running
        | styra_protocol::InteractionActivity::Background => 1,
        styra_protocol::InteractionActivity::Stopped => 2,
    }
}

/// `Pending` is the server summary's name for a live interaction waiting for
/// input (the TUI calls that state `Idle`).
fn is_idle(interaction: &InteractionSummary) -> bool {
    interaction.activity == styra_protocol::InteractionActivity::Pending
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_protocol::{CompletionState, InteractionActivity};

    pub(crate) fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
        InteractionSummary {
            auto_retry: false,
            auto_commit: false,
            id: id.into(),
            name: None,
            tags: Vec::new(),
            workspace_id: "workspace".into(),
            selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
            workspace: PathBuf::from("/workspace"),
            activity,
            activity_reason: None,
            activity_since_ms: 0,
            idle_unseen: false,
            interacted_at_ms: 0,
            last_user_message_at_ms: 0,
            uncommitted_changes: false,
            expected_branch: None,
            checkout: None,
            last_message: None,
            recent_messages: Vec::new(),
            events: 0,
            branched_from: None,
            completed: CompletionState::Active,
            focus_requested: false,
            origin: None,
            integration: None,
            restored: false,
        }
    }

    /// An interaction the operator finished with: stopped, and marked
    /// complete on the Session it serves.
    fn completed(id: &str) -> InteractionSummary {
        let mut interaction = interaction(id, InteractionActivity::Stopped);
        interaction.completed = CompletionState::Completed;
        interaction
    }

    #[test]
    fn paging_follows_filtered_display_order_and_stops_at_the_ends() {
        let mut live = LiveInteractions::default();
        live.open(
            (0..50)
                .map(|index| {
                    let mut item =
                        interaction(&format!("item-{index:02}"), InteractionActivity::Pending);
                    if index % 2 == 0 {
                        item.tags.push("keep".into());
                    }
                    item
                })
                .collect(),
            vec![],
        );
        live.set_tag_filter(vec!["keep".into()], "item-00", None);

        // Display order: item-48 at the top, down to item-00 at the foot.
        live.cursor_page("item-00", None, false);
        assert_eq!(live.cursor("item-00"), "item-20");
        assert!(live.pending("item-00").is_some());
        assert!(live.due("item-00").is_none());
        live.cursor_page("item-00", None, false);
        assert_eq!(live.cursor("item-00"), "item-40");
        for _ in 0..3 {
            live.cursor_page("item-00", None, false);
            assert_eq!(live.cursor("item-00"), "item-48");
        }
        live.cursor_page("item-00", None, true);
        assert_eq!(live.cursor("item-00"), "item-28");
        for _ in 0..4 {
            live.cursor_page("item-00", None, true);
        }
        assert_eq!(live.cursor("item-00"), "item-00");
        assert!(live.pending("item-00").is_none());
    }

    #[test]
    fn paging_handles_an_absent_cursor_and_an_empty_list() {
        let mut live = LiveInteractions::default();
        for down in [true, false] {
            live.cursor_page("missing", None, down);
            assert!(live.pending("missing").is_none());
            live.open(
                vec![interaction("only", InteractionActivity::Pending)],
                vec![],
            );
            live.cursor_page("missing", None, down);
            assert_eq!(live.cursor("missing"), "only");
            live.close();
            live = LiveInteractions::default();
        }
    }

    #[test]
    fn live_interactions_open_newest_first_with_paused_last() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("0000000000200-1-0", InteractionActivity::Pending),
                interaction("0000000000300-1-0", InteractionActivity::Stopped),
                interaction("0000000000100-1-0", InteractionActivity::Running),
            ],
            vec![],
        );

        assert!(live.open);
        let order = |live: &LiveInteractions| {
            live.items
                .iter()
                .map(|interaction| interaction.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            order(&live),
            [
                "0000000000300-1-0",
                "0000000000200-1-0",
                "0000000000100-1-0"
            ]
        );
        assert_eq!(
            live.current("0000000000100-1-0").unwrap().id,
            "0000000000100-1-0"
        );

        // A change of activity does not move a row.
        live.refresh(vec![
            interaction("0000000000100-1-0", InteractionActivity::Pending),
            interaction("0000000000200-1-0", InteractionActivity::Running),
            interaction("0000000000300-1-0", InteractionActivity::Stopped),
        ]);
        assert_eq!(
            order(&live),
            [
                "0000000000300-1-0",
                "0000000000200-1-0",
                "0000000000100-1-0"
            ]
        );

        let mut items = live.items.clone();
        items[0].activity_reason = Some(styra_protocol::InteractionActivityReason::Paused);
        let mut newer_paused = interaction("0000000000400-1-0", InteractionActivity::Stopped);
        newer_paused.activity_reason = Some(styra_protocol::InteractionActivityReason::Paused);
        items.push(newer_paused);
        live.refresh(items);
        assert_eq!(
            order(&live),
            [
                "0000000000200-1-0",
                "0000000000100-1-0",
                "0000000000400-1-0",
                "0000000000300-1-0"
            ]
        );
    }

    #[test]
    fn live_interactions_sort_by_sent_messages_not_focus_or_agent_activity() {
        let mut older = interaction("0000000000100-old", InteractionActivity::Pending);
        older.last_user_message_at_ms = 500;
        let mut newer = interaction("0000000000200-new", InteractionActivity::Running);
        newer.last_user_message_at_ms = 400;
        newer.interacted_at_ms = 900;
        newer.activity_since_ms = 900;
        let unsent = interaction("0000000000300-unsent", InteractionActivity::Pending);
        let mut paused = interaction("0000000000400-paused", InteractionActivity::Stopped);
        paused.last_user_message_at_ms = 1000;
        paused.activity_reason = Some(styra_protocol::InteractionActivityReason::Paused);
        let mut live = LiveInteractions::default();
        live.open(vec![newer.clone(), unsent, paused, older], vec![]);
        let ids = |live: &LiveInteractions| {
            live.items
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&live),
            [
                "0000000000100-old",
                "0000000000200-new",
                "0000000000300-unsent",
                "0000000000400-paused"
            ]
        );

        // A new operator message moves the row on the next listing, even
        // though no other metadata (or agent response) has changed yet.
        newer.last_user_message_at_ms = 1100;
        let mut refreshed = live.items.clone();
        refreshed[1] = newer;
        assert!(live.refresh(refreshed));
        assert_eq!(
            ids(&live),
            [
                "0000000000200-new",
                "0000000000100-old",
                "0000000000300-unsent",
                "0000000000400-paused"
            ]
        );
        assert_eq!(live.display_indices("", Some("workspace")), [3, 2, 1, 0]);
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
        let previous = live.previous("one", "", Some("workspace")).unwrap();
        assert_eq!(previous.id, "two");
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
    fn refreshed_names_have_one_lookup_for_the_list_and_current_screen() {
        let mut live = LiveInteractions::default();
        let mut original = interaction("one", InteractionActivity::Pending);
        original.name = Some("Original".into());
        live.open(vec![original], vec![]);

        assert_eq!(live.name("one"), Some("Original"));
        assert_eq!(live.current("one").unwrap().name, None);

        let mut renamed = interaction("one", InteractionActivity::Pending);
        renamed.name = Some("Renamed".into());
        assert!(live.refresh(vec![renamed]));

        assert_eq!(live.name("one"), Some("Renamed"));
        assert_eq!(live.current("one").unwrap().name, None);
    }

    #[test]
    fn a_snapshot_seeds_another_listing_with_its_names() {
        let mut named = interaction("one", InteractionActivity::Pending);
        named.name = Some("Named".into());
        let mut live = LiveInteractions::default();
        assert!(!live.listed());
        live.refresh(vec![
            named,
            interaction("two", InteractionActivity::Running),
        ]);
        assert!(live.listed());

        let mut seeded = LiveInteractions::default();
        seeded.refresh(live.snapshot());

        assert_eq!(seeded.name("one"), Some("Named"));
        assert_eq!(seeded.name("two"), None);
        assert_eq!(seeded.items, live.items);
    }

    #[test]
    fn completed_interactions_are_hidden_until_toggled() {
        let mut abandoned = interaction("abandoned", InteractionActivity::Stopped);
        abandoned.completed = CompletionState::Abandoned;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                completed("completed"),
                interaction("active", InteractionActivity::Pending),
                abandoned,
            ],
            vec![],
        );

        assert_eq!(
            live.visible_indices("", Some("workspace")),
            vec![1],
            "completed and abandoned rows are hidden by default"
        );
        live.toggle_completed();
        assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);
    }

    /// Finished rows are left on the server until the operator asks to see
    /// them, except the one on screen, which the screen still describes.
    #[test]
    fn the_listing_asks_for_finished_rows_only_when_they_are_shown() {
        let mut live = LiveInteractions::default();
        let query = live.query("current");
        assert!(query.skip_done);
        assert!(!query.skip_stopped, "hiding stopped rows is a local filter");
        assert_eq!(query.keep, vec!["current".to_owned()]);
        assert!(query.admits_summary(&completed("current")));
        assert!(!query.admits_summary(&completed("other")));

        assert!(
            live.query("").keep.is_empty(),
            "a blank screen keeps nothing"
        );

        live.toggle_completed();
        assert!(!live.query("current").skip_done);
    }

    #[test]
    fn stopped_rows_can_be_hidden() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("stopped", InteractionActivity::Stopped),
                interaction("idle", InteractionActivity::Pending),
                completed("completed"),
            ],
            vec![],
        );
        live.show_completed = true;
        assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);

        live.toggle_stopped();

        assert_eq!(
            live.visible_indices("", Some("workspace")),
            vec![1],
            "stopped rows, completed ones included, are hidden"
        );
    }

    /// Integrating the Interaction on screen completes it; it stays listed,
    /// with its branches nested under it, for as long as it is on screen.
    #[test]
    fn the_current_row_is_listed_whatever_its_state() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                completed("current"),
                interaction("stopped", InteractionActivity::Stopped),
                completed("other"),
            ],
            vec![],
        );
        assert_eq!(
            live.visible_indices("current", Some("workspace")),
            vec![0, 1]
        );

        live.toggle_stopped();
        assert_eq!(live.visible_indices("current", Some("workspace")), vec![0]);
        assert!(live.visible_indices("", Some("workspace")).is_empty());

        live.set_tag_filter(vec!["missing".into()], "current", Some("workspace"));
        assert!(
            live.visible_indices("current", Some("workspace"))
                .is_empty(),
            "the tag filter still narrows the list"
        );
    }

    #[test]
    fn finishing_with_stopped_rows_hidden_skips_a_stopped_sibling() {
        let mut stopped = interaction("stopped", InteractionActivity::Stopped);
        stopped.workspace_id = "workspace".into();
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other".into();
        let mut live = LiveInteractions::default();
        live.open(vec![completed("finished"), stopped, elsewhere], vec![]);
        live.show_completed = true;
        live.hide_stopped = true;

        assert_eq!(
            live.select_past_hidden("finished", None).unwrap().id,
            "elsewhere"
        );
    }

    /// Completion is a property of the Session, cleared by the server when it
    /// is resumed, so an interaction started again arrives already not
    /// completed — and the listing shows it without anything here having to
    /// clear a mark.
    #[test]
    fn an_interaction_started_again_is_no_longer_completed() {
        let mut live = LiveInteractions::default();
        live.open(vec![completed("finished")], vec![]);
        assert!(live.visible_indices("", Some("workspace")).is_empty());

        live.refresh(vec![interaction("finished", InteractionActivity::Running)]);

        assert_eq!(live.visible_indices("", Some("workspace")), vec![0]);
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
    fn finishing_prefers_the_same_worktree_before_the_rest_of_the_workspace() {
        for finished_state in [CompletionState::Completed, CompletionState::Abandoned] {
            for show_completed in [false, true] {
                for sibling_before in [false, true] {
                    let mut finished = completed("finished");
                    finished.completed = finished_state;
                    finished.workspace = PathBuf::from("/worktrees/shared");
                    let mut sibling = interaction("sibling", InteractionActivity::Pending);
                    sibling.workspace = finished.workspace.clone();
                    let other_worktree =
                        interaction("other-worktree", InteractionActivity::Running);
                    let items = if sibling_before {
                        vec![sibling, finished, other_worktree]
                    } else {
                        vec![finished, other_worktree, sibling]
                    };
                    let mut live = LiveInteractions::default();
                    live.open(items, vec![]);
                    live.show_completed = show_completed;

                    let next = live.select_past_hidden("finished", None).unwrap();

                    assert_eq!(next.id, "sibling");
                }
            }
        }
    }

    #[test]
    fn finishing_skips_stopped_and_finished_worktree_siblings() {
        let mut finished = completed("finished");
        finished.workspace = PathBuf::from("/worktrees/shared");
        let mut stopped = interaction("stopped", InteractionActivity::Stopped);
        stopped.workspace = finished.workspace.clone();
        let mut done = interaction("done", InteractionActivity::Pending);
        done.workspace = finished.workspace.clone();
        done.completed = CompletionState::Completed;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                finished,
                stopped,
                done,
                interaction("next", InteractionActivity::Pending),
            ],
            vec![],
        );
        live.show_completed = true;

        assert_eq!(
            live.select_past_hidden("finished", None).unwrap().id,
            "next"
        );
    }

    #[test]
    fn finishing_with_completed_rows_shown_stays_when_no_local_work_remains() {
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other".into();
        let mut live = LiveInteractions::default();
        live.open(vec![completed("finished"), elsewhere], vec![]);
        live.show_completed = true;

        assert_eq!(
            live.select_past_hidden("finished", None).unwrap().id,
            "finished"
        );
    }

    /// The rest of the completed interaction's own Workspace comes before any
    /// other Workspace, even one listed next to it — and its live work comes
    /// before a stopped row of its own Workspace.
    #[test]
    fn completing_an_interaction_moves_on_to_live_work_in_its_own_workspace() {
        let mut live = LiveInteractions::default();
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other".into();
        live.open(
            vec![
                interaction("earlier", InteractionActivity::Running),
                completed("finished"),
                elsewhere,
                interaction("stopped", InteractionActivity::Stopped),
            ],
            vec![],
        );

        let next = live.select_past_hidden("finished", None).unwrap();

        assert_eq!(next.id, "earlier");
    }

    /// A stopped row of its own Workspace is still work left in it, and is
    /// moved to before any other Workspace's live work.
    #[test]
    fn completing_an_interaction_stays_in_its_workspace_while_any_work_is_left() {
        let mut live = LiveInteractions::default();
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other".into();
        live.open(
            vec![
                completed("finished"),
                elsewhere,
                interaction("stopped", InteractionActivity::Stopped),
            ],
            vec![],
        );

        let next = live.select_past_hidden("finished", None).unwrap();

        assert_eq!(next.id, "stopped");
    }

    /// Nearest is where the navigator draws the rows — grouped by worktree —
    /// not the order they were created in.
    #[test]
    fn completing_an_interaction_moves_to_the_nearest_row_as_drawn() {
        let in_worktree = |id: &str, activity, name: &str| {
            let mut interaction = interaction(id, activity);
            let path = PathBuf::from(format!("/worktrees/{name}"));
            interaction.workspace = path.clone();
            interaction.checkout = Some(styra_protocol::CheckoutState {
                worktree: path,
                repository: PathBuf::from("/workspace"),
                branch: Some(name.into()),
            });
            interaction
        };
        let mut finished = in_worktree("finished", InteractionActivity::Stopped, "second");
        finished.completed = CompletionState::Completed;
        let mut live = LiveInteractions::default();
        // Drawn as: first-1, first-2, main, second (finished); in creation
        // order first-2 would be the row just before the finished one.
        live.open(
            vec![
                in_worktree("first-1", InteractionActivity::Pending, "first"),
                interaction("main", InteractionActivity::Pending),
                in_worktree("first-2", InteractionActivity::Pending, "first"),
                finished,
            ],
            vec![],
        );

        let next = live.select_past_hidden("finished", None).unwrap();

        assert_eq!(next.id, "main");
    }

    /// With no work left in its own Workspace, the move falls back to the
    /// next visible row wherever it is.
    #[test]
    fn completing_the_last_live_interaction_of_a_workspace_moves_past_it() {
        let mut live = LiveInteractions::default();
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other".into();
        live.open(vec![completed("finished"), elsewhere], vec![]);

        let next = live.select_past_hidden("finished", None).unwrap();

        assert_eq!(next.id, "elsewhere");
    }

    /// A focus request is answered when its interaction first arrives, and
    /// only then: not for rows already running when the client started, and
    /// not again on the refreshes after.
    #[test]
    fn a_focus_request_is_claimed_once_on_arrival() {
        let mut asking = interaction("asking", InteractionActivity::Running);
        asking.focus_requested = true;
        let mut live = LiveInteractions::default();
        live.refresh(vec![asking.clone()]);
        assert_eq!(live.take_focus_claim(), None, "it was already running");

        let mut arrived = interaction("arrived", InteractionActivity::Running);
        arrived.focus_requested = true;
        live.refresh(vec![
            asking.clone(),
            interaction("quiet", InteractionActivity::Running),
            arrived.clone(),
        ]);
        assert_eq!(
            live.take_focus_claim().map(|claim| claim.id),
            Some("arrived".into())
        );
        assert_eq!(live.take_focus_claim(), None);

        live.refresh(vec![asking, arrived]);
        assert_eq!(
            live.take_focus_claim(),
            None,
            "still listed is not arriving"
        );
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

    /// The tally is of everything listed, seen or not, and counts background
    /// work as running: it answers "what is the fleet doing", not "what is
    /// waiting for me".
    #[test]
    fn the_activity_tally_covers_every_listed_interaction() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("working", InteractionActivity::Running),
                interaction("background", InteractionActivity::Background),
                interaction("waiting", InteractionActivity::Pending),
                interaction("done", InteractionActivity::Stopped),
                interaction("also-done", InteractionActivity::Stopped),
            ],
            Vec::new(),
        );
        assert_eq!(
            live.activity_counts(),
            ActivityCounts {
                running: 2,
                idle: 1,
                stopped: 2,
            }
        );
    }

    /// Completion is the reason the server stopped these, so without the
    /// exclusion every interaction the operator ever signed off on would keep
    /// inflating the stopped number for the rest of the session.
    #[test]
    fn the_activity_tally_leaves_out_completed_interactions() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("working", InteractionActivity::Running),
                interaction("abandoned", InteractionActivity::Stopped),
                completed("signed-off"),
            ],
            Vec::new(),
        );
        assert_eq!(
            live.activity_counts(),
            ActivityCounts {
                running: 1,
                idle: 0,
                stopped: 1,
            }
        );
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

        // Display order: later, busy, unseen, current.
        assert_eq!(live.next_idle_unseen("current", None).unwrap().id, "later");
        assert_eq!(live.next_idle_unseen("later", None).unwrap().id, "unseen");
        assert_eq!(live.next_idle_unseen("unseen", None).unwrap().id, "later");
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

        // The step follows the list's order, wrapping at its end.
        assert_eq!(live.next_live("current", None).unwrap().id, "background");
        assert_eq!(live.next_live("background", None).unwrap().id, "waiting");
        assert_eq!(live.next_live("waiting", None).unwrap().id, "current");
    }

    #[test]
    fn the_n_destination_prioritizes_unseen_idle_work_before_live_work() {
        let mut idle = interaction("idle", InteractionActivity::Pending);
        idle.idle_unseen = true;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                idle,
                interaction("next-live", InteractionActivity::Running),
                interaction("current", InteractionActivity::Running),
            ],
            vec![],
        );

        assert_eq!(
            live.next_attention_or_live("current", None).unwrap().id,
            "idle"
        );

        live.items
            .iter_mut()
            .find(|interaction| interaction.id == "idle")
            .unwrap()
            .idle_unseen = false;
        assert_eq!(
            live.next_attention_or_live("current", None).unwrap().id,
            "next-live"
        );
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

        assert_eq!(live.next_active("current", None).unwrap().id, "background");
        assert_eq!(live.next_active("background", None).unwrap().id, "current");
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

        assert!(live.next_active("current", None).is_none());
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

        assert!(live.next_live("current", None).is_none());
    }

    /// In Workspace scope the steps stay in the current Workspace: the
    /// operator narrowed the list to it, so work elsewhere is not a
    /// destination, and the scope is left as it was.
    #[test]
    fn steps_in_workspace_scope_stay_in_the_current_workspace() {
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Running);
        elsewhere.workspace_id = "other-workspace".into();
        let mut unseen_elsewhere = interaction("unseen-elsewhere", InteractionActivity::Pending);
        unseen_elsewhere.workspace_id = "other-workspace".into();
        unseen_elsewhere.idle_unseen = true;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                elsewhere,
                unseen_elsewhere,
                interaction("here", InteractionActivity::Running),
            ],
            vec![],
        );
        live.toggle_workspace_scope();
        let scope = Some("workspace");

        assert_eq!(
            live.next_attention_or_live("current", scope).unwrap().id,
            "here"
        );
        assert_eq!(
            live.next_attention_or_live("here", scope).unwrap().id,
            "current"
        );
        assert_eq!(live.next_active("current", scope).unwrap().id, "here");
        assert_eq!(live.previous_active("current", scope).unwrap().id, "here");

        let next = live.cursor_to_next_active("current", scope).unwrap();
        assert_eq!(next.id, "here");
        assert!(live.only_current_workspace);
        // Moved like a j/k step, so the load waits for the cursor to rest.
        assert_eq!(live.cursor("current"), "here");
    }

    /// With the rest of the current Workspace stopped there is nowhere to
    /// step in Workspace scope, though there is in All.
    #[test]
    fn steps_in_workspace_scope_do_not_leave_it_for_lack_of_work() {
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

        assert_eq!(
            live.next_live("current", Some("workspace")).unwrap().id,
            "elsewhere"
        );
        live.toggle_workspace_scope();
        assert!(live.next_live("current", Some("workspace")).is_none());
        assert!(live.next_active("current", Some("workspace")).is_none());
    }

    /// Presses quicker than the settle must keep walking the working set, so
    /// each step starts from the cursor rather than the loaded Interaction.
    #[test]
    fn repeated_working_steps_walk_on_from_the_cursor() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("second", InteractionActivity::Running),
                interaction("third", InteractionActivity::Running),
            ],
            vec![],
        );
        let first = live.cursor_to_next_active("current", None).unwrap().id;
        let second = live.cursor_to_next_active("current", None).unwrap().id;

        assert_ne!(first, second);
        assert_eq!(live.cursor("current"), second);
    }

    /// The backwards step walks the working set in reverse, skipping idle
    /// work just as the forward one does.
    #[test]
    fn the_backwards_working_step_reverses_the_forward_one() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("current", InteractionActivity::Running),
                interaction("idle", InteractionActivity::Pending),
                interaction("second", InteractionActivity::Running),
                interaction("third", InteractionActivity::Running),
            ],
            vec![],
        );
        let forward = live.next_active("current", None).unwrap().id;
        let backward = live.previous_active("current", None).unwrap().id;
        assert_ne!(forward, backward);
        assert_eq!(live.previous_active(&forward, None).unwrap().id, "current");

        let back = live.cursor_to_previous_active("current", None).unwrap().id;
        assert_eq!(back, backward);
        assert_ne!(back, "idle");
        let again = live.cursor_to_previous_active("current", None).unwrap().id;
        assert_eq!(again, forward, "a second step walks on from the cursor");
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

        assert!(live.next_idle_unseen("current", None).is_none());
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

        live.cursor_next("three", Some("workspace"));
        live.cursor_next("three", Some("workspace"));

        // Two rows crossed, and neither of the interactions passed over — nor
        // the one now under the cursor — has been asked for yet.
        assert_eq!(live.cursor("three"), "one");
        assert_eq!(
            live.pending("three")
                .map(|interaction| interaction.id.as_str()),
            Some("one")
        );
        assert!(live.due("three").is_none());

        std::thread::sleep(LOAD_SETTLE + Duration::from_millis(20));
        assert_eq!(
            live.due("three").map(|interaction| interaction.id.as_str()),
            Some("one")
        );

        // Once the load has landed the cursor is home again, so the settled
        // move is not made a second time.
        live.rest();
        assert_eq!(live.cursor("one"), "one");
        assert!(live.due("one").is_none());
    }

    #[test]
    fn cursor_endpoints_follow_visible_rows_and_defer_loading() {
        let mut live = LiveInteractions::default();
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other-workspace".into();
        live.open(
            vec![
                completed("finished"),
                interaction("one", InteractionActivity::Pending),
                interaction("two", InteractionActivity::Pending),
                interaction("three", InteractionActivity::Pending),
                elsewhere,
            ],
            vec![],
        );
        live.toggle_workspace_scope();
        let scope = Some("workspace");

        live.cursor_first("one", scope);
        assert_eq!(live.cursor("one"), "three");
        assert!(live.pending("one").is_some());
        assert!(live.due("one").is_none());

        live.cursor_last("one", scope);
        assert_eq!(live.cursor("one"), "one");
        assert!(live.pending("one").is_none());

        live.items[1].tags = vec!["match".into()];
        live.items[2].tags = vec!["match".into()];
        live.set_tag_filter(vec!["match".into()], "three", scope);
        live.cursor_last("three", scope);
        assert_eq!(live.cursor("three"), "one");
        live.cursor_first("three", scope);
        assert_eq!(live.cursor("three"), "two");

        live.set_tag_filter(vec!["missing".into()], "three", scope);
        live.rest();
        live.cursor_first("three", scope);
        live.cursor_last("three", scope);
        assert_eq!(live.cursor("three"), "three");
        assert!(live.pending("three").is_none());
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

        live.cursor_next("two", Some("workspace"));
        live.cursor_previous("two", Some("workspace"));

        assert_eq!(live.cursor("two"), "two");
        assert!(live.pending("two").is_none());
        assert!(live.due("two").is_none());
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
        live.cursor_previous("one", Some("workspace"));

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
                interaction("next", InteractionActivity::Running),
                other,
                interaction("current", InteractionActivity::Pending),
            ],
            vec![],
        );

        live.toggle_workspace_scope();
        assert!(live.only_current_workspace);
        assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 2]);
        assert_eq!(
            live.next("current", "", Some("workspace")).unwrap().id,
            "next"
        );

        live.toggle_workspace_scope();
        assert!(!live.only_current_workspace);
        assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);
    }

    /// Interactions working in the same checkout are drawn together, each
    /// checkout where its newest entry falls — the main checkout no different
    /// from a worktree — so j/k walks them in that order too, in either scope.
    #[test]
    fn navigation_follows_the_directory_grouped_display_order() {
        let in_worktree = |id: &str| {
            let mut interaction = interaction(id, InteractionActivity::Pending);
            interaction.workspace = PathBuf::from("/worktrees/feature");
            interaction.checkout = Some(styra_protocol::CheckoutState {
                worktree: PathBuf::from("/worktrees/feature"),
                repository: PathBuf::from("/workspace"),
                branch: Some("feature".into()),
            });
            interaction
        };
        let mut other = interaction("other", InteractionActivity::Pending);
        other.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                in_worktree("feature-1"),
                interaction("main-1", InteractionActivity::Pending),
                other,
                in_worktree("feature-2"),
                interaction("main-2", InteractionActivity::Pending),
            ],
            vec![],
        );
        let ordered = |live: &LiveInteractions| {
            live.display_indices("", Some("workspace"))
                .into_iter()
                .map(|index| live.items[index].id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ordered(&live),
            ["other", "main-2", "main-1", "feature-2", "feature-1"]
        );
        assert_eq!(
            live.next("main-1", "", Some("workspace")).unwrap().id,
            "feature-2"
        );

        live.toggle_workspace_scope();
        assert_eq!(
            ordered(&live),
            ["main-2", "main-1", "feature-2", "feature-1"]
        );
    }

    fn branched(id: &str, activity: InteractionActivity, from: &str) -> InteractionSummary {
        let mut interaction = interaction(id, activity);
        interaction.origin = Some(styra_protocol::SessionOrigin {
            session_id: from.into(),
            provider: styra_protocol::agent::Provider::Codex,
            at_ms: None,
            history: Default::default(),
        });
        interaction
    }

    fn display<'a>(
        live: &'a LiveInteractions,
        workspace_id: Option<&str>,
    ) -> Vec<(&'a str, usize)> {
        live.display_rows("", workspace_id)
            .into_iter()
            .map(|row| (live.items[row.index].id.as_str(), row.depth))
            .collect()
    }

    /// A branch sits beneath the interaction it came from, as it does in the
    /// Session picker, rather than wherever its creation would sort it — a
    /// fresh branch is the newest, and would otherwise sink to the foot of the
    /// list, away from its source.
    #[test]
    fn a_branch_is_listed_beneath_its_source() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                branched(
                    "0000000000400-twig",
                    InteractionActivity::Pending,
                    "0000000000300-branch",
                ),
                interaction("0000000000200-other", InteractionActivity::Running),
                branched(
                    "0000000000300-branch",
                    InteractionActivity::Stopped,
                    "0000000000100-source",
                ),
                interaction("0000000000100-source", InteractionActivity::Running),
            ],
            vec![],
        );

        assert_eq!(
            display(&live, Some("workspace")),
            [
                ("0000000000100-source", 0),
                ("0000000000300-branch", 1),
                ("0000000000400-twig", 2),
                ("0000000000200-other", 0),
            ]
        );
        // Walking the list follows the tree the navigator draws.
        assert_eq!(
            live.next("0000000000100-source", "", Some("workspace"))
                .unwrap()
                .id,
            "0000000000300-branch"
        );
        assert_eq!(
            live.next("0000000000400-twig", "", Some("workspace"))
                .unwrap()
                .id,
            "0000000000200-other"
        );
        live.toggle_workspace_scope();
        assert_eq!(
            display(&live, Some("workspace")),
            [
                ("0000000000100-source", 0),
                ("0000000000300-branch", 1),
                ("0000000000400-twig", 2),
                ("0000000000200-other", 0),
            ]
        );
    }

    /// A branch whose source is not shown has nothing to hang from, so it
    /// stands at the root rather than disappearing with its source.
    #[test]
    fn a_branch_of_a_hidden_source_stands_at_the_root() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                completed("source"),
                branched("branch", InteractionActivity::Stopped, "source"),
                branched("orphan", InteractionActivity::Pending, "closed"),
            ],
            vec![],
        );

        assert_eq!(
            display(&live, Some("workspace")),
            [("orphan", 0), ("branch", 0)]
        );
        live.toggle_completed();
        assert_eq!(
            display(&live, Some("workspace")),
            [("orphan", 0), ("source", 0), ("branch", 1)]
        );
    }

    /// `interaction` working in its own worktree on `branch`, made from `into`.
    fn worktree(
        mut interaction: InteractionSummary,
        branch: &str,
        into: &str,
    ) -> InteractionSummary {
        let path = PathBuf::from(format!("/worktrees/{branch}"));
        interaction.workspace = path.clone();
        interaction.checkout = Some(styra_protocol::CheckoutState {
            worktree: path,
            repository: PathBuf::from("/workspace"),
            branch: Some(branch.into()),
        });
        interaction.branched_from = Some(styra_protocol::BranchPoint {
            branch: Some(into.into()),
            commit: "0".into(),
        });
        interaction
    }

    /// A row as `(id, depth, level, heading's target, named source)`.
    type Placement<'a> = (
        &'a str,
        usize,
        usize,
        Option<Option<String>>,
        Option<String>,
    );

    fn placements(live: &LiveInteractions) -> Vec<Placement<'_>> {
        live.display_rows("", Some("workspace"))
            .into_iter()
            .map(|row| {
                (
                    live.items[row.index].id.as_str(),
                    row.depth,
                    row.level,
                    row.heading.map(|heading| heading.into),
                    row.branched_from,
                )
            })
            .collect()
    }

    /// Where the work goes is the structure — a worktree made from another's
    /// branch is headed beneath it — and where the conversation came from is
    /// told within that: nested in the same checkout, named from any other.
    #[test]
    fn worktrees_nest_by_integration_and_branches_by_checkout() {
        use InteractionActivity::Pending;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("tidy", Pending),
                // Branched, in the same (main) checkout.
                branched("tidy-more", Pending, "tidy"),
                worktree(interaction("rework", Pending), "auth", "main"),
                // Branched, in the same worktree.
                worktree(branched("jwt", Pending, "rework"), "auth", "main"),
                // Not branched, in the same worktree.
                worktree(interaction("review", Pending), "auth", "main"),
                // Not branched, in a worktree made from that one.
                worktree(interaction("tests", Pending), "tests", "auth"),
                // Branched, into a worktree made from the source's.
                worktree(branched("spike", Pending, "rework"), "spike", "auth"),
                // Branched, into an unrelated worktree.
                worktree(branched("docs", Pending, "rework"), "docs", "main"),
            ],
            vec![],
        );
        let into = |branch: &str| Some(Some(branch.to_owned()));
        let rework = Some("rework".to_owned());
        assert_eq!(
            placements(&live),
            [
                ("review", 0, 1, into("main"), None),
                ("rework", 0, 1, None, None),
                ("jwt", 1, 1, None, None),
                ("spike", 0, 2, into("auth"), rework.clone()),
                ("tests", 0, 2, into("auth"), None),
                // Follows its source's worktree rather than splitting it.
                ("docs", 0, 0, None, rework),
                ("tidy", 0, 0, None, None),
                ("tidy-more", 1, 0, None, None),
            ]
        );
        // Walking the list follows the same order.
        assert_eq!(live.next("jwt", "", Some("workspace")).unwrap().id, "spike");
    }

    /// A branch working in a worktree of its own still follows the entry it
    /// came from, past that entry's branches in its own checkout, rather than
    /// sorting wherever its worktree would on its own.
    #[test]
    fn a_branch_in_another_worktree_follows_its_source() {
        use InteractionActivity::Pending;
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                interaction("newest", Pending),
                worktree(branched("twig", Pending, "spike"), "twig", "main"),
                worktree(branched("spike", Pending, "source"), "spike", "main"),
                branched("nested", Pending, "source"),
                interaction("source", Pending),
                interaction("oldest", Pending),
            ],
            vec![],
        );
        let source = |id: &str| Some(id.to_owned());
        assert_eq!(
            placements(&live),
            [
                ("oldest", 0, 0, None, None),
                ("source", 0, 0, None, None),
                ("nested", 1, 0, None, None),
                ("spike", 0, 0, None, source("source")),
                ("twig", 0, 0, None, source("spike")),
                ("newest", 0, 0, None, None),
            ]
        );
        assert_eq!(
            live.next("nested", "", Some("workspace")).unwrap().id,
            "spike"
        );
    }

    /// A worktree made from one whose interactions are all hidden has no
    /// heading to sit beneath, so it stands at the root, but is still headed
    /// with where its work goes.
    #[test]
    fn a_worktree_made_from_a_hidden_one_keeps_its_heading() {
        let mut live = LiveInteractions::default();
        let mut source = worktree(completed("rework"), "auth", "main");
        source.activity = InteractionActivity::Stopped;
        live.open(
            vec![
                source,
                worktree(
                    interaction("tests", InteractionActivity::Pending),
                    "tests",
                    "auth",
                ),
            ],
            vec![],
        );
        assert_eq!(
            placements(&live),
            [("tests", 0, 1, Some(Some("auth".to_owned())), None)]
        );
    }

    /// Interactions sharing a worktree stay together under its heading: a
    /// branch working elsewhere is not nested beneath a source among them,
    /// which would put it between them, but follows the whole worktree.
    #[test]
    fn a_branch_does_not_break_up_a_shared_worktree() {
        let in_worktree = |mut interaction: InteractionSummary| {
            interaction.workspace = PathBuf::from("/worktrees/feature");
            interaction.checkout = Some(styra_protocol::CheckoutState {
                worktree: PathBuf::from("/worktrees/feature"),
                repository: PathBuf::from("/workspace"),
                branch: Some("feature".into()),
            });
            interaction
        };
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                in_worktree(interaction("source", InteractionActivity::Pending)),
                branched("branch", InteractionActivity::Stopped, "source"),
                in_worktree(interaction("sibling", InteractionActivity::Pending)),
            ],
            vec![],
        );

        assert_eq!(
            display(&live, Some("workspace")),
            [("sibling", 0), ("source", 0), ("branch", 0)]
        );
    }

    #[test]
    fn row_navigation_without_a_selection_starts_at_the_bottom() {
        for backwards in [false, true] {
            let mut live = LiveInteractions::default();
            let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
            elsewhere.workspace_id = "other-workspace".into();
            live.open(
                vec![
                    interaction("one", InteractionActivity::Pending),
                    interaction("two", InteractionActivity::Pending),
                    interaction("three", InteractionActivity::Pending),
                    elsewhere,
                ],
                vec![],
            );
            live.toggle_workspace_scope();
            let scope = Some("workspace");
            assert_eq!(
                display(&live, scope),
                [("three", 0), ("two", 0), ("one", 0)]
            );

            // The current interaction can be absent or hidden by the scope.
            for current in ["missing", "elsewhere"] {
                live.rest();
                if backwards {
                    live.cursor_previous(current, scope);
                } else {
                    live.cursor_next(current, scope);
                }
                assert_eq!(live.cursor(current), "one");
                assert_eq!(live.pending(current).unwrap().id, "one");
                assert!(live.due(current).is_none());

                live.cursor_previous(current, scope);
                assert_eq!(live.cursor(current), "two");
                live.cursor_next(current, scope);
                assert_eq!(live.cursor(current), "one");
            }
        }

        let mut empty = LiveInteractions::default();
        empty.cursor_next("missing", None);
        empty.cursor_previous("missing", None);
        assert_eq!(empty.cursor("missing"), "missing");
        assert!(empty.pending("missing").is_none());
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
            .display_indices("", Some("workspace"))
            .into_iter()
            .map(|index| live.items[index].id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            ["other-running", "other-pending", "running", "pending"]
        );

        assert_eq!(
            live.next("other-running", "", Some("workspace"))
                .unwrap()
                .id,
            "other-pending"
        );
        assert_eq!(
            live.next("other-pending", "", Some("workspace"))
                .unwrap()
                .id,
            "running"
        );
        assert_eq!(
            live.previous("running", "", Some("workspace")).unwrap().id,
            "other-pending"
        );
        assert_eq!(
            live.previous("pending", "", Some("workspace")).unwrap().id,
            "running"
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

        // Display order: third | other-running, other-pending | running, pending
        assert_eq!(
            live.next_workspace("third", "", workspace).unwrap().id,
            "other-running"
        );
        assert_eq!(
            live.next_workspace("other-pending", "", workspace)
                .unwrap()
                .id,
            "running"
        );
        assert_eq!(
            live.next_workspace("other-running", "", workspace)
                .unwrap()
                .id,
            "running"
        );
        assert!(live.next_workspace("pending", "", workspace).is_none());

        // From the middle of a group, ctrl-k stops at that group's own first
        // entry before leaving it.
        assert_eq!(
            live.previous_workspace("other-pending", "", workspace)
                .unwrap()
                .id,
            "other-running"
        );
        assert_eq!(
            live.previous_workspace("other-running", "", workspace)
                .unwrap()
                .id,
            "third"
        );
        assert_eq!(
            live.previous_workspace("pending", "", workspace)
                .unwrap()
                .id,
            "running"
        );
        assert!(live.previous_workspace("third", "", workspace).is_none());
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

        live.cursor_previous_workspace("pending", workspace);
        live.cursor_previous_workspace("pending", workspace);

        assert_eq!(live.cursor("pending"), "third");
        assert!(live.due("pending").is_none());

        // Back the way it came, onto the interaction already on screen: two
        // groups crossed in each direction, and nothing to load at the end.
        live.cursor_next_workspace("pending", workspace);
        live.cursor_next_workspace("pending", workspace);
        assert_eq!(live.cursor("pending"), "pending");
        assert!(live.pending("pending").is_none());
    }

    /// The Workspace jump lands on the chosen Workspace's first entry, and
    /// widens a Workspace scope that would have hidden it.
    #[test]
    fn the_workspace_jump_lands_on_the_workspaces_first_entry() {
        let mut first = interaction("first", InteractionActivity::Running);
        first.workspace_id = "other-workspace".into();
        let mut second = interaction("second", InteractionActivity::Pending);
        second.workspace_id = "other-workspace".into();
        let mut live = LiveInteractions {
            only_current_workspace: true,
            ..Default::default()
        };
        live.open(
            vec![
                interaction("current", InteractionActivity::Pending),
                second,
                first,
            ],
            vec![],
        );

        assert!(live.cursor_to_workspace("other-workspace", "current", Some("workspace")));
        assert!(!live.only_current_workspace);
        assert_eq!(live.cursor("current"), "first");
        assert!(!live.cursor_to_workspace("missing", "current", Some("workspace")));
    }

    /// A stopped Interaction stays in its own Workspace's group, in item
    /// order read bottom up: its row says that it stopped, so it needs no
    /// place of its own.
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
            live.display_indices("", Some("workspace"))
                .into_iter()
                .map(|index| live.items[index].id.to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ordered(&live),
            ["other-running", "other-stopped", "stopped", "pending"]
        );

        // And in Workspace scope, where only this Workspace's entries are
        // listed, in item order read bottom up.
        live.toggle_workspace_scope();
        assert_eq!(ordered(&live), ["stopped", "pending"]);
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

        assert!(live
            .next_workspace("current", "", Some("workspace"))
            .is_none());
        assert!(live
            .previous_workspace("next", "", Some("workspace"))
            .is_none());
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
    fn first_live_in_workspace_returns_to_the_one_last_turned_to() {
        let mut idle = interaction("idle", InteractionActivity::Pending);
        idle.interacted_at_ms = 10;
        let mut running = interaction("running", InteractionActivity::Running);
        running.interacted_at_ms = 20;
        let mut stopped = interaction("stopped", InteractionActivity::Stopped);
        stopped.interacted_at_ms = 30;
        let untouched = interaction("untouched", InteractionActivity::Pending);
        let interactions = vec![idle, running, stopped, untouched];

        assert_eq!(
            first_live_in_workspace(&interactions, "workspace")
                .unwrap()
                .id,
            "running"
        );
    }

    #[test]
    fn completed_work_is_not_a_live_workspace_entry_even_when_its_agent_accepts() {
        for completed in [CompletionState::Completed, CompletionState::Abandoned] {
            let mut finished = interaction("finished", InteractionActivity::Pending);
            finished.completed = completed;
            finished.interacted_at_ms = 100;
            let unfinished = interaction("unfinished", InteractionActivity::Running);
            let items = [finished.clone(), unfinished];

            assert_eq!(
                first_live_in_workspace(&items, "workspace").unwrap().id,
                "unfinished"
            );
            assert_eq!(first_live(&items).unwrap().id, "unfinished");
            assert!(first_live_in_workspace(&[finished.clone()], "workspace").is_none());
            assert!(first_live(&[finished]).is_none());
        }
    }

    #[test]
    fn a_workspace_whose_interactions_all_stopped_has_no_live_entry() {
        let interactions = vec![
            interaction("stopped", InteractionActivity::Stopped),
            interaction("also-stopped", InteractionActivity::Stopped),
        ];

        assert!(first_live_in_workspace(&interactions, "workspace").is_none());
        assert!(first_live_in_workspace(&interactions, "unknown").is_none());
        assert!(first_live(&interactions).is_none());
    }

    #[test]
    fn first_live_looks_across_every_workspace() {
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other-workspace".into();
        let interactions = vec![
            interaction("stopped", InteractionActivity::Stopped),
            interaction("running", InteractionActivity::Running),
            elsewhere,
        ];

        assert_eq!(first_live(&interactions).unwrap().id, "elsewhere");
    }

    fn named(id: &str, name: &str) -> InteractionSummary {
        let mut interaction = interaction(id, InteractionActivity::Pending);
        interaction.name = Some(name.into());
        interaction
    }

    #[test]
    fn the_filter_narrows_the_list_by_name_or_tag_case_insensitively() {
        let mut tagged = named("three", "refactor");
        tagged.tags = vec!["Billing".into()];
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                named("one", "Fix checkout"),
                named("two", "write docs"),
                tagged,
            ],
            vec![],
        );

        live.start_filter();
        assert!(live.typing_filter());
        for character in "CHECK".chars() {
            live.type_filter(Edit::Push(character), "one", Some("workspace"));
        }
        assert_eq!(live.visible_indices("", Some("workspace")), vec![0]);

        live.clear_filter();
        live.start_filter();
        for character in "bill".chars() {
            live.type_filter(Edit::Push(character), "one", Some("workspace"));
        }
        assert_eq!(live.visible_indices("", Some("workspace")), vec![2]);

        // Enter keeps the filter but hands the keys back to the list.
        live.finish_filter();
        assert!(!live.typing_filter());
        assert_eq!(live.filter(), Some("bill"));

        live.clear_filter();
        assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);
    }

    /// A cursor left on a row the filter hid would act on something the
    /// operator can no longer see, so it moves to the first match.
    #[test]
    fn the_cursor_follows_the_filter_onto_the_first_match() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                named("one", "Fix checkout"),
                named("two", "write docs"),
                named("three", "more docs"),
            ],
            vec![],
        );

        live.start_filter();
        for character in "docs".chars() {
            live.type_filter(Edit::Push(character), "one", Some("workspace"));
        }
        assert_eq!(live.cursor("one"), "three");

        // Still matching as the term changes, so it stays put.
        live.cursor_next("one", Some("workspace"));
        live.type_filter(Edit::Backspace, "one", Some("workspace"));
        assert_eq!(live.cursor("one"), "two");
    }

    #[test]
    fn the_tag_filter_lists_rows_carrying_every_chosen_tag_and_outlasts_reopening() {
        let tagged = |id: &str, tags: &[&str]| {
            let mut interaction = named(id, id);
            interaction.tags = tags.iter().map(|tag| tag.to_string()).collect();
            interaction
        };
        let items = vec![
            tagged("one", &["bug"]),
            tagged("two", &["bug", "urgent"]),
            tagged("three", &["urgent"]),
        ];
        let mut live = LiveInteractions::default();
        live.open(items.clone(), vec![]);

        live.set_tag_filter(vec!["bug".into(), "urgent".into()], "one", None);
        assert_eq!(live.visible_indices("", None), vec![1]);
        assert_eq!(live.cursor("one"), "two");

        live.open(items, vec![]);
        assert_eq!(live.visible_indices("", None), vec![1]);

        live.set_tag_filter(Vec::new(), "one", None);
        assert_eq!(live.visible_indices("", None), vec![0, 1, 2]);
    }

    #[test]
    fn the_tag_filter_can_exclude_tags() {
        let tagged = |id: &str, tags: &[&str]| {
            let mut interaction = named(id, id);
            interaction.tags = tags.iter().map(|tag| tag.to_string()).collect();
            interaction
        };
        let mut live = LiveInteractions::default();
        live.open(
            vec![
                tagged("one", &["bug"]),
                tagged("two", &["bug", "wip"]),
                tagged("three", &["docs"]),
            ],
            vec![],
        );

        live.set_tag_filter(vec!["bug".into(), "-wip".into()], "one", None);
        assert_eq!(live.visible_indices("", None), vec![0]);
    }

    #[test]
    fn live_tags_are_sorted_unique_and_only_come_from_the_current_snapshot() {
        let mut one = named("one", "one");
        one.tags = vec!["urgent".into(), "bug".into()];
        let mut two = named("two", "two");
        two.tags = vec!["bug".into(), "docs".into()];
        let mut live = LiveInteractions::default();
        live.open(vec![one, two], vec![]);

        assert_eq!(live.live_tags(), ["bug", "docs", "urgent"]);
    }

    #[test]
    fn reopening_the_list_drops_the_filter() {
        let mut live = LiveInteractions::default();
        live.open(vec![named("one", "Fix checkout")], vec![]);
        live.start_filter();
        live.type_filter(Edit::Push('z'), "one", None);
        live.open(vec![named("one", "Fix checkout")], vec![]);
        assert!(live.filter().is_none());
        assert!(!live.typing_filter());
    }

    /// The fleet is listed several times a second and is usually exactly as it
    /// was. The event loop paints a frame only when something changed, so a
    /// snapshot identical to the last one must not read as news — otherwise
    /// the poll alone would keep the screen repainting forever.
    #[test]
    fn a_snapshot_identical_to_the_last_one_is_not_a_change() {
        let mut live = LiveInteractions::default();
        live.open(
            vec![interaction("one", InteractionActivity::Running)],
            vec![],
        );

        assert!(
            !live.refresh(vec![interaction("one", InteractionActivity::Running)]),
            "the same fleet, listed again"
        );
        assert!(
            live.refresh(vec![interaction("one", InteractionActivity::Stopped)]),
            "that interaction stopped"
        );
        assert!(
            live.refresh(vec![
                interaction("one", InteractionActivity::Stopped),
                interaction("two", InteractionActivity::Running),
            ]),
            "another interaction joined the fleet"
        );
    }

    #[test]
    fn a_row_flashes_when_something_new_happens_on_it() {
        let mut live = LiveInteractions::default();
        let mut talking = interaction("talking", InteractionActivity::Running);
        live.refresh(vec![
            talking.clone(),
            interaction("quiet", InteractionActivity::Running),
        ]);
        assert!(
            !live.flashing("talking") && !live.flashing("quiet"),
            "the fleet as this client found it is not news"
        );

        talking.last_message = Some("done".into());
        talking.events = 5;
        let mut busy = interaction("quiet", InteractionActivity::Running);
        busy.events = 9;
        live.refresh(vec![
            talking.clone(),
            busy,
            interaction("arrived", InteractionActivity::Running),
        ]);
        assert!(live.flashing("talking"), "its agent said something new");
        assert!(!live.flashing("quiet"), "events alone are not news");
        assert!(live.flashing("arrived"), "a new row is news");

        live.flashed.values_mut().for_each(|at| *at -= FLASH);
        assert!(live.expire_flashes());
        assert!(!live.flashing("talking"));
        assert!(!live.expire_flashes(), "nothing left to put out");

        talking.activity = InteractionActivity::Pending;
        live.refresh(vec![talking]);
        assert!(live.flashing("talking"), "it changed what it is doing");
    }
}
