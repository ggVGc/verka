//! The live-interaction navigator embedded above the event timeline.

use crate::chrome::StopTone;
use std::borrow::Cow;
use styra_protocol::CompletionState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InteractionStatus {
    Pending,
    Running {
        events: usize,
    },
    Idle,
    /// Idle because its last turn failed: the agent survived and still takes
    /// messages, but the turn it was given did not get done, so the row must
    /// not read as an ordinary "your turn" idle.
    IdleFailed,
    Background,
    Stopped(StopTone),
    Error,
    Ended,
}

/// How a row is tied to the cursor's by a branch drawn apart from its source:
/// one the rows' nesting does not show, so the row is marked instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BranchLink {
    #[default]
    None,
    /// The cursor's interaction was branched from this one.
    Source,
    /// This one was branched from the cursor's interaction.
    Branch,
}

/// What an interaction is doing, as its row in the navigator says it: the
/// status block and the labels after the name. For drawing that state where
/// the interaction is mentioned outside the navigator, the same way.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InteractionState {
    pub status: InteractionStatus,
    /// See [`InteractionRow::Interaction`]'s `stop_reason`.
    pub stop_reason: Option<String>,
    /// See [`InteractionRow::Interaction`]'s `rate_limited`.
    pub rate_limited: Option<String>,
    pub newly_idle: bool,
    pub completion: CompletionState,
}

pub enum InteractionRow<'a> {
    Workspace(Cow<'a, str>),
    /// A worktree heading the rows of the interactions working in it, which
    /// follow it one `level` further in, and then the headings of the
    /// worktrees integrated into it, also one further in.
    Directory {
        name: Cow<'a, str>,
        /// How many headings it sits beneath, as an interaction's `level`.
        level: usize,
        /// The branch integrating the worktree's work merges it into.
        into: Option<Cow<'a, str>>,
    },
    Interaction {
        name: Cow<'a, str>,
        /// How many [`InteractionRow::Directory`] headings the row sits
        /// beneath, each one indenting it a step further. Zero for a row
        /// directly under its Workspace.
        level: usize,
        /// How many sources deep this interaction was branched: nested
        /// beneath the entry it came from, as the Session picker nests it.
        depth: usize,
        /// The name of the interaction this one was branched from, when it is
        /// not nested beneath it: it works in another checkout, or is not
        /// listed.
        branched_from: Option<Cow<'a, str>>,
        /// How many listed interactions name this one as their source with
        /// `branched_from`, rather than being nested beneath it.
        branches: usize,
        /// The branch tying this row to the cursor's, either way, so it is
        /// marked for the eye to find.
        branch_link: BranchLink,
        /// The model's id, which names its provider too.
        model: &'a str,
        /// The Git branch checked out for this interaction. `None` means the
        /// interaction is not associated with a Git checkout; a detached
        /// checkout is passed as the descriptive text `detached head`.
        branch: Option<&'a str>,
        /// Assigned branch when the checkout has switched away from it.
        expected_branch: Option<&'a str>,
        status: InteractionStatus,
        selected: bool,
        newly_idle: bool,
        /// Something new just happened on it, so the row is drawn lit for a
        /// moment to catch the eye.
        flashing: bool,
        /// Why a stopped interaction stopped. Unlike the reasons the other
        /// statuses carry, this one is rendered on the row: the stopped
        /// entries sit at the foot of the list precisely so they can be read
        /// through, and "stopped" on its own does not say whether the operator
        /// paused it, a plan window refused it, or the agent fell over.
        stop_reason: Option<Cow<'a, str>>,
        /// The window that refused this interaction's work, when it is idle
        /// because a plan window turned it away rather than because its turn
        /// ended. Idling of that kind is not the operator's turn to speak:
        /// nothing they send will run until the window turns over, so the row
        /// says so instead of reading as ordinary idle.
        rate_limited: Option<Cow<'a, str>>,
        /// The interaction has stopped and left work uncommitted in its
        /// repository — see [`crate::footer::FooterView::uncommitted_changes`].
        uncommitted: bool,
        /// Whether, and how, the operator is finished with the Session. Each
        /// way of being done has its own badge, so a Session given up on
        /// does not read as one that got done, and the irreversible seal
        /// does not read as either.
        completion: CompletionState,
        tags: &'a [String],
        last_message: Option<&'a str>,
    },
}

pub struct InteractionNavigator<'a> {
    pub scope: Cow<'a, str>,
    pub all_workspaces: bool,
    pub completion_filter: Cow<'a, str>,
    /// The tags every listed row carries, chosen with `ctrl-t`.
    pub tag_filter: &'a [String],
    /// The `/` filter's term, when one is being typed or is in force.
    pub filter: Option<&'a str>,
    /// Whether the filter is still being typed, so it is drawn with a caret.
    pub typing_filter: bool,
    /// The first line shown last frame. [`render`] keeps it unless the cursor
    /// comes within [`SCROLL_MARGIN`] rows of an edge, and returns the line it
    /// actually drew from. `None` starts at the bottom on the first draw.
    pub requested_offset: Option<usize>,
    pub rows: Vec<InteractionRow<'a>>,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/interactions.rs"]
mod rendering;
