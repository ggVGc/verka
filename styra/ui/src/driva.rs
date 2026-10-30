//! The server's general account of the current Workspace and Interaction:
//! durable Workspace metadata, live Interaction state, and the Driva sandbox
//! it runs in (or would run in before the first message).
//!
//! Before anything has launched the same fields describe the policy the next
//! interaction would start under, marked as planned so the two are not read as
//! the same claim. In that state the view is also where the policy is chosen.
//!
//! What is chosen is two settings, not one, and they are shown as two: the
//! Workspace's standing policy applies to every launch here and outlives every
//! interaction in it, while this interaction's own settings are layered over it
//! and go when it does. Each gets its own pane, with the same three rows in the
//! same order, so the difference between them is which pane a grant sits in and
//! nothing else. The up/down arrows move the editing keys between the panes and
//! the focused one says so; every other key acts on whichever that is, so there
//! is one set of keys rather than one per layer.
//!
//! Above both is the policy those two resolve to, which is what the agent
//! actually gets — including the parts neither pane can change: the workspace
//! mount, the profile's credential mounts, the broker's control mount.

use crate::{chrome::PanelChrome, fuzzy_list::FuzzyList};
use std::path::PathBuf;
use styra_protocol::{BranchPoint, CheckoutState, DrivaOptions, LaunchPolicy, ProjectAction};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaunchScope {
    Workspace,
    #[default]
    Interaction,
}

/// The current page of the details panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailsTab {
    #[default]
    Details,
    Sandbox,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrivaStatus {
    Pending,
    Running,
    Idle,
    Background,
    Stopped,
    Ended,
}

pub struct DrivaWorkspace {
    pub id: Option<String>,
    pub name: Option<String>,
    pub given_name: Option<String>,
    pub git_repository: Option<PathBuf>,
    pub host_path: Option<PathBuf>,
    pub server_path: Option<PathBuf>,
    pub session_count: Option<usize>,
    pub age: Option<String>,
    pub created_at_ms: Option<u64>,
    pub last_accessed_at_ms: Option<u64>,
    pub root: Option<PathBuf>,
    pub working_directory: Option<PathBuf>,
    /// Where the server parks this Workspace's linked worktrees.
    pub worktrees: Option<PathBuf>,
}

pub struct DrivaActivity {
    pub status: DrivaStatus,
}

pub struct DrivaLaunch<'a> {
    pub workspace: &'a LaunchPolicy,
    pub interaction: &'a LaunchPolicy,
    pub scope: LaunchScope,
    pub workspace_cursor: usize,
    pub interaction_cursor: usize,
    pub prompt: Option<&'a str>,
    /// The paths listed under the prompt, and the name typed to narrow them
    /// with the cursor among what it leaves.
    pub paths: Option<(&'a [String], &'a FuzzyList)>,
    /// The open prompt for a project action's command, and what is typed.
    pub command_prompt: Option<(ProjectAction, &'a str)>,
    pub driva: Option<&'a DrivaOptions>,
    pub planned: bool,
    pub requested_scroll: u16,
}

pub struct DrivaView<'a> {
    pub chrome: PanelChrome,
    pub editable: bool,
    /// Whether mounts can be added and removed even though the rest of the
    /// policy is a record: true for an idle interaction, which is restarted to
    /// take them.
    pub mounts_editable: bool,
    pub tab: DetailsTab,
    pub details_requested_scroll: u16,
    pub launch: DrivaLaunch<'a>,
    pub workspace: DrivaWorkspace,
    pub activity: DrivaActivity,
    /// Effective host commands, resolved independently of the agent's sandbox.
    pub project_commands: [String; 2],
    /// The keys that run each project command from the event list, as the
    /// client has them bound.
    pub project_action_keys: [String; 2],
    pub selection_name: String,
    pub session_id: &'a str,
    pub session_name: Option<&'a str>,
    pub queued_count: usize,
    pub last_message: Option<String>,
    pub workspace_launch_pending: usize,
    pub git_repository_prompt: Option<&'a str>,
    /// Where the interaction's agent was working in Git's terms when it last
    /// stopped working, as the server read it — see
    /// [`styra_protocol::InteractionSummary::checkout`]. `None` when the
    /// workspace is not in a repository, or before the interaction has gone
    /// idle once.
    pub checkout: Option<CheckoutState>,
    /// Where the Session's own branch was created from — see
    /// [`styra_protocol::InteractionSummary::branched_from`]. `None` for a
    /// Session Styra made no branch for.
    pub branched_from: Option<BranchPoint>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrivaFeedback {
    pub scroll_limit: u16,
    pub effective_scroll: u16,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/driva.rs"]
mod rendering;
