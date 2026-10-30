//! Shared framed-panel chrome used by main application views.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Pending,
    Running,
    Idle,
    Background,
    Stopped(StopTone),
    Error,
    Ended,
}

/// Why an interaction stopped, as far as its color goes. Shared by every view
/// that draws a stopped interaction, so one reason reads the same everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StopTone {
    Paused,
    Completed,
    Abandoned,
    Sealed,
    Integrated,
    IntegrationPending,
    RateLimited,
    Failed,
    Exited,
    ServerRestarted,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelChrome {
    pub focused: bool,
    pub workspace: Option<String>,
    /// The interaction's operator-facing title, shown right after the
    /// Workspace name.
    pub session: Option<String>,
    /// The linked worktree the interaction is working in, when it is not the
    /// Workspace's own checkout. `None` for a plain checkout, so the bar does
    /// not repeat what the Workspace name already said.
    pub worktree: Option<String>,
    /// The branch a new interaction would fork, shown before it starts.
    pub source_branch: Option<String>,
    pub agent: String,
    pub model: String,
    pub model_reported: bool,
    pub effort: Option<String>,
    pub effort_reported: bool,
    pub status: String,
    pub status_tone: StatusTone,
    pub elapsed: Option<String>,
    pub suffix: Option<String>,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/chrome.rs"]
mod rendering;
