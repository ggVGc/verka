//! The event list: the rows, which one is selected, and which of them the
//! current filters show.
//!
//! Navigation is the whole of it. Every move is expressed as "the nearest
//! index a [`Step`] can land on", so the two step sizes (`j`/`k` over
//! everything visible, `J`/`K` over rows with something to preview) share one
//! implementation, and neither can land on a row the filters are hiding.
//!
//! Moves report whether the selection actually changed, because what follows
//! from that — resetting the preview scroll — belongs to state this module
//! deliberately does not hold. [`crate::app::App`] carries a [`Timeline`] and
//! joins the two.

use std::sync::atomic::{AtomicU64, Ordering};

use styra_protocol::event::{AgentEvent, DetailBlock};
use styra_protocol::Contract;

/// A stable handle to one row of the list.
///
/// Minted when the entry is appended and never reused, so it keeps naming the
/// same row as entries arrive above and below it — unlike a position, which
/// every filter change reinterprets. A renderer can therefore hold one across
/// frames and know what it refers to.
///
/// Unique across the whole process, not just one list: the renderer's row
/// cache outlives any one interaction, so an id that restarted with each
/// [`Timeline`] would let a new interaction's rows be drawn from an earlier
/// one's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(u64);

impl EventId {
    /// An id no other entry in this process has or will have.
    fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The bare number, for handing this identity to a renderer that keys
    /// cached work on it without depending on this crate.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Identity of one *state* of an entry: which row, and how many times the
/// event on it has been replaced since it arrived.
///
/// A row is not immutable. A command completing, a tool finishing, a task
/// reporting progress, and a run of thinking ticking over all rewrite a row
/// that is already on the list rather than adding a second one — see
/// [`crate::ingest`]. The id alone would therefore go stale; paired with
/// [`Entry::revision`] it identifies exactly one version of one row, which is
/// what anything caching per-entry work has to key on to stay correct.
///
/// No renderer keys on this yet — the event list still rebuilds every row on
/// every frame, and `styra_ui`'s cache works on block text instead (see
/// `styra_ui::render_cache`). The identity comes first because it is the part
/// that has to be right: a cache built on a version that a rewrite can slip
/// past would show a finished command as still running, and the bug would
/// surface as a rare visual glitch rather than a test failure.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventVersion {
    pub id: EventId,
    pub revision: u32,
}

/// One event in the list, with its fold state.
///
/// `event` is private because replacing it has to bump [`Self::revision`]:
/// identity that a mutation can slip past is worse than none, since a stale
/// version reads as a valid one. [`Self::event`] hands out the read access
/// that nearly every caller wants; [`Self::set_event`] and
/// [`Self::update_event`] are the only ways to change it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    id: EventId,
    revision: u32,
    event: AgentEvent,
    pub expanded: bool,
    /// The index into [`crate::app::App::raw`] of the wire line this entry was
    /// decoded from, if known — lets the raw view jump straight to the line
    /// behind an entry instead of making the operator hunt for it.
    /// Best-effort: an operator's own message is echoed as an entry before its
    /// encoded wire line is journaled, so for it this points at whatever line
    /// came just before instead of its own.
    pub raw_index: Option<usize>,
    /// The shape this turn asked its reply to come back in, for an operator
    /// message the server framed. Set at ingest, where the framing is
    /// recognised and removed from `event`, so the list shows the message as
    /// it was written and still says what was asked of it. The verbatim line
    /// including the framing remains in the raw view.
    pub contract: Option<Contract>,
}

impl Entry {
    /// What this row shows.
    pub fn event(&self) -> &AgentEvent {
        &self.event
    }

    /// This row's stable handle, which outlives every rewrite of its event.
    #[allow(dead_code)]
    pub fn id(&self) -> EventId {
        self.id
    }

    /// This row together with how many times it has been rewritten — the
    /// thing to key cached rendering on. See [`EventVersion`], which also says
    /// why this has no caller outside the tests yet.
    #[allow(dead_code)]
    pub fn version(&self) -> EventVersion {
        EventVersion {
            id: self.id,
            revision: self.revision,
        }
    }

    /// Replace what this row shows, marking it as a new version.
    pub fn set_event(&mut self, event: AgentEvent) {
        self.event = event;
        self.revision = self.revision.saturating_add(1);
    }

    /// Edit what this row shows in place, marking it as a new version.
    ///
    /// For the updates that merge into the event already there rather than
    /// replacing it wholesale — a task ending keeps the summary an earlier
    /// report gave it, a thinking line accumulates its token count.
    pub fn update_event(&mut self, edit: impl FnOnce(&mut AgentEvent)) {
        edit(&mut self.event);
        self.revision = self.revision.saturating_add(1);
    }

    /// Whether this entry has anything to show beyond its one-line summary —
    /// the same test that decides whether the list shows a fold arrow next
    /// to it. `crate::presentation`'s detail rendering always drops the body's first
    /// line (it invariably restates the summary — the command, the
    /// message's first line, ...), so one line of detail alone doesn't
    /// count; this mirrors that exactly rather than checking the raw,
    /// undropped `AgentEvent::detail()` output. A summary truncated with an
    /// ellipsis also counts, even with no extra detail lines, since its full
    /// text is only reachable by expanding.
    pub fn has_detail(&self) -> bool {
        detail_line_count(&self.event) > 0 || self.event.summary().ends_with('…')
    }
}

/// Total line count across an event's detail blocks, splitting multi-line
/// text and code the same way the list's detail rendering does, minus the
/// one line that rendering always drops as a restatement of the summary.
fn detail_line_count(event: &AgentEvent) -> usize {
    let count: usize = event
        .detail()
        .iter()
        .map(|block| match block {
            DetailBlock::Text(text) | DetailBlock::Code { text, .. } => text.lines().count(),
        })
        .sum();
    count.saturating_sub(1)
}

/// How far one navigation key moves: the two step sizes the event list offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Every visible entry, one at a time (`J`/`K`).
    Line,
    /// Only entries with something past their summary, so the keys that drive
    /// the preview never stop on a row with nothing to preview (`j`/`k`).
    WithDetail,
}

/// The list of what the agent has done, and the operator's place in it.
pub struct Timeline {
    pub entries: Vec<Entry>,
    pub selected: usize,
    /// When true, the selection tracks the newest entry as events arrive.
    pub follow: bool,
    /// When false, minor lifecycle events (thread/turn/usage) are hidden from
    /// the list and skipped by navigation.
    pub show_minor: bool,
    /// When false — the default — the list contains only messages exchanged
    /// between the operator and the agent; when true it shows every event
    /// (minor ones still subject to `show_minor`).
    pub all_events: bool,
    /// First visible item in the event list. Rendering updates this after it
    /// accounts for wrapped and expanded row heights, so navigation can keep
    /// a vim-like margin above and below the selection.
    pub list_offset: usize,
    /// Rendered row within [`Self::list_offset`] at the top of the viewport.
    pub list_row_offset: usize,
    /// Signed rendered-row movement waiting for the renderer, which alone
    /// knows the wrapped heights needed to resolve it.
    pub list_scroll_delta: i32,
    /// Whether rendering should bring the selection back into view. Explicit
    /// viewport scrolling clears this until selection navigation resumes.
    pub anchor_selection: bool,
    /// Selection index used for the last rendered frame. Comparing it with
    /// `selected` distinguishes deliberate upward navigation from a live row
    /// merely changing height between frames.
    pub rendered_selection: Option<usize>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            // A fresh list is at its own tail, so it follows what arrives.
            follow: true,
            show_minor: false,
            all_events: false,
            list_offset: 0,
            list_row_offset: 0,
            list_scroll_delta: 0,
            anchor_selection: true,
            rendered_selection: None,
        }
    }
}

impl Timeline {
    /// Scroll the rendered interaction by ten text rows without changing
    /// which entry is selected.
    pub fn scroll_view_down(&mut self) {
        self.list_scroll_delta = self.list_scroll_delta.saturating_add(10);
        self.anchor_selection = false;
    }

    /// Scroll the rendered interaction toward its beginning without changing
    /// which entry is selected.
    pub fn scroll_view_up(&mut self) {
        self.list_scroll_delta = self.list_scroll_delta.saturating_sub(10);
        self.anchor_selection = false;
    }

    /// Append a row, giving it an identity no other row has or will have.
    ///
    /// The only way to build an [`Entry`]: ids come from here, so an entry
    /// cannot exist without one.
    pub fn push(
        &mut self,
        event: AgentEvent,
        expanded: bool,
        raw_index: Option<usize>,
        contract: Option<Contract>,
    ) {
        self.entries.push(Entry {
            id: EventId::mint(),
            revision: 0,
            event,
            expanded,
            raw_index,
            contract,
        });
    }

    // --- Filters -------------------------------------------------------------

    pub(crate) fn event_is_visible(&self, event: &AgentEvent) -> bool {
        (self.show_minor || !event.is_minor()) && (self.all_events || event.is_conversation())
    }

    /// Whether an entry is shown in the list under the current filters.
    pub fn is_visible(&self, idx: usize) -> bool {
        self.event_is_visible(self.entries[idx].event())
    }

    /// Whether an entry is one `j`/`k` should land on: visible, and carrying
    /// content worth previewing. Usually that means a fold arrow, but file
    /// events are always navigable because their preview includes the current
    /// file contents even when their event detail is only one line.
    fn is_navigable(&self, idx: usize) -> bool {
        self.is_visible(idx)
            && (self.entries[idx].has_detail()
                || matches!(*self.entries[idx].event(), AgentEvent::FileChanged { .. }))
    }

    fn reaches(&self, idx: usize, step: Step) -> bool {
        match step {
            Step::Line => self.is_visible(idx),
            Step::WithDetail => self.is_navigable(idx),
        }
    }

    /// Toggle whether minor lifecycle events (thread/turn/usage) are shown.
    /// Returns whether the selection had to move to stay on a visible row.
    pub fn toggle_minor(&mut self) -> bool {
        self.show_minor = !self.show_minor;
        self.reconcile_selection()
    }

    /// Toggle whether the list shows every event or only operator/agent
    /// messages; see [`Self::toggle_minor`] for the return.
    pub fn toggle_all_events(&mut self) -> bool {
        self.all_events = !self.all_events;
        self.reconcile_selection()
    }

    /// Pull the selection back onto a visible row after a filter change hid
    /// the one it was on.
    fn reconcile_selection(&mut self) -> bool {
        if self.entries.is_empty() || self.is_visible(self.selected) {
            return false;
        }
        match self
            .seek_back(self.selected, Step::Line)
            .or_else(|| self.seek_forward(self.selected, Step::Line))
        {
            Some(idx) => {
                self.selected = idx;
                true
            }
            None => false,
        }
    }

    // --- Navigation ----------------------------------------------------------
    //
    // Each of these returns whether the selection moved, which is what tells
    // the caller to reset the preview scroll.

    /// The nearest index `step` can land on at or after `from`, if any.
    pub(crate) fn seek_forward(&self, from: usize, step: Step) -> Option<usize> {
        (from..self.entries.len()).find(|&i| self.reaches(i, step))
    }

    /// The nearest index `step` can land on at or before `from`, if any.
    fn seek_back(&self, from: usize, step: Step) -> Option<usize> {
        (0..=from).rev().find(|&i| self.reaches(i, step))
    }

    /// Move towards the tail, re-enabling follow only once the selection
    /// reaches the last entry this step can land on.
    pub(crate) fn select_forward(&mut self, step: Step) -> bool {
        let moved = match self.seek_forward(self.selected + 1, step) {
            Some(next) => {
                self.selected = next;
                true
            }
            None => false,
        };
        self.follow =
            !self.entries.is_empty() && self.seek_forward(self.selected + 1, step).is_none();
        moved
    }

    /// Move towards the start. Leaving the tail always pins the view.
    pub(crate) fn select_backward(&mut self, step: Step) -> bool {
        let moved = match self
            .selected
            .checked_sub(1)
            .and_then(|from| self.seek_back(from, step))
        {
            Some(prev) => {
                self.selected = prev;
                true
            }
            None => false,
        };
        self.follow = false;
        moved
    }

    pub fn select_first(&mut self) -> bool {
        let moved = match self.seek_forward(0, Step::Line) {
            Some(first) => {
                self.selected = first;
                true
            }
            None => false,
        };
        self.follow =
            !self.entries.is_empty() && self.seek_forward(self.selected + 1, Step::Line).is_none();
        moved
    }

    pub fn select_last(&mut self) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let moved = match self.seek_back(self.entries.len() - 1, Step::Line) {
            Some(last) => {
                self.selected = last;
                true
            }
            None => false,
        };
        self.follow = true;
        moved
    }

    /// Put the selection on the last entry, where following leaves it.
    pub(crate) fn select_tail(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }

    // --- Expansion -----------------------------------------------------------

    /// Whether an entry renders expanded. Conversation-only mode shows every
    /// remaining line in full: with tool activity filtered away, what is left
    /// is prose meant to be read, and folding it would leave the list nearly
    /// empty. The per-entry flag is left untouched, so the previous folding
    /// comes back as soon as all events are shown.
    pub fn entry_expanded(&self, idx: usize) -> bool {
        !self.all_events || self.entries[idx].expanded
    }

    pub fn toggle_expand(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.selected) {
            entry.expanded = !entry.expanded;
        }
    }

    pub fn expand_only_selected(&mut self) {
        for (index, entry) in self.entries.iter_mut().enumerate() {
            entry.expanded = index == self.selected;
        }
    }

    pub fn expand_all(&mut self) {
        for entry in &mut self.entries {
            entry.expanded = true;
        }
    }

    pub fn collapse_all(&mut self) {
        for entry in &mut self.entries {
            entry.expanded = false;
        }
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// The stretch of the interaction log the selected entry stands for: that
    /// entry's own conversation message and everything the agent did under it,
    /// up to the next message.
    ///
    /// This is what the conversation-only filter hides. With it on, the list
    /// shows one row per message and the tool calls between two of them are off
    /// screen entirely; this says which entries those are, so a view can show
    /// the work behind the message being read without the operator having to
    /// turn the filter off and find the place again.
    ///
    /// A selection that is not itself a message — reachable with the filter off
    /// — belongs to the message it came after, so the span is the same one
    /// whichever of its rows the cursor happens to sit on. Before the session's
    /// first message there is none to anchor to, so the span opens at the start
    /// of the log.
    ///
    /// Empty only when the log itself is.
    pub fn conversation_span(&self) -> std::ops::Range<usize> {
        if self.entries.is_empty() {
            return 0..0;
        }
        let selected = self.selected.min(self.entries.len() - 1);
        let start = (0..=selected)
            .rev()
            .find(|&idx| self.entries[idx].event().is_conversation())
            .unwrap_or(0);
        let end = (start + 1..self.entries.len())
            .find(|&idx| self.entries[idx].event().is_conversation())
            .unwrap_or(self.entries.len());
        start..end
    }

    /// The file changes made during the selected message's stretch — the
    /// entries [`Self::conversation_span`] scopes it to, so the same work the
    /// entry-log pane lists. This is what the preview shows for a message: its
    /// text is already on the list, and what the agent changed under it is not.
    ///
    /// Codex follows each file-change item with a snapshot of the whole turn's
    /// diff so far. Where the stretch has one, the newest stands for them all:
    /// showing the items as well would repeat every change, and an item's own
    /// diff is best-effort where the snapshot is not.
    pub fn conversation_changes(&self) -> Vec<&AgentEvent> {
        let events = self.entries[self.conversation_span()]
            .iter()
            .map(Entry::event);
        if let Some(snapshot) = events
            .clone()
            .rev()
            .find(|event| matches!(event, AgentEvent::DiffUpdated { .. }))
        {
            return vec![snapshot];
        }
        events
            .filter(|event| matches!(event, AgentEvent::FileChanged { .. }))
            .collect()
    }

    /// The newest entry standing for a shell command, which is what the
    /// preview panel follows in [`crate::app::PreviewTarget::Command`].
    pub fn newest_command(&self) -> Option<&Entry> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.event().tag() == "shell")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline(events: Vec<AgentEvent>) -> Timeline {
        let mut timeline = Timeline::default();
        for event in events {
            timeline.push(event, false, None, None);
        }
        timeline
    }

    fn message(text: &str) -> AgentEvent {
        AgentEvent::AgentMessage {
            text: text.to_owned(),
        }
    }

    /// The point of an id: it names a row, not a position. Everything a
    /// renderer might key on it survives entries arriving around it.
    #[test]
    fn an_id_names_one_row_for_the_life_of_the_list() {
        let mut list = timeline(vec![message("first"), message("second")]);
        let ids: Vec<EventId> = list.entries.iter().map(Entry::id).collect();
        assert_ne!(ids[0], ids[1], "two rows must not share an id");

        list.push(message("third"), false, None, None);
        let after: Vec<EventId> = list.entries.iter().map(Entry::id).collect();
        assert_eq!(&after[..2], &ids[..], "appending must not renumber rows");
        assert!(
            !ids.contains(&after[2]),
            "a new row must not reuse an id already handed out"
        );
    }

    /// The revision is what keeps the id honest: a row that is rewritten in
    /// place is the same row showing something else, and anything holding
    /// rendered work for it has to be able to tell.
    #[test]
    fn rewriting_a_row_keeps_its_id_and_advances_its_revision() {
        let mut list = timeline(vec![message("thinking")]);
        let before = list.entries[0].version();

        list.entries[0].set_event(message("done"));
        let after = list.entries[0].version();

        assert_eq!(after.id, before.id, "a rewrite is the same row");
        assert_ne!(after, before, "a rewrite is a different version of it");

        list.entries[0].update_event(|event| {
            if let AgentEvent::AgentMessage { text } = event {
                text.push('!');
            }
        });
        assert_ne!(
            list.entries[0].version(),
            after,
            "an in-place edit is a rewrite too"
        );
    }

    /// Folding is the operator's view of a row, not a change to what it
    /// shows — and the two are keyed separately, so a fold must not spend a
    /// revision.
    #[test]
    fn folding_a_row_does_not_make_it_a_new_version() {
        let mut list = timeline(vec![message("first")]);
        let before = list.entries[0].version();
        list.toggle_expand();
        assert_eq!(list.entries[0].version(), before);
    }

    /// The caller resets the preview scroll on a move, so a key that could not
    /// move must say so rather than reporting every press as a move.
    #[test]
    fn a_move_reports_whether_the_selection_actually_changed() {
        let mut list = timeline(vec![message("one"), message("two")]);
        list.selected = 0;
        assert!(list.select_forward(Step::Line));
        assert_eq!(list.selected, 1);
        // At the tail there is nowhere to go, and following resumes.
        assert!(!list.select_forward(Step::Line));
        assert!(list.follow);

        assert!(list.select_backward(Step::Line));
        assert_eq!(list.selected, 0);
        // Leaving the tail pins the view whether or not the move landed.
        assert!(!list.follow);
        assert!(!list.select_backward(Step::Line));
    }

    fn shell(command: &str) -> AgentEvent {
        AgentEvent::CommandStarted {
            command: command.to_owned(),
        }
    }

    fn user(text: &str) -> AgentEvent {
        AgentEvent::UserMessage {
            text: text.to_owned(),
        }
    }

    /// The span is the message and the work under it, which is exactly what
    /// the conversation-only filter leaves out of the list.
    #[test]
    fn a_conversation_entrys_span_runs_up_to_the_next_message() {
        let mut list = timeline(vec![
            user("ask"),
            shell("one"),
            shell("two"),
            message("answer"),
            shell("three"),
        ]);

        list.selected = 0;
        assert_eq!(list.conversation_span(), 0..3);

        // The next message opens its own span, which runs to the end of the
        // log while the turn is still going.
        list.selected = 3;
        assert_eq!(list.conversation_span(), 3..5);
    }

    /// With the filter off the cursor can rest on a tool row. It belongs to
    /// the message it came after, so it names that message's span rather than
    /// one starting at itself.
    #[test]
    fn a_selection_between_messages_names_the_span_it_sits_in() {
        let mut list = timeline(vec![user("ask"), shell("one"), shell("two")]);
        list.selected = 2;
        assert_eq!(list.conversation_span(), 0..3);
    }

    /// An agent that works before saying anything leaves entries with no
    /// message to anchor to; they are still part of the log, so the span opens
    /// at its start rather than coming back empty.
    #[test]
    fn work_before_the_first_message_belongs_to_the_start_of_the_log() {
        let mut list = timeline(vec![shell("one"), shell("two"), message("hello")]);
        list.selected = 1;
        assert_eq!(list.conversation_span(), 0..2);

        assert_eq!(
            timeline(Vec::new()).conversation_span(),
            0..0,
            "empty only when the log is"
        );
    }

    /// A filter that hides the selected row has to pull the selection onto one
    /// that is still shown — and say that it did, since the preview it was
    /// showing is no longer the selected entry's.
    #[test]
    fn a_filter_that_hides_the_selection_moves_it_to_a_visible_row() {
        let mut list = timeline(vec![
            message("kept"),
            AgentEvent::TurnStarted,
            AgentEvent::TurnStarted,
        ]);
        list.show_minor = true;
        list.selected = 2;

        assert!(list.toggle_minor());
        assert_eq!(list.selected, 0, "back to the nearest visible row");
        assert!(list.is_visible(list.selected));

        // With the selection already visible there is nothing to move.
        assert!(!list.toggle_minor());
        assert_eq!(list.selected, 0);
    }
}
