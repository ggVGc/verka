//! The terminal presentation boundary for Styra.
//!
//! The `tui` crate owns application state, server effects, and input handling.
//! This crate owns the terminal lifecycle and the rendering operations used by
//! those loops, along with the scroll and search state those renderings are
//! measured against, which `styra-diff` shares.
//!
//! Models and interaction helpers are always available. Terminal ownership and
//! widget implementations require the default `renderer` feature.

use std::time::Duration;

pub mod answer;
pub mod application;
pub mod busy;
pub mod chrome;
#[cfg(feature = "renderer")]
pub mod code;
#[cfg(feature = "renderer")]
pub mod diff;
pub mod driva;
pub mod event_list;
pub mod files;
pub mod footer;
pub mod fuzzy_list;
pub mod help;
pub mod interactions;
#[cfg(feature = "renderer")]
mod language;
pub mod launcher;
pub mod log;
pub mod markdown;
pub mod messages;
pub mod modal_input;
pub mod overlays;
pub mod overview;
#[cfg(feature = "renderer")]
pub mod palette;
pub mod picker;
pub mod placeholders;
pub mod preview;
pub mod processes;
pub mod query;
pub mod quota;
pub mod raw;
pub mod recording;
#[cfg(feature = "renderer")]
mod render_cache;
pub mod scroll;
pub mod search;
pub mod text;
#[cfg(feature = "renderer")]
pub mod text_prompt;
#[cfg(feature = "renderer")]
pub mod theme;
pub mod transcript;
pub mod worktrees;

/// A stable identity for layout feedback that application navigation consumes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PanelId {
    Help,
    Transcript {
        session: String,
    },
    Preview {
        session: String,
        target: PreviewPanel,
    },
    Driva {
        session: String,
    },
    CheckoutDiff {
        session: String,
    },
    EntryLog,
}

/// The semantic document measured by a preview-shaped panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewPanel {
    Selection,
    Command,
    StyraWire,
    Messages,
    ProviderRaw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollFeedback {
    pub panel: PanelId,
    pub limit: u16,
    pub effective_offset: u16,
    /// How many lines the panel shows at once, for scrolling by a fraction
    /// of it. Zero for a panel that does not report it.
    pub viewport: u16,
}

/// Measurements produced only after a frame was drawn successfully.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderFeedback {
    pub scroll: Vec<ScrollFeedback>,
    pub list_offset: Option<usize>,
    pub list_row_offset: Option<usize>,
    /// The line the interactions navigator was drawn from, when it was drawn.
    pub navigator_offset: Option<usize>,
    /// First visible row in the checkout diff's file tree.
    pub diff_file_list_offset: Option<usize>,
    /// Effective side-by-side layout and horizontal offset of the checkout diff.
    pub diff_layout: Option<(bool, u16)>,
    /// The file selected on the checkout diff's report page, the page's
    /// scroll and its file patch's, as drawn, while the page is open.
    pub diff_report_page: Option<(usize, u16, Option<u16>)>,
    /// How many tiles each row of the overview held, top to bottom, when it
    /// was drawn.
    pub overview_rows: Option<Vec<usize>>,
}

/// An error raised by the terminal boundary.
///
/// The error deliberately carries only a diagnostic message.  In particular,
/// Ratatui backend and frame types do not escape into the interface consumed
/// by the application.  Callers can add their own application context with
/// `anyhow` in the usual way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError {
    message: String,
}

impl UiError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UiError {}

impl From<anyhow::Error> for UiError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(format!("{error:#}"))
    }
}

pub type UiResult<T> = std::result::Result<T, UiError>;

/// Terminal boundary consumed by the application and picker loops.
///
/// Each presentation operation is exposed directly, so implementations do not
/// have to decode an intermediate screen enum. Ratatui is one implementation;
/// tests and alternative frontends can implement the same boundary without it.
pub trait Ui {
    fn render_application(
        &mut self,
        view: &application::ApplicationView<'_>,
    ) -> UiResult<RenderFeedback>;

    fn render_help(
        &mut self,
        window: &str,
        rows: &[help::HelpRow<'_>],
        close_key: &str,
        requested_scroll: u16,
    ) -> UiResult<RenderFeedback>;

    fn render_launcher(&mut self, view: &launcher::LauncherView) -> UiResult<RenderFeedback>;

    // Picker display state is passed explicitly across the UI boundary.
    #[allow(clippy::too_many_arguments)]
    fn render_session_picker(
        &mut self,
        scope: &str,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        show_completed: bool,
    ) -> UiResult<RenderFeedback>;

    // Picker display state is passed explicitly across the UI boundary.
    #[allow(clippy::too_many_arguments)]
    fn render_session_picker_message(
        &mut self,
        scope: &str,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        show_completed: bool,
        title: &str,
        message: &str,
    ) -> UiResult<RenderFeedback>;

    // Picker display state is passed explicitly across the UI boundary.
    #[allow(clippy::too_many_arguments)]
    fn render_session_picker_name_prompt(
        &mut self,
        scope: &str,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        show_completed: bool,
        value: &str,
    ) -> UiResult<RenderFeedback>;

    // Picker display state is passed explicitly across the UI boundary.
    #[allow(clippy::too_many_arguments)]
    fn render_workspace_picker(
        &mut self,
        workspaces: &[styra_protocol::WorkspaceSummary],
        selected: usize,
        interactions: &[styra_protocol::InteractionSummary],
        preview: picker::SessionsPreview<'_>,
        filter: Option<&str>,
        offer: Option<&std::path::Path>,
        show_deleted: bool,
    ) -> UiResult<RenderFeedback>;

    fn render_fuzzy_picker(
        &mut self,
        view: &fuzzy_list::FuzzyListView<'_>,
    ) -> UiResult<RenderFeedback>;

    fn render_worktree_picker(
        &mut self,
        view: &worktrees::WorktreePickerView<'_>,
    ) -> UiResult<RenderFeedback>;

    // Picker display state is passed explicitly across the UI boundary.
    #[allow(clippy::too_many_arguments)]
    fn render_workspace_picker_prompt(
        &mut self,
        workspaces: &[styra_protocol::WorkspaceSummary],
        selected: usize,
        interactions: &[styra_protocol::InteractionSummary],
        preview: picker::SessionsPreview<'_>,
        filter: Option<&str>,
        title: &str,
        value: &str,
    ) -> UiResult<RenderFeedback>;

    fn render_template_picker(
        &mut self,
        templates: &[styra_protocol::TemplateSummary],
        chosen: &[String],
        cursor: usize,
    ) -> UiResult<RenderFeedback>;

    fn render_template_picker_loading(&mut self) -> UiResult<RenderFeedback>;
    fn poll_event(&mut self, timeout: Duration) -> UiResult<Option<crossterm::event::Event>>;
    /// Keep paste grouped while the message editor owns input.
    fn set_bracketed_paste(&mut self, _enabled: bool) -> UiResult<()> {
        Ok(())
    }
    fn close(&mut self) -> UiResult<()>;
}

#[cfg(feature = "renderer")]
mod terminal;
#[cfg(feature = "renderer")]
pub use terminal::{RatatuiUi, TerminalUi, TestUi};
