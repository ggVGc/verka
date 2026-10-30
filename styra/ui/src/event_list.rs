//! The main event list: each entry a summary line that grows inline when
//! expanded, plus the empty-list start screen and the trailing status tail.

use crate::{
    chrome::PanelChrome,
    markdown::{EntryIndex, LinkDisplay},
    search::SearchView,
};
use std::time::Duration;
use styra_protocol::{
    event::{AgentEvent, Protocol},
    Contract, QueuedMessage,
};

/// Which row this is, and which state of it — see the host's event identity
/// (`tui::timeline::EventVersion`), which this mirrors.
///
/// A row is not identified by its position: the filters renumber those every
/// time they change. Nor by its event alone: a command completing or a task
/// reporting rewrites a row already on the list rather than appending a new
/// one. The pair is what stays true, and it is what [`entry_item`] keys its
/// cached rendering on.
///
/// Opaque here on purpose. This crate does not know how the host mints these
/// and only ever compares them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntryVersion {
    pub id: u64,
    pub revision: u32,
}

pub struct EventEntry<'a> {
    pub event: &'a AgentEvent,
    /// Which row, and which state of it. See [`EntryVersion`].
    pub version: EntryVersion,
    pub expanded: bool,
    pub has_detail: bool,
    pub contract: Option<&'a Contract>,
    pub selected: bool,
    /// The Markdown link within this entry to mark, if link navigation is on it.
    pub link_highlight: Option<EntryIndex>,
    /// For an [`AgentEvent::Branched`] entry, what the host knows of the
    /// other session now. See [`BranchLink`].
    pub branch: BranchLink<'a>,
    /// Provider transition recorded by the linked branch's origin.
    pub branch_provider_switch: Option<(
        styra_protocol::agent::Provider,
        styra_protocol::agent::Provider,
    )>,
    /// The entry precedes a branched Session's branch point: history copied
    /// from its source rather than its own, drawn faded so the two read apart.
    pub inherited: bool,
}

/// The session an [`AgentEvent::Branched`] marker leads to, as the host's
/// live roster has it now. Every field is `None` when the host has no row for
/// that id (it was never listed, or has since been forgotten), and for an
/// entry that is not a marker at all.
///
/// The event's own `name` field is not used for display: it is a snapshot
/// taken when the marker was written, and a rename afterward would leave it
/// stale. The same goes for the rest, which the event never carried.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchLink<'a> {
    /// The operator-facing name.
    pub name: Option<&'a str>,
    /// What it is doing, drawn as its row in the navigator draws it.
    pub state: Option<crate::interactions::InteractionState>,
    /// The agent's last message, already one line, as the navigator quotes
    /// it beneath the row.
    pub message: Option<&'a str>,
}

pub enum EventListStatus {
    Pending,
    Running {
        elapsed: Duration,
        quiet: Option<Duration>,
        events: usize,
    },
    /// `reason` says what left the session idle, where saying it adds
    /// something: an interrupted turn and a finished one leave the same
    /// screen otherwise.
    Idle {
        reason: Option<String>,
        /// The turn that left it idle failed, so the line is drawn as an
        /// error rather than as the green of a turn that got done.
        failed: bool,
    },
    Background {
        elapsed: Duration,
    },
    Stopped {
        elapsed: Duration,
        reason: Option<String>,
        tone: crate::chrome::StopTone,
    },
    Ended,
}

/// Listed Interactions by activity, shown as a tight `running/idle/stopped`
/// tally: one glance at the fleet without opening the navigator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityCounts {
    pub running: usize,
    pub idle: usize,
    pub stopped: usize,
}

pub struct EventListView<'a> {
    pub chrome: PanelChrome,
    pub entries: Vec<EventEntry<'a>>,
    /// The whole fleet's tally, on this pane's bottom border — see
    /// [`ActivityCounts`].
    pub activity: ActivityCounts,
    /// The list shows every event rather than the default conversation only,
    /// which the bottom border then says, along with `show_minor`.
    pub all_events: bool,
    /// Whether minor lifecycle events are among them; see [`all_events_title`].
    pub show_minor: bool,
    /// The interaction being shown has stopped working and left uncommitted
    /// changes in its repository — see [`crate::chrome::uncommitted_title`].
    pub uncommitted_changes: bool,
    pub usage: Option<(u64, u64, u64)>,
    pub can_configure_launch: bool,
    pub selection_name: String,
    pub requested_offset: usize,
    pub requested_row_offset: usize,
    /// Signed rendered-row movement to apply to the requested anchor.
    pub scroll_delta: i32,
    /// Keep the selected entry on screen. Explicit viewport scrolling turns
    /// this off so the view can move independently of the selection.
    pub anchor_selection: bool,
    /// With `anchor_selection`, leave the viewport at the requested anchor
    /// while the selected entry's summary line is on screen, instead of
    /// re-applying scrolloff: moving between entries does not scroll.
    pub keep_viewport: bool,
    /// Whether an explicit move toward older entries permits scrolloff to
    /// reveal rows above the current viewport anchor. Live content updates do
    /// not set this: a row changing height must not look like navigation.
    pub moved_backward: bool,
    /// On entering an interaction, place its selected newest entry close to
    /// the top so its immediate history remains readable below it. This is a
    /// rendered-line count, not an item count: wrapped entries count as the
    /// lines they occupy.
    pub max_lines_above_selection: Option<usize>,
    pub protocol: Protocol,
    pub links: LinkDisplay,
    /// The `/` search: what has been typed, and whether the prompt still has
    /// the keys. See [`crate::search`].
    pub search: SearchView<'a>,
    pub status: EventListStatus,
    /// Messages written while the agent was busy, oldest first. They are
    /// listed under the status tail, where they will land once sent.
    pub queued: &'a [QueuedMessage],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventListFeedback {
    pub effective_offset: usize,
    pub effective_row_offset: usize,
}

pub struct EntryLogView<'a> {
    pub entries: Vec<EventEntry<'a>>,
    /// Whether this pane, rather than the event list above it, is holding the
    /// navigation keys. A focused pane wears the active border and names the
    /// key that hands them back; only then does any of its rows carry the
    /// selection, so the screen never shows two cursors at once.
    pub focused: bool,
    pub requested_scroll: u16,
    pub protocol: Protocol,
    pub links: LinkDisplay,
    /// The keys that move focus between the pane and the list, and that close
    /// it, as the client has them bound.
    pub focus_key: String,
    pub close_key: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryLogFeedback {
    pub limit: u16,
    pub effective_scroll: u16,
}

/// Where an entry's header is displayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryContext {
    /// Includes summary text and an expansion marker for collapsed entries
    /// with detail. Expanded entries reserve the marker's space.
    EventList,
    /// The body already shows the full content, so the header omits summary
    /// text and expansion controls.
    Preview,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/event_list.rs"]
mod rendering;
