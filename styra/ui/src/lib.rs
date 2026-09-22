//! The terminal presentation boundary for Styra.
//!
//! The `tui` crate owns application state, server effects, and input handling.
//! This crate owns the terminal lifecycle and the rendering operations used by
//! those loops.

use anyhow::Context;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend, TestBackend};
use ratatui::{Frame, Terminal};
use std::io::{Stdout, Write};
use std::time::Duration;

pub mod answer;
pub mod application;
pub mod chrome;
pub mod code;
pub mod driva;
pub mod event_list;
pub mod files;
pub mod footer;
pub mod help;
pub mod interactions;
pub mod launcher;
pub mod log;
pub mod markdown;
pub mod messages;
pub mod modal_input;
pub mod overlays;
pub mod palette;
pub mod picker;
pub mod preview;
pub mod quota;
pub mod raw;
pub mod transcript;

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
    EntryLog,
}

/// The semantic document measured by a preview-shaped panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewPanel {
    Selection,
    Command,
    StyraWire,
    ProviderRaw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollFeedback {
    pub panel: PanelId,
    pub limit: u16,
    pub effective_offset: u16,
}

/// Measurements produced only after a frame was drawn successfully.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderFeedback {
    pub scroll: Vec<ScrollFeedback>,
    pub list_offset: Option<usize>,
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

    fn render_session_picker(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
    ) -> UiResult<RenderFeedback>;

    fn render_session_picker_message(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        title: &str,
        message: &str,
    ) -> UiResult<RenderFeedback>;

    fn render_session_picker_name_prompt(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        value: &str,
    ) -> UiResult<RenderFeedback>;

    fn render_workspace_picker(
        &mut self,
        workspaces: &[styra_protocol::WorkspaceSummary],
        selected: usize,
        interactions: &[styra_protocol::InteractionSummary],
        preview: picker::SessionsPreview<'_>,
    ) -> UiResult<RenderFeedback>;

    fn render_template_picker(
        &mut self,
        templates: &[styra_protocol::TemplateSummary],
        chosen: &[String],
        cursor: usize,
    ) -> UiResult<RenderFeedback>;

    fn render_template_picker_loading(&mut self) -> UiResult<RenderFeedback>;
    fn poll_event(&mut self, timeout: Duration) -> UiResult<Option<crossterm::event::Event>>;
    fn close(&mut self) -> UiResult<()>;
}

/// The production Ratatui/Crossterm terminal.
pub struct TerminalUi<B: Backend> {
    terminal: Option<Terminal<B>>,
    restore: fn(&mut Terminal<B>),
}

pub type RatatuiUi = TerminalUi<CrosstermBackend<Stdout>>;
pub type TestUi = TerminalUi<TestBackend>;

impl RatatuiUi {
    pub fn new() -> UiResult<Self> {
        enable_raw_mode()
            .context("enabling raw mode")
            .map_err(UiError::from)?;
        let mut stdout = std::io::stdout();
        if let Err(error) = crossterm::execute!(stdout, EnterAlternateScreen) {
            disable_raw_mode().ok();
            return Err(UiError::from(
                anyhow::Error::from(error).context("entering the alternate screen"),
            ));
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self {
                terminal: Some(terminal),
                restore: restore_crossterm,
            }),
            Err(error) => {
                disable_raw_mode().ok();
                // stdout is unavailable after handing it to the backend on this
                // path, but the alternate screen is still restored by a fresh
                // handle below.
                let mut stdout = std::io::stdout();
                crossterm::execute!(stdout, LeaveAlternateScreen).ok();
                Err(UiError::from(
                    anyhow::Error::from(error).context("initialising terminal"),
                ))
            }
        }
    }
}

fn restore_crossterm(terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
    disable_raw_mode().ok();
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    Write::flush(terminal.backend_mut()).ok();
}

fn no_restore(_: &mut Terminal<TestBackend>) {}

impl<B> TerminalUi<B>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    fn draw(
        &mut self,
        render: impl FnOnce(&mut Frame) -> RenderFeedback,
    ) -> UiResult<RenderFeedback> {
        let mut feedback = RenderFeedback::default();
        self.terminal
            .as_mut()
            .context("drawing after the terminal was closed")?
            .draw(|frame| feedback = render(frame))
            .context("drawing terminal")
            .map_err(UiError::from)?;
        Ok(feedback)
    }

    fn render_application(
        &mut self,
        view: &application::ApplicationView<'_>,
    ) -> UiResult<RenderFeedback> {
        self.draw(|frame| application::render(frame, view))
    }

    fn render_help(
        &mut self,
        window: &str,
        rows: &[help::HelpRow<'_>],
        close_key: &str,
        requested_scroll: u16,
    ) -> UiResult<RenderFeedback> {
        self.draw(|frame| {
            let limit = help::render(
                frame,
                frame.area(),
                window,
                rows,
                close_key,
                requested_scroll,
            );
            RenderFeedback {
                scroll: vec![ScrollFeedback {
                    panel: PanelId::Help,
                    limit,
                    effective_offset: requested_scroll.min(limit),
                }],
                list_offset: None,
            }
        })
    }

    fn render_launcher(&mut self, view: &launcher::LauncherView) -> UiResult<RenderFeedback> {
        self.draw(|frame| {
            launcher::render_launcher(frame, view, frame.area());
            RenderFeedback::default()
        })
    }

    fn render_session_picker(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
    ) -> UiResult<RenderFeedback> {
        self.render_session_picker_with(
            sessions,
            selected,
            order,
            preview,
            filter,
            searching,
            |_| {},
        )
    }

    fn render_session_picker_message(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        title: &str,
        message: &str,
    ) -> UiResult<RenderFeedback> {
        self.render_session_picker_with(
            sessions,
            selected,
            order,
            preview,
            filter,
            searching,
            |frame| picker::render_message_popup(frame, title, message),
        )
    }

    fn render_session_picker_name_prompt(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        value: &str,
    ) -> UiResult<RenderFeedback> {
        self.render_session_picker_with(
            sessions,
            selected,
            order,
            preview,
            filter,
            searching,
            |frame| picker::render_name_prompt(frame, value),
        )
    }

    fn render_session_picker_with(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        overlay: impl FnOnce(&mut Frame),
    ) -> UiResult<RenderFeedback> {
        self.draw(|frame| {
            picker::render_picker(frame, sessions, selected, order, preview, filter, searching);
            overlay(frame);
            RenderFeedback::default()
        })
    }

    fn render_workspace_picker(
        &mut self,
        workspaces: &[styra_protocol::WorkspaceSummary],
        selected: usize,
        interactions: &[styra_protocol::InteractionSummary],
        preview: picker::SessionsPreview<'_>,
    ) -> UiResult<RenderFeedback> {
        self.draw(|frame| {
            picker::render_workspace_picker(frame, workspaces, selected, interactions, preview);
            RenderFeedback::default()
        })
    }

    fn render_template_picker(
        &mut self,
        templates: &[styra_protocol::TemplateSummary],
        chosen: &[String],
        cursor: usize,
    ) -> UiResult<RenderFeedback> {
        self.draw(|frame| {
            picker::render_template_picker(frame, templates, chosen, cursor);
            RenderFeedback::default()
        })
    }

    fn render_template_picker_loading(&mut self) -> UiResult<RenderFeedback> {
        self.draw(|frame| {
            picker::render_template_picker_loading(frame);
            RenderFeedback::default()
        })
    }

    fn poll_event(&mut self, timeout: Duration) -> UiResult<Option<crossterm::event::Event>> {
        if crossterm::event::poll(timeout)
            .context("polling terminal input")
            .map_err(UiError::from)?
        {
            Ok(Some(
                crossterm::event::read()
                    .context("reading terminal input")
                    .map_err(UiError::from)?,
            ))
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> UiResult<()> {
        if let Some(terminal) = self.terminal.as_mut() {
            (self.restore)(terminal);
        }
        self.terminal = None;
        Ok(())
    }
}

impl<B> Ui for TerminalUi<B>
where
    B: Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    fn render_application(
        &mut self,
        view: &application::ApplicationView<'_>,
    ) -> UiResult<RenderFeedback> {
        TerminalUi::render_application(self, view)
    }

    fn render_help(
        &mut self,
        window: &str,
        rows: &[help::HelpRow<'_>],
        close_key: &str,
        requested_scroll: u16,
    ) -> UiResult<RenderFeedback> {
        TerminalUi::render_help(self, window, rows, close_key, requested_scroll)
    }

    fn render_launcher(&mut self, view: &launcher::LauncherView) -> UiResult<RenderFeedback> {
        TerminalUi::render_launcher(self, view)
    }

    fn render_session_picker(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
    ) -> UiResult<RenderFeedback> {
        TerminalUi::render_session_picker(
            self, sessions, selected, order, preview, filter, searching,
        )
    }

    fn render_session_picker_message(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        title: &str,
        message: &str,
    ) -> UiResult<RenderFeedback> {
        TerminalUi::render_session_picker_message(
            self, sessions, selected, order, preview, filter, searching, title, message,
        )
    }

    fn render_session_picker_name_prompt(
        &mut self,
        sessions: &[styra_protocol::SessionSummary],
        selected: usize,
        order: picker::SessionOrder,
        preview: picker::Preview<'_>,
        filter: Option<&str>,
        searching: bool,
        value: &str,
    ) -> UiResult<RenderFeedback> {
        TerminalUi::render_session_picker_name_prompt(
            self, sessions, selected, order, preview, filter, searching, value,
        )
    }

    fn render_workspace_picker(
        &mut self,
        workspaces: &[styra_protocol::WorkspaceSummary],
        selected: usize,
        interactions: &[styra_protocol::InteractionSummary],
        preview: picker::SessionsPreview<'_>,
    ) -> UiResult<RenderFeedback> {
        TerminalUi::render_workspace_picker(self, workspaces, selected, interactions, preview)
    }

    fn render_template_picker(
        &mut self,
        templates: &[styra_protocol::TemplateSummary],
        chosen: &[String],
        cursor: usize,
    ) -> UiResult<RenderFeedback> {
        TerminalUi::render_template_picker(self, templates, chosen, cursor)
    }

    fn render_template_picker_loading(&mut self) -> UiResult<RenderFeedback> {
        TerminalUi::render_template_picker_loading(self)
    }

    fn poll_event(&mut self, timeout: Duration) -> UiResult<Option<crossterm::event::Event>> {
        TerminalUi::poll_event(self, timeout)
    }

    fn close(&mut self) -> UiResult<()> {
        TerminalUi::close(self)
    }
}

/// In-memory implementation used by renderer and presentation-adapter tests.
/// It exercises the exact same dispatch as the production terminal without
/// entering raw mode or touching process-global terminal state.
impl TestUi {
    pub fn new(width: u16, height: u16) -> UiResult<Self> {
        Ok(Self {
            terminal: Some(
                Terminal::new(TestBackend::new(width, height))
                    .context("initialising test terminal")
                    .map_err(UiError::from)?,
            ),
            restore: no_restore,
        })
    }

    /// Copy the rendered terminal cells into plain text rows. This keeps
    /// consumers of the test seam independent of Ratatui's buffer types.
    pub fn rows(&self) -> Vec<String> {
        let buffer = self
            .terminal
            .as_ref()
            .expect("test terminal is open")
            .backend()
            .buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).expect("cell inside buffer").symbol())
                    .collect()
            })
            .collect()
    }
}

impl<B: Backend> Drop for TerminalUi<B> {
    fn drop(&mut self) {
        if let Some(terminal) = self.terminal.as_mut() {
            (self.restore)(terminal);
        }
    }
}
