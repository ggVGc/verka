//! Side-panel and full-screen previews for one timeline entry.

use crate::{
    event_list::EventEntry,
    markdown::{EntryIndex, LinkDisplay},
};
use std::borrow::Cow;
use styra_protocol::event::{AgentEvent, Protocol};

/// Stands for the Workspace root at the start of a changed file's path.
pub const WORKSPACE_SHORTHAND: &str = "ws:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewTarget {
    Selection,
    Command,
}

pub struct PreviewView<'a> {
    pub entry: Option<EventEntry<'a>>,
    /// The entry itself, when it is a file change: drawn as one, from a diff
    /// the host may have placed in its file (see [`ChangeView::diff`]).
    pub entry_change: Option<ChangeView<'a>>,
    /// The absolute paths the Workspace is known by — inside the agent's
    /// sandbox and on this host. A changed file's path under one of them is
    /// drawn from the Workspace on, after [`WORKSPACE_SHORTHAND`].
    pub workspace_roots: Vec<String>,
    pub protocol: Protocol,
    pub target: PreviewTarget,
    pub links: LinkDisplay,
    pub link_highlight: Option<EntryIndex>,
    /// The log of the Session a `branch` marker names, shown under the marker
    /// in place of its own detail: the marker only says where the other side
    /// is, and what happened there is what the operator wants to read.
    pub branch_log: Option<BranchLog<'a>>,
    /// The local destination being inspected while Markdown-link navigation is
    /// active. This supersedes the conversation entry in the preview pane.
    pub file_target: Option<FileTarget>,
    /// The selected Styra answer, parsed under its original request contract.
    pub answer_target: Option<styra_protocol::Answer>,
    pub requested_scroll: u16,
    pub fullscreen: bool,
}

/// One file change as the preview draws it.
pub struct ChangeView<'a> {
    /// The files it touched. Empty for a diff that names its own files in its
    /// headers.
    pub paths: &'a [String],
    /// The diff to draw, or `None` when the provider reported none. It may
    /// differ from what the provider sent: a snippet that gave no position
    /// can have had one found for it, so its lines can be numbered.
    pub diff: Option<Cow<'a, str>>,
}

/// Whether the linked Session's log has been read yet. Loading is a round
/// trip to the server, so an unread log and an empty one must not read the
/// same.
#[derive(Clone, Copy, Debug)]
pub enum BranchLog<'a> {
    Loading,
    /// Its events, presented as its own provider presents them.
    Ready {
        events: &'a [AgentEvent],
        protocol: Protocol,
    },
    Failed(&'a str),
}

pub struct FileTarget {
    /// The destination exactly as the link wrote it, including any position.
    pub location: String,
    pub content: FileTargetContent,
    /// A one-based source line from the destination, if one was supplied.
    pub line: Option<u32>,
}

pub enum FileTargetContent {
    Empty,
    Ready(String),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewFeedback {
    pub limit: u16,
    pub effective_scroll: u16,
    /// How many lines the preview shows at once.
    pub viewport: u16,
}

/// One message, before terminal styling and wrapping.
pub struct MessageView<'a> {
    pub event: &'a AgentEvent,
    pub protocol: Protocol,
    pub links: LinkDisplay,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/preview.rs"]
mod rendering;
