//! The live Interactions navigator: the list of Interactions the server is
//! running, and the operator's place in it.
//!
//! Held apart from [`App`](crate::app::App) because none of it depends on the
//! Interaction currently on screen. [`crate::presentation::interactions`] renders it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

mod time_filter;

use styra_protocol::{InteractionQuery, InteractionSummary, WorkspaceSummary};
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
    /// Retained when reopening the list and switching interactions.
    time_filter: time_filter::TimeFilter,
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
    /// While the Workspace jump is open over the navigator, the Workspace it
    /// highlights (none if its query leaves nothing): only its rows are
    /// listed, and it stands in for the current Workspace — see
    /// [`Self::scope`]. `None` otherwise.
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
    /// client at the top before keeping the current interaction in view.
    pub scroll_offset: Option<usize>,
    /// Set while the view has been scrolled with the arrows, away from where
    /// the cursor would keep it; any cursor move brings it back.
    pub scrolled_away: bool,
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
        self.scrolled_away = false;
    }

    /// Scroll the list ten lines down without moving the cursor. The
    /// renderer stops it at the foot of the list.
    pub fn scroll_view_down(&mut self) {
        self.scroll_offset = Some(self.scroll_offset.unwrap_or(0).saturating_add(10));
        self.scrolled_away = true;
    }

    /// Scroll the list ten lines up without moving the cursor.
    pub fn scroll_view_up(&mut self) {
        self.scroll_offset = Some(self.scroll_offset.unwrap_or(0).saturating_sub(10));
        self.scrolled_away = true;
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

    /// The Session `id` was branched off, when it was branched off another.
    pub fn parent(&self, id: &str) -> Option<&str> {
        self.branch_origins
            .get(id)
            .map(|(parent, _, _)| parent.as_str())
    }

    /// Whether `id` was branched, directly or through branches between, off
    /// `ancestor`. Bounded so malformed cyclic origins end.
    pub fn descends_from(&self, id: &str, ancestor: &str) -> bool {
        let mut at = id;
        for _ in 0..self.branch_origins.len() {
            match self.parent(at) {
                Some(parent) if parent == ancestor => return true,
                Some(parent) => at = parent,
                None => return false,
            }
        }
        false
    }

    /// The Session at the top of `id`'s branch ancestry: `id` itself when it
    /// was not branched off another. Walks the persisted origins, so siblings
    /// whose shared source has left the listing still share a root. Bounded
    /// so malformed cyclic origins end.
    pub fn branch_root<'a>(&'a self, id: &'a str) -> &'a str {
        let mut root = id;
        for _ in 0..self.branch_origins.len() {
            match self.parent(root) {
                Some(parent) => root = parent,
                None => break,
            }
        }
        root
    }

    /// The listed Sessions branched off `id`, and off those in turn, as a
    /// `branched to` marker leading to `id` in `owner`'s conversation draws
    /// them: in listing order, each followed by its own, with how many
    /// generations below `id` it is, counting from 1. A Session already above
    /// one — `owner` included — is not listed beneath it again, so a cycle in
    /// the origins ends.
    pub fn branch_tree(&self, id: &str, owner: &str) -> Vec<(usize, &str)> {
        fn walk<'a>(
            items: &'a [InteractionSummary],
            id: &str,
            lineage: &mut Vec<String>,
            tree: &mut Vec<(usize, &'a str)>,
        ) {
            lineage.push(id.to_owned());
            for child in items {
                let branched_here = child
                    .origin
                    .as_ref()
                    .is_some_and(|origin| origin.session_id == id);
                if branched_here && !lineage.contains(&child.id) {
                    tree.push((lineage.len() - 1, child.id.as_str()));
                    walk(items, &child.id, lineage, tree);
                }
            }
            lineage.pop();
        }
        let mut tree = Vec::new();
        walk(&self.items, id, &mut vec![owner.to_owned()], &mut tree);
        tree
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

    /// How the listed Interactions divide between running, idle and stopped:
    /// the standing tally of the whole fleet. `Background` is work in
    /// flight, so it counts as running, and a dormant interaction is idle
    /// with no agent behind it yet, so it counts as idle.
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
                styra_protocol::InteractionActivity::Stopped if interaction.dormant() => {
                    counts.idle += 1
                }
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

    /// The Workspace the navigator's Workspace scope shows: the one the
    /// Workspace jump highlights while it is open, else `workspace_id`, the
    /// one on screen. Without this a Workspace scope would leave the jump
    /// nothing to show for any Workspace but the one on screen.
    fn scope<'a>(&'a self, workspace_id: Option<&'a str>) -> Option<&'a str> {
        match &self.jump_workspaces {
            Some(ids) => ids.first().map(String::as_str),
            None => workspace_id,
        }
    }

    /// The rows the navigator lists, in item order. `current`, the
    /// Interaction on screen, is listed whatever its state: completing,
    /// integrating or stopping it must not take it, and with it its place
    /// among its branches, out of the list while it is still being looked at.
    pub fn visible_indices(&self, current: &str, workspace_id: Option<&str>) -> Vec<usize> {
        let workspace_id = self.scope(workspace_id);
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
    /// All scope: the completion, stopped, time and tag filters stand, while the
    /// typed filter is one that opening clears.
    pub fn lists(&self, interaction: &InteractionSummary) -> bool {
        (self.show_completed || !interaction.completed.is_done())
            && (!self.hide_stopped || interaction.open())
            && self.in_time_window(interaction)
            && self.matches_tag_filter(interaction)
    }

    /// The Interactions the overview tiles, in the order it lays them out:
    /// every one still open — working, waiting on the operator, or dormant
    /// until the operator's next message — newest first by creation time.
    ///
    /// Shares the global Workspace scope and time filter with the navigator,
    /// while ignoring its search, tag and completion filters. Completed and
    /// stopped interactions are never tiled.
    pub fn overview_indices(&self, workspace_id: Option<&str>) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, interaction)| {
                interaction.open()
                    && !interaction.completed.is_done()
                    && self.in_time_window(interaction)
                    && self.in_workspace_scope(interaction, workspace_id)
            })
            .map(|(index, _)| index)
            .collect();
        indices.sort_by_key(|index| std::cmp::Reverse(created_at_ms(&self.items[*index])));
        indices
    }

    /// The visible indices in the order [`crate::presentation::interactions`] draws
    /// them: in All scope the entries are grouped under their Workspace
    /// heading, so j/k has to walk that order rather than the raw item order.
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
    /// completed, or out of scope — stands at the root.
    pub fn display_rows(&self, current: &str, workspace_id: Option<&str>) -> Vec<DisplayRow> {
        display_tree(&self.items, self.visible_indices(current, workspace_id))
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
        display_tree(&self.items, candidates)
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

    /// The next Interaction that is still live — see [`is_live`] — from `from` in display order, wrapping past the end of
    /// the list so repeated presses walk the whole working set and come back.
    ///
    /// Scoped as [`Self::next_idle_unseen`] is. Completed Interactions are not
    /// candidates, for the same reason the navigator hides them.
    pub fn next_live(&self, from: &str, workspace_id: Option<&str>) -> Option<InteractionSummary> {
        self.step_from(&self.step_order(workspace_id), from, is_live)
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
        .or_else(|| self.step_from(order, from, is_live))
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

    /// The entry under the cursor while it is not yet the one on screen:
    /// what a settled cursor loads.
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

    /// Move the cursor onto the next listed Interaction working in the same
    /// linked worktree as the cursor's, in display order, wrapping past the
    /// end so repeated presses cycle through them all. Its sources and
    /// branches count as any other. Nothing to land on — the cursor's in the
    /// main checkout, or alone in its worktree — leaves it where it is.
    pub fn cursor_to_next_in_worktree(
        &mut self,
        current: &str,
        workspace_id: Option<&str>,
    ) -> Option<InteractionSummary> {
        let from = self.cursor(current).to_owned();
        let at = self
            .items
            .iter()
            .find(|interaction| interaction.id == from)
            .filter(|interaction| !in_main_checkout(interaction, &self.workspaces))?
            .clone();
        let next = self.step_from(
            &self.display_indices(current, workspace_id),
            &from,
            |interaction| same_checkout(interaction, &at, &self.workspaces),
        )?;
        self.move_cursor_to(next.id.clone(), current);
        Some(next)
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
        // Wherever the view was scrolled to, it goes back to the cursor.
        self.scrolled_away = false;
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
            // Start from the top of the list when nothing is selected.
            .unwrap_or(0);
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
            .unwrap_or(0);
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
        let order = display_tree(&self.items, shown)
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
                    && (!self.hide_stopped || interaction.open())
            })
            .collect::<Vec<_>>();
        let live_alongside = alongside
            .iter()
            .copied()
            .filter(|(_, index)| self.items[*index].open())
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

/// Unfinished work that is still open: its agent can receive messages, or it
/// is dormant and the next message resumes one (see
/// [`InteractionSummary::open`]). Shared by startup selection, Workspace
/// entry, the live Workspace jump and `n`.
pub fn is_live(interaction: &InteractionSummary) -> bool {
    interaction.open() && !interaction.completed.is_done()
}

/// The Interaction to land on when entering `workspace_id`: the one an
/// operator most recently turned to, so arriving returns to the work they were
/// last on. Among those the server cannot tell apart — none turned to since it
/// started — one waiting on the operator outranks one mid-turn, and the newest
/// wins a tie. This also counts opening an Interaction, while the navigator
/// sorts by the most recent operator message.
///
/// Stopped and completed Interactions are not candidates; dormant ones are.
/// Entering a Workspace should land on work that can still be talked to; a
/// stopped Interaction is reached the same way it always was, through the
/// Session picker.
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
/// order the Workspaces themselves first appear, each group keeping the item
/// order: the groups the navigator lays out under its Workspace headings.
fn grouped_by_workspace(interactions: &[InteractionSummary], visible: Vec<usize>) -> Vec<usize> {
    let mut ordered = visible.clone();
    // The sort is stable, so each group keeps the item order.
    ordered.sort_by_cached_key(|index| {
        let workspace_id = &interactions[*index].workspace_id;
        visible
            .iter()
            .position(|other| &interactions[*other].workspace_id == workspace_id)
    });
    ordered
}

/// Whether `left` and `right` work in the same checkout of their Workspace:
/// both in its main checkout, or both in the same worktree.
pub(crate) fn same_checkout(
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

/// One entry of the list, placed where the navigator draws it: a branch
/// hangs beneath the entry it was branched from, whichever checkout either
/// works in. Rows name their worktree themselves, so where the work goes is
/// not part of the layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayRow {
    /// The entry's index in [`LiveInteractions::items`].
    pub index: usize,
    /// How many sources deep it hangs beneath the entry it was branched from.
    pub depth: usize,
    /// The Session this entry was branched from, when it does not hang beneath
    /// it because that one is not shown.
    pub branched_from: Option<String>,
}

/// `visible` laid out as [`DisplayRow`]s, in the order the navigator draws
/// them: each Workspace's entries together, and within each the branch tree,
/// each branch directly beneath the entry it was branched from, siblings in
/// the order they already had — the same tree the Session picker draws.
///
/// A source counts wherever it works in the Workspace. A branch of one not
/// shown — closed, hidden as completed, or out of scope — stands at the root
/// and names its source instead. A malformed loop of origins has no root, so
/// its entries are placed at the root in their existing order rather than
/// lost.
fn display_tree(interactions: &[InteractionSummary], visible: Vec<usize>) -> Vec<DisplayRow> {
    let ordered = grouped_by_workspace(interactions, visible);
    let mut out = Vec::with_capacity(ordered.len());
    for rows in ordered.chunk_by(|left, right| {
        interactions[*left].workspace_id == interactions[*right].workspace_id
    }) {
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
                branched_from: interactions[index]
                    .origin
                    .as_ref()
                    .filter(|_| depth == 0)
                    .map(|origin| origin.session_id.clone()),
            }
        }));
    }
    out
}

/// Whether `interaction` works in its Workspace's own checkout — on its main
/// branch, typically — rather than in a worktree made for it. Before Git has
/// been asked where it works, that is whether it works in the Workspace
/// directory: Styra makes its worktrees outside it. A Workspace whose own
/// directory is a linked worktree of a clone elsewhere still has that
/// checkout as its main one: it is the one the Workspace heading names.
pub fn in_main_checkout(interaction: &InteractionSummary, workspaces: &[WorkspaceSummary]) -> bool {
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.id == interaction.workspace_id);
    match &interaction.checkout {
        Some(checkout) => {
            !checkout.linked()
                || workspace.is_some_and(|workspace| {
                    workspace.git_repository.as_ref() == Some(&checkout.worktree)
                        || workspace.host_path.starts_with(&checkout.worktree)
                })
        }
        None => workspace
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
    // Latest operator message first, regardless of activity or stop reason.
    interactions.sort_by_key(|interaction| {
        (
            std::cmp::Reverse(last_message_or_creation_ms(interaction)),
            std::cmp::Reverse(created_at_ms(interaction)),
        )
    });
}

fn last_message_or_creation_ms(interaction: &InteractionSummary) -> u64 {
    if interaction.last_user_message_at_ms == 0 {
        created_at_ms(interaction)
    } else {
        interaction.last_user_message_at_ms
    }
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
pub(crate) mod tests;
