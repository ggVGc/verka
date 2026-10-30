//! Shared application footer layout and styling.

/// The Git branch this binary was built from. The footer names it ahead of the
/// working directory unless it is `main`.
pub const BUILD_BRANCH: &str = env!("STYRA_BUILD_BRANCH");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub tone: Tone,
    pub bold: bool,
}

pub struct FooterView<'a> {
    pub working_directory: &'a str,
    pub quota: &'a [Segment],
    pub auto_retry: bool,
    /// Whether the interaction shown commits each turn's work as it goes
    /// idle.
    pub auto_commit: bool,
    /// Work the client started that is still going on in the background,
    /// in the operator's words; `None` when there is none.
    pub background: Option<&'a str>,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/footer.rs"]
mod rendering;
