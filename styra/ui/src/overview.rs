//! The overview: every active interaction laid out as a tile in a grid, so
//! the whole fleet can be watched at once instead of one row at a time.

use crate::{
    event_list::{BranchLink, EventListStatus},
    interactions::InteractionStatus,
    markdown::LinkDisplay,
    search::SearchView,
};
use std::borrow::Cow;
use styra_protocol::{
    event::{AgentEvent, Protocol},
    Contract,
};

pub struct OverviewTile<'a> {
    pub name: Cow<'a, str>,
    pub workspace: Cow<'a, str>,
    /// The provider, model and effort, as the launcher names them.
    pub selection: Cow<'a, str>,
    /// As [`crate::interactions::InteractionRow::Interaction::branch`].
    pub branch: Option<&'a str>,
    pub status: InteractionStatus,
    /// The line its own event list ends with, drawn under the messages as
    /// the list draws it under the last entry.
    pub tail: EventListStatus,
    /// The interaction this client is attached to.
    pub current: bool,
    pub newly_idle: bool,
    /// As [`crate::interactions::InteractionRow::Interaction::rate_limited`].
    pub rate_limited: Option<Cow<'a, str>>,
    pub uncommitted: bool,
    pub tags: &'a [String],
    /// How the interaction's provider presents its events, so its messages
    /// read as they do in its own event list.
    pub protocol: Protocol,
    /// The tail of the conversation, oldest first. As many as fit are shown,
    /// counted back from the latest.
    pub messages: Vec<OverviewMessage<'a>>,
}

/// One entry of a tile's conversation, drawn as the interaction's own event
/// list draws it.
pub struct OverviewMessage<'a> {
    /// A message from either side, or a branch marker.
    pub event: AgentEvent,
    /// The answer shape an operator message asked for, if it asked for one.
    pub contract: Option<Contract>,
    /// For a branch marker, where it leads, as [`EventEntry::branch`].
    pub branch: BranchLink<'a>,
}

pub struct OverviewView<'a> {
    pub tiles: Vec<OverviewTile<'a>>,
    pub running_only: bool,
    /// The name of the one Workspace the grid is narrowed to, or `None` when
    /// it shows every Workspace.
    pub workspace_scope: Option<Cow<'a, str>>,
    /// The tile under the cursor, an index into [`Self::tiles`].
    pub selected: usize,
    /// How links in the messages are drawn, as in the event list.
    pub links: LinkDisplay,
    /// The `/` search, marking matching words across every tile as it marks
    /// them in the event list. See [`crate::search`].
    pub search: SearchView<'a>,
}

/// How the grid was laid out, which the application needs to move the cursor
/// up and down between rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverviewFeedback {
    /// How many tiles each row holds, top to bottom.
    pub rows: Vec<usize>,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/overview.rs"]
mod rendering;
