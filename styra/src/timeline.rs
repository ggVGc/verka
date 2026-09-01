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

use std::cell::Cell;
use styra_server::event::{AgentEvent, DetailBlock};
use styra_server::Contract;

/// One event in the list, with its fold state.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub event: AgentEvent,
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
    /// Whether this entry has anything to show beyond its one-line summary —
    /// the same test that decides whether the list shows a fold arrow next
    /// to it. `crate::ui`'s detail rendering always drops the body's first
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
    /// When true, the list contains only messages exchanged between the
    /// operator and the agent.
    pub conversation_only: bool,
    /// First visible item in the event list. Rendering updates this after it
    /// accounts for wrapped and expanded row heights, so navigation can keep
    /// a vim-like margin above and below the selection.
    pub list_offset: Cell<usize>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            // A fresh list is at its own tail, so it follows what arrives.
            follow: true,
            show_minor: false,
            conversation_only: true,
            list_offset: Cell::new(0),
        }
    }
}

impl Timeline {
    // --- Filters -------------------------------------------------------------

    pub(crate) fn event_is_visible(&self, event: &AgentEvent) -> bool {
        (self.show_minor || !event.is_minor())
            && (!self.conversation_only
                || matches!(
                    event,
                    AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. }
                ))
    }

    /// Whether an entry is shown in the list under the current filters.
    pub fn is_visible(&self, idx: usize) -> bool {
        self.event_is_visible(&self.entries[idx].event)
    }

    /// Whether an entry is one `j`/`k` should land on: visible, and carrying
    /// content worth previewing. Usually that means a fold arrow, but file
    /// events are always navigable because their preview includes the current
    /// file contents even when their event detail is only one line.
    fn is_navigable(&self, idx: usize) -> bool {
        self.is_visible(idx)
            && (self.entries[idx].has_detail()
                || matches!(self.entries[idx].event, AgentEvent::FileChanged { .. }))
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

    /// Toggle whether the list shows only operator/agent messages; see
    /// [`Self::toggle_minor`] for the return.
    pub fn toggle_conversation_only(&mut self) -> bool {
        self.conversation_only = !self.conversation_only;
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
    /// comes back as soon as the filter is turned off.
    pub fn entry_expanded(&self, idx: usize) -> bool {
        self.conversation_only || self.entries[idx].expanded
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

    /// The newest entry standing for a shell command, which is what the
    /// preview panel follows in [`crate::app::PreviewTarget::Command`].
    pub fn newest_command(&self) -> Option<&Entry> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.event.tag() == "shell")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline(events: Vec<AgentEvent>) -> Timeline {
        Timeline {
            entries: events
                .into_iter()
                .map(|event| Entry {
                    event,
                    expanded: false,
                    raw_index: None,
                    contract: None,
                })
                .collect(),
            ..Timeline::default()
        }
    }

    fn message(text: &str) -> AgentEvent {
        AgentEvent::AgentMessage {
            text: text.to_owned(),
        }
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
