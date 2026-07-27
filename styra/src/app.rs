//! Application state: the event list, selection, expansion, focus, the message
//! buffer, and session status.
//!
//! This module is pure state and transitions — no terminal, no threads, no IO —
//! so the whole interaction model is unit-testable. [`crate::ui`] renders it and
//! `main` feeds it input and session updates.

use std::collections::VecDeque;
use std::path::PathBuf;
use styra_server::agent::{Provider, Selection, PROVIDERS};
use styra_server::event::{AgentEvent, DetailBlock, TokenUsage};
use styra_server::{DrivaOptions, InteractionEnd, LogEntry, RawLine};

/// Which region receives keys, like vim's normal/insert split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Navigate and fold the event list.
    List,
    /// Type into the message box.
    Input,
}

/// What the main region shows: the decoded event list, the raw wire stream,
/// the diagnostic log, the rendered transcript, the session's Driva policy,
/// or the selected entry's full-screen preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Events,
    Raw,
    Log,
    Transcript,
    Driva,
    Preview,
}

/// The session's lifecycle as the operator sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No agent process has been launched yet; it starts on the operator's
    /// first submitted message (see `App::pending`).
    Pending,
    /// The agent is working.
    Running,
    /// A turn completed; the agent is idle, awaiting input.
    Idle,
    /// The operator stopped the session; the process may still be winding down.
    Stopped,
    /// The agent process ended.
    Ended {
        exit_code: Option<i32>,
        error: Option<String>,
    },
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Pending => "not started".into(),
            Status::Running => "running".into(),
            Status::Idle => "idle".into(),
            Status::Stopped => "stopped".into(),
            Status::Ended { error: Some(_), .. } => "failed".into(),
            Status::Ended {
                exit_code: Some(code),
                ..
            } => format!("ended ({code})"),
            Status::Ended { .. } => "ended".into(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Status::Pending | Status::Running | Status::Idle | Status::Stopped
        )
    }
}

/// One event in the list, with its fold state.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub event: AgentEvent,
    pub expanded: bool,
    /// The index into [`App::raw`] of the wire line this entry was decoded
    /// from, if known — lets the raw view jump straight to the line behind
    /// an entry instead of making the operator hunt for it. Best-effort: an
    /// operator's own message is echoed as an entry before its encoded wire
    /// line is journaled, so for it this points at whatever line came just
    /// before instead of its own.
    pub raw_index: Option<usize>,
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

/// The agent, model, and reasoning effort a session is on, as the status line
/// names them.
///
/// `model` and `effort` are what the agent *reported* once it started, falling
/// back to what the launch asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchLabel {
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Whether `model` came from the agent itself rather than from the launch
    /// request, so the display can distinguish what *is* running from what was
    /// asked for.
    pub model_reported: bool,
    /// The same for `effort`. It can differ from `model_reported`: Claude Code
    /// names the model it resolved but never an effort, so a Claude session's
    /// effort is only ever what the launch asked for.
    pub effort_reported: bool,
}

/// Which of the launch picker's three columns has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchColumn {
    Provider,
    Model,
    Effort,
}

/// The row a column opens on for a newly chosen agent: the provider's own
/// declared default (see [`Provider::default_model`]), so switching agents lands
/// on that provider's standard model and effort.
fn default_model_row(provider: Provider) -> usize {
    provider
        .models()
        .iter()
        .position(|model| *model == provider.default_model())
        .unwrap_or(0)
}

fn default_effort_row(provider: Provider) -> usize {
    provider
        .efforts()
        .iter()
        .position(|effort| *effort == provider.default_effort())
        .unwrap_or(0)
}

/// The launch picker: the agent, model, and reasoning effort the *next* session
/// will start with.
///
/// It edits a pending choice, not a running session — confirming it only records
/// the selection, and the operator's own first message still starts the agent.
/// Every row is a concrete choice out of the provider's own catalogs
/// ([`Provider::models`], [`Provider::efforts`]), and a [`Selection`] always pins
/// both, so there is nothing for a row meaning "whatever the agent is configured
/// for" to express. A newly chosen agent opens on the model and effort
/// the provider's declared defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launcher {
    pub column: LaunchColumn,
    pub provider: usize,
    /// An index into [`Provider::models`], then `carried_model` if there is one.
    pub model: usize,
    /// An index into [`Provider::efforts`].
    pub effort: usize,
    /// A model the picker does not offer but the session was nonetheless
    /// launched with. Shown as a final row so
    /// the operator can leave it selected; the picker cannot type one, only
    /// carry one it was opened on.
    pub carried_model: Option<String>,
}

impl Launcher {
    /// Open the picker on `selection` — it always names a model and an effort, so
    /// there is always a row to open on. A model the provider's catalog does not
    /// list is carried as its own final row rather than dropped, so confirming
    /// the picker cannot silently change an existing selection.
    pub fn from_selection(selection: &Selection) -> Self {
        let provider = PROVIDERS
            .iter()
            .position(|candidate| *candidate == selection.provider)
            .unwrap_or(0);
        let models = selection.provider.models();
        let (model, carried_model) = match models
            .iter()
            .position(|candidate| *candidate == selection.model)
        {
            Some(index) => (index, None),
            None => (models.len(), Some(selection.model.clone())),
        };
        let effort = selection
            .provider
            .efforts()
            .iter()
            .position(|candidate| *candidate == selection.effort)
            .unwrap_or(0);
        Self {
            column: LaunchColumn::Provider,
            provider,
            model,
            effort,
            carried_model,
        }
    }

    pub fn provider(&self) -> Provider {
        PROVIDERS[self.provider.min(PROVIDERS.len() - 1)]
    }

    /// What the picker currently describes. Every row is a concrete choice, so
    /// this is always a fully pinned selection; the clamps cover a row index that
    /// somehow outran its column rather than any "unset" state.
    pub fn selection(&self) -> Selection {
        let provider = self.provider();
        let models = provider.models();
        let model = match models.get(self.model) {
            Some(model) => (*model).to_owned(),
            None => self
                .carried_model
                .clone()
                .unwrap_or_else(|| provider.default_model().to_owned()),
        };
        let efforts = provider.efforts();
        let effort = efforts
            .get(self.effort)
            .copied()
            .unwrap_or_else(|| provider.default_effort());
        Selection {
            provider,
            model,
            effort,
        }
    }

    /// The model column's rows: the provider's catalog, plus a carried model if
    /// the picker was opened on one.
    pub fn model_rows(&self) -> usize {
        self.provider().models().len() + usize::from(self.carried_model.is_some())
    }

    /// How many rows the focused column has.
    fn rows(&self) -> usize {
        match self.column {
            LaunchColumn::Provider => PROVIDERS.len(),
            LaunchColumn::Model => self.model_rows(),
            LaunchColumn::Effort => self.provider().efforts().len(),
        }
    }

    fn row(&mut self) -> &mut usize {
        match self.column {
            LaunchColumn::Provider => &mut self.provider,
            LaunchColumn::Model => &mut self.model,
            LaunchColumn::Effort => &mut self.effort,
        }
    }

    pub fn next(&mut self) {
        let last = self.rows() - 1;
        let row = self.row();
        *row = (*row + 1).min(last);
        self.after_move();
    }

    pub fn prev(&mut self) {
        let row = self.row();
        *row = row.saturating_sub(1);
        self.after_move();
    }

    fn after_move(&mut self) {
        // A model or effort chosen for the previous provider means nothing to the
        // new one — the ladders and catalogs differ — so both reset to that
        // agent's own opening rows rather than to whatever sits at the same
        // index. That includes a carried model, which belonged to the agent the
        // picker was opened on.
        if self.column == LaunchColumn::Provider {
            self.model = default_model_row(self.provider());
            self.effort = default_effort_row(self.provider());
            self.carried_model = None;
        }
    }

    pub fn next_column(&mut self) {
        self.column = match self.column {
            LaunchColumn::Provider => LaunchColumn::Model,
            LaunchColumn::Model => LaunchColumn::Effort,
            LaunchColumn::Effort => LaunchColumn::Provider,
        };
    }

    pub fn prev_column(&mut self) {
        self.column = match self.column {
            LaunchColumn::Provider => LaunchColumn::Effort,
            LaunchColumn::Model => LaunchColumn::Provider,
            LaunchColumn::Effort => LaunchColumn::Model,
        };
    }
}

/// The complete UI state.
pub struct App {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub focus: Focus,
    pub view: View,
    pub input: String,
    /// Submitted prompts, oldest first, for readline-style Up/Down recall.
    prompt_history: Vec<String>,
    /// The recalled history row and the draft that was present before recall.
    history_cursor: Option<usize>,
    history_draft: String,
    /// Messages submitted while the current turn is running. They remain
    /// visible until sent or the interaction is stopped.
    queued_messages: VecDeque<String>,
    pub status: Status,
    /// When true, the selection tracks the newest entry as events arrive.
    pub follow: bool,
    /// When false, minor lifecycle events (thread/turn/usage) are hidden from
    /// the list and skipped by navigation.
    pub show_minor: bool,
    /// When true, a side panel shows the full expanded content of the
    /// selected entry, independent of whether it is folded in the list.
    pub show_preview: bool,
    /// Lines scrolled down from the top of the selected entry's preview.
    /// Rendering clamps this to the last page because wrapping depends on the
    /// terminal width.
    pub preview_scroll: u16,
    /// What the next session launches with: agent, model, reasoning effort.
    /// This is the operator's standing choice, edited through [`Launcher`] while
    /// nothing is running. The terminal client persists a confirmed choice.
    pub selection: Selection,
    /// The open launch picker, while the operator is choosing.
    pub launcher: Option<Launcher>,
    /// The model and reasoning effort the agent itself reported when it started
    /// the session, which is what is actually running — a launch pins a model,
    /// but only the agent can confirm what it resolved to. `None` until the
    /// agent's session-start line arrives (and for agents that report neither).
    pub reported_model: Option<(String, Option<String>)>,
    /// Durable Workspace containing the current Session, when known.
    pub workspace_id: Option<String>,
    pub session_id: String,
    /// The host directory backing the agent's sandboxed workspace, when
    /// known (a live session; a replayed journal has no live workspace).
    /// Lets the preview panel read a changed file's current content.
    pub workspace_root: Option<PathBuf>,
    /// The Driva policy the live session was launched under (mounts, network,
    /// isolation backend). `None` for a session that has not launched yet, or
    /// a replayed journal, which has no live sandbox to describe.
    pub driva_options: Option<DrivaOptions>,
    pub latest_usage: Option<TokenUsage>,
    /// The verbatim wire interaction, in occurrence order.
    pub raw: Vec<RawLine>,
    /// Which wire line the raw view has selected.
    pub raw_selected: usize,
    /// When true, `raw_selected` tracks the newest line as it arrives.
    pub raw_follow: bool,
    /// Lines scrolled down from the top of the selected wire line's pretty-
    /// printed preview. Rendering clamps this to the last page, as
    /// [`preview_scroll`](Self::preview_scroll) does for the entry preview.
    pub raw_preview_scroll: u16,
    /// Diagnostic log entries, in occurrence order.
    pub log: Vec<LogEntry>,
    /// Lines scrolled back from the bottom of the log view; 0 tracks the tail.
    pub log_scroll_back: u16,
    /// Lines scrolled down from the top of the rendered transcript view; 0
    /// shows its start. Unlike the raw/log views, the transcript reads as a
    /// document from the beginning rather than anchoring to the tail.
    pub transcript_scroll: u16,
    /// Set when the operator asks to quit; the event loop observes it.
    pub should_quit: bool,
    /// Set when the operator asks to choose a Workspace.
    pub workspace_requested: bool,
    /// Set when the operator asks to list the server's live interactions; the event
    /// loop observes it and opens the interactions picker to attach to one.
    pub interactions_requested: bool,
    /// Set when the operator asks to stop the current interaction and return to the
    /// blank start screen; the event loop observes it.
    pub reset_requested: bool,
}

impl App {
    pub fn new(selection: Selection, session_id: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            focus: Focus::List,
            view: View::Events,
            input: String::new(),
            prompt_history: Vec::new(),
            history_cursor: None,
            history_draft: String::new(),
            queued_messages: VecDeque::new(),
            status: Status::Running,
            follow: true,
            show_minor: false,
            show_preview: false,
            preview_scroll: 0,
            selection,
            launcher: None,
            reported_model: None,
            workspace_id: None,
            session_id: session_id.into(),
            workspace_root: None,
            driva_options: None,
            latest_usage: None,
            raw: Vec::new(),
            raw_selected: 0,
            raw_follow: true,
            raw_preview_scroll: 0,
            log: Vec::new(),
            log_scroll_back: 0,
            transcript_scroll: 0,
            should_quit: false,
            workspace_requested: false,
            interactions_requested: false,
            reset_requested: false,
        }
    }

    /// A fresh App with no agent process launched yet: no journal or session
    /// id exists until the operator submits a first message, at which point
    /// the event loop spawns the session and fills those in.
    /// Opens directly in input focus, since typing there is the only thing
    /// that moves the session forward.
    ///
    /// `selection` is what the session will launch with; it is also the only
    /// state in this screen the operator can still change (see
    /// [`App::open_launcher`]), since nothing has been launched to be stuck
    /// with yet.
    pub fn pending(selection: Selection) -> Self {
        let mut app = Self::new(selection, String::new());
        app.status = Status::Pending;
        app.focus = Focus::Input;
        app
    }

    /// Whether the launch picker is reachable: only before anything has been
    /// launched. A live or replayed session's agent, model, and effort are
    /// settled facts about a process that already ran; changing them is a
    /// property of the *next* session, reached by resetting (`S`) first.
    pub fn can_configure_launch(&self) -> bool {
        self.status == Status::Pending
    }

    /// Open the launch picker on the current selection, if anything can still
    /// be chosen.
    pub fn open_launcher(&mut self) {
        if self.can_configure_launch() {
            self.launcher = Some(Launcher::from_selection(&self.selection));
        }
    }

    /// Adopt what the picker describes as the next session's launch, and close
    /// it. Nothing is launched or sent: the operator's first message still
    /// starts the agent.
    pub fn confirm_launcher(&mut self) {
        if let Some(launcher) = self.launcher.take() {
            self.set_selection(launcher.selection());
        }
    }

    /// Close the picker, leaving the selection as it was.
    pub fn cancel_launcher(&mut self) {
        self.launcher = None;
    }

    /// What the status line names: the agent, and the model and reasoning effort
    /// in use.
    ///
    /// The agent's own report wins where it made one, since that is what is
    /// actually running; otherwise the requested selection answers for it.
    pub fn launch_label(&self) -> LaunchLabel {
        let agent = self.selection.provider.as_str().to_owned();
        let requested_model = Some(self.selection.model.clone());
        let requested_effort = Some(self.selection.effort.as_str().to_owned());
        match &self.reported_model {
            Some((model, effort)) => LaunchLabel {
                agent,
                model: Some(model.clone()),
                effort_reported: effort.is_some(),
                effort: effort.clone().or(requested_effort),
                model_reported: true,
            },
            None => LaunchLabel {
                agent,
                model: requested_model,
                effort: requested_effort,
                model_reported: false,
                effort_reported: false,
            },
        }
    }

    /// Record the launch choice so the status line names what an `Enter` would start.
    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = selection;
    }

    /// Replace the message box's contents outright, used to restore a message
    /// that failed to launch so it isn't lost.
    pub fn set_input(&mut self, text: String) {
        self.input = text;
        self.reset_history_navigation();
    }

    /// Append a diagnostic log entry, keeping the tail in view unless the
    /// operator has scrolled up (mirrors [`push_raw`](Self::push_raw)).
    pub fn push_log(&mut self, entry: LogEntry) {
        self.log.push(entry);
        if self.log_scroll_back > 0 {
            self.log_scroll_back = self.log_scroll_back.saturating_add(1);
        }
    }

    pub fn log_scroll_up(&mut self) {
        let max = self.log.len().saturating_sub(1) as u16;
        self.log_scroll_back = self.log_scroll_back.saturating_add(1).min(max);
    }

    pub fn log_scroll_down(&mut self) {
        self.log_scroll_back = self.log_scroll_back.saturating_sub(1);
    }

    pub fn log_to_top(&mut self) {
        self.log_scroll_back = self.log.len().saturating_sub(1) as u16;
    }

    pub fn log_to_bottom(&mut self) {
        self.log_scroll_back = 0;
    }

    /// Append a verbatim wire line. When the operator has selected a line
    /// explicitly, the view stays pinned to it; otherwise the selection
    /// tracks the new tail.
    pub fn push_raw(&mut self, line: RawLine) {
        self.raw.push(line);
        if self.raw_follow {
            self.raw_selected = self.raw.len() - 1;
            self.raw_preview_scroll = 0;
        }
    }

    /// Toggle the raw wire view on, or back to the event list. Entering it
    /// focuses the wire line behind the currently selected entry (or the
    /// tail, while the list is following it, or if no line is known for the
    /// selection), so switching views keeps the same point in the session
    /// in view rather than resetting to wherever the raw view was last left.
    pub fn toggle_raw(&mut self) {
        if self.view == View::Raw {
            self.view = View::Events;
            return;
        }
        self.view = View::Raw;
        self.raw_preview_scroll = 0;
        if !self.follow {
            if let Some(idx) = self
                .entries
                .get(self.selected)
                .and_then(|entry| entry.raw_index)
            {
                self.raw_selected = idx.min(self.raw.len().saturating_sub(1));
                self.raw_follow = false;
                return;
            }
        }
        self.raw_selected = self.raw.len().saturating_sub(1);
        self.raw_follow = true;
    }

    /// Toggle the diagnostic log view on, or back to the event list.
    pub fn toggle_log(&mut self) {
        self.view = if self.view == View::Log {
            View::Events
        } else {
            View::Log
        };
    }

    /// Toggle the rendered transcript view on, or back to the event list.
    pub fn toggle_transcript(&mut self) {
        self.view = if self.view == View::Transcript {
            View::Events
        } else {
            View::Transcript
        };
    }

    /// Toggle the Driva policy view on, or back to the event list.
    pub fn toggle_driva(&mut self) {
        self.view = if self.view == View::Driva {
            View::Events
        } else {
            View::Driva
        };
    }

    /// Toggle a full-screen view of the selected entry's content on, or back
    /// to the event list.
    pub fn toggle_fullscreen_preview(&mut self) {
        self.view = if self.view == View::Preview {
            View::Events
        } else {
            View::Preview
        };
    }

    /// Move the raw view's selection to the next wire line.
    pub fn raw_select_next(&mut self) {
        if self.raw_selected + 1 < self.raw.len() {
            self.raw_selected += 1;
            self.raw_preview_scroll = 0;
        }
        // Re-enable follow only when the selection reaches the tail.
        self.raw_follow = !self.raw.is_empty() && self.raw_selected + 1 >= self.raw.len();
    }

    /// Move the raw view's selection to the previous wire line.
    pub fn raw_select_prev(&mut self) {
        if self.raw_selected > 0 {
            self.raw_selected -= 1;
            self.raw_preview_scroll = 0;
        }
        // Moving off the tail pins the view.
        self.raw_follow = false;
    }

    pub fn raw_select_first(&mut self) {
        if self.raw.is_empty() {
            return;
        }
        self.raw_selected = 0;
        self.raw_preview_scroll = 0;
        self.raw_follow = false;
    }

    pub fn raw_select_last(&mut self) {
        if self.raw.is_empty() {
            return;
        }
        self.raw_selected = self.raw.len() - 1;
        self.raw_preview_scroll = 0;
        self.raw_follow = true;
    }

    pub fn raw_preview_page_down(&mut self) {
        self.raw_preview_scroll = self.raw_preview_scroll.saturating_add(10);
    }

    pub fn raw_preview_page_up(&mut self) {
        self.raw_preview_scroll = self.raw_preview_scroll.saturating_sub(10);
    }

    /// Scroll the transcript view forward (towards its end).
    pub fn transcript_scroll_down(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(1);
    }

    /// Scroll the transcript view backward (towards its start).
    pub fn transcript_scroll_up(&mut self) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(1);
    }

    pub fn transcript_to_top(&mut self) {
        self.transcript_scroll = 0;
    }

    /// Jump past the transcript's true end; rendering clamps this back to
    /// the last page, so the exact rendered line count need not be known here.
    pub fn transcript_to_bottom(&mut self) {
        self.transcript_scroll = u16::MAX;
    }

    /// True when the operator can still send messages.
    pub fn can_send(&self) -> bool {
        self.status.is_active()
    }

    pub fn queued_messages(&self) -> impl Iterator<Item = &str> {
        self.queued_messages.iter().map(String::as_str)
    }

    pub fn queued_message_count(&self) -> usize {
        self.queued_messages.len()
    }

    pub fn queue_message(&mut self, message: String) {
        self.queued_messages.push_back(message);
    }

    pub fn take_queued_message(&mut self) -> Option<String> {
        self.queued_messages.pop_front()
    }

    pub fn clear_queued_messages(&mut self) -> usize {
        let count = self.queued_messages.len();
        self.queued_messages.clear();
        count
    }

    // --- Ingesting session updates -----------------------------------------

    /// Append a decoded event, advancing status and, while following, selection.
    pub fn push_event(&mut self, event: AgentEvent) {
        // A command completion is the final state of the command-start row.
        // Replace the most recent matching start instead of adding a second
        // line, so the list shows one command whose indication changes from
        // running to its result.
        if let AgentEvent::CommandCompleted { command, .. } = &event {
            if let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
                matches!(&entry.event, AgentEvent::CommandStarted { command: started } if started == command)
            }) {
                entry.event = event;
                if self.follow {
                    self.selected = self.entries.len() - 1;
                    self.preview_scroll = 0;
                }
                return;
            }
        }
        // Same as above, for tool calls: a `ToolCompleted` is the final state
        // of its matching `ToolStarted` row, correlated by id rather than
        // name — Claude's `tool_result` only ever repeats the `tool_use_id`,
        // never the tool's name or arguments, so the completed event's own
        // `name`/`detail` are placeholders that get replaced with the started
        // row's real ones (e.g. `Bash` and its command), so the finished row
        // still shows what actually ran rather than just the bare tool name.
        if let AgentEvent::ToolCompleted {
            id,
            status,
            output,
            ..
        } = &event
        {
            if let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
                matches!(&entry.event, AgentEvent::ToolStarted { id: started, .. } if started == id)
            }) {
                if let AgentEvent::ToolStarted { id, name, detail } = &entry.event {
                    entry.event = AgentEvent::ToolCompleted {
                        id: id.clone(),
                        name: name.clone(),
                        detail: detail.clone(),
                        status: status.clone(),
                        output: output.clone(),
                    };
                }
                if self.follow {
                    self.selected = self.entries.len() - 1;
                    self.preview_scroll = 0;
                }
                return;
            }
            // Claude's Edit/Write/MultiEdit tool calls surface as `FileChanged`
            // at start, not `ToolStarted` (see `claude_tool_started`), so their
            // matching `ToolCompleted` never finds a started row above and
            // would otherwise fall through to a new, id-only line. A clean
            // result just confirms what the `FileChanged` row already showed,
            // so it is dropped rather than appended a second time; a failed
            // one replaces the row with a visible error, since the diff shown
            // there may not have actually landed.
            if let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
                matches!(&entry.event, AgentEvent::FileChanged { id: changed, .. } if changed == id)
            }) {
                if status == "error" {
                    if let AgentEvent::FileChanged { paths, .. } = &entry.event {
                        entry.event = AgentEvent::Error {
                            message: format!("{}: {output}", paths.join(", ")),
                        };
                    }
                }
                if self.follow {
                    self.selected = self.entries.len() - 1;
                    self.preview_scroll = 0;
                }
                return;
            }
        }
        match &event {
            AgentEvent::TurnCompleted { usage } => {
                // The app-server protocol's `turn/completed` carries no usage
                // figures of its own (a default, empty one); keep whatever the
                // last `UsageUpdated` reported rather than blanking the display.
                if *usage != TokenUsage::default() {
                    self.latest_usage = Some(usage.clone());
                }
                if self.status.is_active() {
                    self.status = Status::Idle;
                }
            }
            AgentEvent::UsageUpdated { usage } => {
                self.latest_usage = Some(usage.clone());
            }
            // The agent naming its own model settles what is running, so it
            // replaces the launch request in the status line. An effort the
            // agent does not report leaves whatever was already known standing
            // (Claude Code names a model but never an effort, so the launch's
            // own `--effort` remains the only word on it).
            AgentEvent::ThreadStarted {
                model: Some(model),
                effort,
                ..
            } => {
                let known = effort
                    .clone()
                    .or_else(|| self.reported_model.take().and_then(|(_, effort)| effort));
                self.reported_model = Some((model.clone(), known));
            }
            AgentEvent::UserMessage { .. }
            | AgentEvent::TurnStarted
            | AgentEvent::CommandStarted { .. }
            | AgentEvent::ToolStarted { .. }
            | AgentEvent::AgentMessage { .. }
            | AgentEvent::Thinking { .. }
            | AgentEvent::PlanUpdated { .. } => {
                if self.status.is_active() {
                    self.status = Status::Running;
                }
            }
            _ => {}
        }
        self.entries.push(Entry {
            event,
            expanded: false,
            raw_index: self.raw.len().checked_sub(1),
        });
        if self.follow {
            self.selected = self.entries.len() - 1;
            self.preview_scroll = 0;
        }
    }

    /// Record that the session ended. This is terminal regardless of `Stopped`.
    pub fn on_ended(&mut self, end: InteractionEnd) {
        self.status = Status::Ended {
            exit_code: end.exit_code,
            error: end.error,
        };
    }

    // --- List navigation ----------------------------------------------------

    /// Whether an entry is shown in the list under the current minor filter.
    pub fn is_visible(&self, idx: usize) -> bool {
        self.show_minor || !self.entries[idx].event.is_minor()
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

    /// The nearest visible index at or after `from`, if any.
    fn next_visible(&self, from: usize) -> Option<usize> {
        (from..self.entries.len()).find(|&i| self.is_visible(i))
    }

    /// The nearest visible index at or before `from`, if any.
    fn prev_visible(&self, from: usize) -> Option<usize> {
        (0..=from).rev().find(|&i| self.is_visible(i))
    }

    /// The nearest navigable index at or after `from`, if any.
    fn next_navigable(&self, from: usize) -> Option<usize> {
        (from..self.entries.len()).find(|&i| self.is_navigable(i))
    }

    /// The nearest navigable index at or before `from`, if any.
    fn prev_navigable(&self, from: usize) -> Option<usize> {
        (0..=from).rev().find(|&i| self.is_navigable(i))
    }

    /// Toggle the side panel that previews the selected entry's full content.
    pub fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
    }

    pub fn preview_page_down(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_add(10);
    }

    pub fn preview_page_up(&mut self) {
        self.preview_scroll = self.preview_scroll.saturating_sub(10);
    }

    /// Record the host directory backing the agent's workspace, so the
    /// preview panel can resolve a changed file's path to its current
    /// content on disk.
    pub fn set_workspace_root(&mut self, path: PathBuf) {
        self.workspace_root = Some(path);
    }

    /// Record the Driva policy the live session was launched under.
    pub fn set_driva_options(&mut self, options: DrivaOptions) {
        self.driva_options = Some(options);
    }

    /// Toggle whether minor lifecycle events (thread/turn/usage) are shown.
    pub fn toggle_minor(&mut self) {
        self.show_minor = !self.show_minor;
        if !self.entries.is_empty() && !self.is_visible(self.selected) {
            if let Some(idx) = self
                .prev_visible(self.selected)
                .or_else(|| self.next_visible(self.selected))
            {
                self.selected = idx;
                self.preview_scroll = 0;
            }
        }
    }

    /// Move to the next entry with an arrow (something beyond its bare
    /// summary), skipping over ones with nothing else to show. See
    /// [`Self::select_next_line`] to instead step one entry at a time.
    pub fn select_next(&mut self) {
        if let Some(next) = self.next_navigable(self.selected + 1) {
            self.selected = next;
            self.preview_scroll = 0;
        }
        // Re-enable follow only when the selection reaches the navigable tail.
        self.follow = !self.entries.is_empty() && self.next_navigable(self.selected + 1).is_none();
    }

    /// Move to the previous entry with an arrow; see [`Self::select_next`].
    pub fn select_prev(&mut self) {
        if let Some(prev) = self
            .selected
            .checked_sub(1)
            .and_then(|from| self.prev_navigable(from))
        {
            self.selected = prev;
            self.preview_scroll = 0;
        }
        // Moving off the tail pins the view.
        self.follow = false;
    }

    /// Move to the next visible entry regardless of whether it has anything
    /// beyond its summary — a finer-grained step than [`Self::select_next`],
    /// which skips entries with no arrow.
    pub fn select_next_line(&mut self) {
        if let Some(next) = self.next_visible(self.selected + 1) {
            self.selected = next;
            self.preview_scroll = 0;
        }
        // Re-enable follow only when the selection reaches the visible tail.
        self.follow = !self.entries.is_empty() && self.next_visible(self.selected + 1).is_none();
    }

    /// Move to the previous visible entry; see [`Self::select_next_line`].
    pub fn select_prev_line(&mut self) {
        if let Some(prev) = self
            .selected
            .checked_sub(1)
            .and_then(|from| self.prev_visible(from))
        {
            self.selected = prev;
            self.preview_scroll = 0;
        }
        // Moving off the tail pins the view.
        self.follow = false;
    }

    pub fn select_first(&mut self) {
        if let Some(first) = self.next_visible(0) {
            self.selected = first;
            self.preview_scroll = 0;
        }
        self.follow = !self.entries.is_empty() && self.next_visible(self.selected + 1).is_none();
    }

    pub fn select_last(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if let Some(last) = self.prev_visible(self.entries.len() - 1) {
            self.selected = last;
            self.preview_scroll = 0;
        }
        self.follow = true;
    }

    // --- Expansion -----------------------------------------------------------

    pub fn toggle_expand(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.selected) {
            entry.expanded = !entry.expanded;
        }
    }

    pub fn expand_selected(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.selected) {
            entry.expanded = true;
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

    // --- Focus ---------------------------------------------------------------

    pub fn enter_input(&mut self) {
        self.focus = Focus::Input;
    }

    pub fn enter_list(&mut self) {
        self.focus = Focus::List;
    }

    // --- Message editing -----------------------------------------------------

    pub fn input_char(&mut self, ch: char) {
        self.reset_history_navigation();
        self.input.push(ch);
    }

    pub fn input_backspace(&mut self) {
        self.reset_history_navigation();
        self.input.pop();
    }

    /// Delete the word immediately before the end of the buffer (`Ctrl-W`),
    /// readline-style: trailing whitespace first, then non-whitespace back
    /// to the previous word boundary (or the start of the buffer).
    pub fn input_delete_word(&mut self) {
        self.reset_history_navigation();
        let trimmed = self.input.trim_end_matches(char::is_whitespace).len();
        self.input.truncate(trimmed);
        let word_start = self
            .input
            .rfind(char::is_whitespace)
            .map(|idx| idx + 1)
            .unwrap_or(0);
        self.input.truncate(word_start);
    }

    pub fn input_newline(&mut self) {
        self.reset_history_navigation();
        self.input.push('\n');
    }

    pub fn input_clear(&mut self) {
        self.input.clear();
        self.reset_history_navigation();
    }

    /// Recall older submitted prompts, preserving the current draft so Down
    /// can return to it after walking back to the newest history entry.
    pub fn input_history_previous(&mut self) {
        if self.prompt_history.is_empty() {
            return;
        }
        let next = match self.history_cursor {
            Some(index) => index.saturating_sub(1),
            None => {
                self.history_draft.clone_from(&self.input);
                self.prompt_history.len() - 1
            }
        };
        self.history_cursor = Some(next);
        self.input.clone_from(&self.prompt_history[next]);
    }

    pub fn input_history_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 < self.prompt_history.len() {
            let next = index + 1;
            self.history_cursor = Some(next);
            self.input.clone_from(&self.prompt_history[next]);
        } else {
            self.history_cursor = None;
            self.input.clone_from(&self.history_draft);
            self.history_draft.clear();
        }
    }

    fn reset_history_navigation(&mut self) {
        self.history_cursor = None;
        self.history_draft.clear();
    }

    /// Take the trimmed message for sending, clearing the buffer. Returns
    /// `None` when the buffer holds only whitespace.
    pub fn take_message(&mut self) -> Option<String> {
        let message = self.input.trim().to_owned();
        self.input.clear();
        if message.is_empty() {
            None
        } else {
            self.prompt_history.push(message.clone());
            self.reset_history_navigation();
            Some(message)
        }
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Ask the event loop to choose a Workspace and then one of its Sessions.
    pub fn request_workspace(&mut self) {
        self.workspace_requested = true;
    }

    /// Ask the event loop to list the server's live interactions and, if the operator
    /// picks one, attach to it. The current interaction is left running on the server,
    /// not stopped: attaching only changes what this client views.
    pub fn request_interactions(&mut self) {
        self.interactions_requested = true;
    }

    /// Ask the event loop to stop the current interaction and return to the blank
    /// start screen, with no interaction viewed.
    pub fn request_reset(&mut self) {
        self.reset_requested = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_server::agent::Effort;

    fn app() -> App {
        App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "session-1",
        )
    }

    #[test]
    fn following_tracks_the_newest_entry() {
        let mut app = app();
        app.push_event(AgentEvent::TurnStarted);
        app.push_event(AgentEvent::AgentMessage { text: "hi".into() });
        assert!(app.follow);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn moving_up_pins_the_view_and_reaching_the_tail_resumes_follow() {
        let mut app = app();
        // Multi-line so every entry has detail and so is reachable by the
        // has-detail-only select_next/select_prev this test exercises.
        for _ in 0..3 {
            app.push_event(AgentEvent::AgentMessage {
                text: "x\nmore x".into(),
            });
        }
        app.select_prev();
        assert!(!app.follow);
        assert_eq!(app.selected, 1);

        // New events no longer move the selection while pinned.
        app.push_event(AgentEvent::AgentMessage {
            text: "x\nmore x".into(),
        });
        assert_eq!(app.selected, 1);

        // Walking back down to the tail re-enables follow.
        app.select_next();
        app.select_next();
        app.select_next();
        assert!(app.follow);
        assert_eq!(app.selected, app.entries.len() - 1);
    }

    #[test]
    fn moving_up_by_line_pins_the_view_and_reaching_the_tail_resumes_follow() {
        // Same follow/pin contract as select_next/select_prev, but for
        // select_next_line/select_prev_line (J/K), which move one visible
        // entry at a time regardless of whether it has detail.
        let mut app = app();
        for _ in 0..3 {
            app.push_event(AgentEvent::AgentMessage { text: "x".into() });
        }
        app.select_prev_line();
        assert!(!app.follow);
        assert_eq!(app.selected, 1);

        app.push_event(AgentEvent::AgentMessage { text: "x".into() });
        assert_eq!(app.selected, 1);

        app.select_next_line();
        app.select_next_line();
        app.select_next_line();
        assert!(app.follow);
        assert_eq!(app.selected, app.entries.len() - 1);
    }

    #[test]
    fn expansion_is_per_entry_and_bulk_toggles_work() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::AgentMessage { text: "b".into() });

        app.select_first();
        app.toggle_expand();
        assert!(app.entries[0].expanded);
        assert!(!app.entries[1].expanded);

        app.expand_all();
        assert!(app.entries.iter().all(|entry| entry.expanded));
        app.collapse_all();
        assert!(app.entries.iter().all(|entry| !entry.expanded));
    }

    #[test]
    fn status_follows_turn_lifecycle_and_captures_usage() {
        let mut app = app();
        assert_eq!(app.status, Status::Running);
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage {
                input_tokens: 7,
                ..Default::default()
            },
        });
        assert_eq!(app.status, Status::Idle);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 7);

        app.push_event(AgentEvent::UserMessage {
            text: "more".into(),
        });
        assert_eq!(app.status, Status::Running);
    }

    #[test]
    fn queued_messages_are_fifo_and_can_be_cleared() {
        let mut app = app();
        app.queue_message("first".into());
        app.queue_message("second".into());

        assert_eq!(app.queued_message_count(), 2);
        assert_eq!(app.take_queued_message(), Some("first".into()));
        assert_eq!(app.take_queued_message(), Some("second".into()));
        assert_eq!(app.take_queued_message(), None);

        app.queue_message("keep until Esc".into());
        assert_eq!(app.clear_queued_messages(), 1);
        assert_eq!(app.queued_message_count(), 0);
    }

    #[test]
    fn usage_updates_mid_turn_refresh_the_display_without_ending_the_turn() {
        // The app-server protocol reports a token-usage snapshot after every
        // step within a turn (each tool call, each model round), not just the
        // last one. Only a real `TurnCompleted` should flip the status to
        // idle; `UsageUpdated` must not, or the indicator falsely reads idle
        // while the agent is still actively working.
        let mut app = app();
        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test".into(),
        });
        assert_eq!(app.status, Status::Running);

        app.push_event(AgentEvent::UsageUpdated {
            usage: TokenUsage {
                input_tokens: 10,
                ..Default::default()
            },
        });
        assert_eq!(
            app.status,
            Status::Running,
            "a usage ping mid-turn must not end it"
        );
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 10);

        app.push_event(AgentEvent::CommandStarted {
            command: "cargo build".into(),
        });
        app.push_event(AgentEvent::UsageUpdated {
            usage: TokenUsage {
                input_tokens: 20,
                ..Default::default()
            },
        });
        assert_eq!(app.status, Status::Running);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 20);

        // The app-server's real end-of-turn signal carries no usage of its
        // own; the last reported usage must survive it, not reset to zero.
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        assert_eq!(app.status, Status::Idle);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 20);
    }

    #[test]
    fn a_tool_completion_replaces_its_started_row_and_recovers_the_tool_name() {
        // Claude's `tool_result` only ever repeats the `tool_use_id`, never
        // the tool's name, so the merge must pull the name back from the
        // matching `ToolStarted` row rather than leaving the placeholder id
        // on screen, and it must not append a second line.
        let mut app = app();
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "ok".into(),
        });

        assert_eq!(app.entries.len(), 1);
        assert_eq!(
            app.entries[0].event,
            AgentEvent::ToolCompleted {
                id: "toolu_1".into(),
                name: "Bash".into(),
                detail: "{\"command\":\"cargo test\"}".into(),
                status: "completed".into(),
                output: "ok".into(),
            }
        );
    }

    #[test]
    fn a_clean_edit_completion_is_dropped_since_the_file_changed_row_already_shows_it() {
        // Edit/Write/MultiEdit surface as `FileChanged` at start, not
        // `ToolStarted`, so their `ToolCompleted` used to find no match and
        // fall through to a second, id-only line. A clean result adds
        // nothing the `FileChanged` row didn't already show.
        let mut app = app();
        app.push_event(AgentEvent::FileChanged {
            id: "toolu_2".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ edit @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_2".into(),
            name: "toolu_2".into(),
            detail: String::new(),
            status: "completed".into(),
            output: String::new(),
        });

        assert_eq!(app.entries.len(), 1);
        assert!(matches!(app.entries[0].event, AgentEvent::FileChanged { .. }));
    }

    #[test]
    fn a_failed_edit_completion_replaces_the_file_changed_row_with_a_visible_error() {
        let mut app = app();
        app.push_event(AgentEvent::FileChanged {
            id: "toolu_3".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ edit @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_3".into(),
            name: "toolu_3".into(),
            detail: String::new(),
            status: "error".into(),
            output: "old_string not found".into(),
        });

        assert_eq!(app.entries.len(), 1);
        assert_eq!(
            app.entries[0].event,
            AgentEvent::Error {
                message: "src/lib.rs: old_string not found".into(),
            }
        );
    }

    #[test]
    fn ending_is_terminal_for_events_but_not_can_send() {
        let mut app = app();
        app.on_ended(InteractionEnd {
            exit_code: Some(0),
            error: None,
        });
        assert_eq!(
            app.status,
            Status::Ended {
                exit_code: Some(0),
                error: None
            }
        );
        // `can_send` only flags the input box's title; the event loop still
        // lets a new message resume an ended Session through its provider.
        assert!(!app.can_send());
        // A late event does not revive an ended session.
        app.push_event(AgentEvent::AgentMessage {
            text: "late".into(),
        });
        assert!(matches!(app.status, Status::Ended { .. }));
    }

    #[test]
    fn pending_opens_in_input_focus_with_no_session_yet_and_allows_sending() {
        let app = App::pending(Selection::new(Provider::Codex));
        assert_eq!(app.status, Status::Pending);
        assert_eq!(app.focus, Focus::Input);
        assert!(app.session_id.is_empty());
        assert!(app.input.is_empty());
        // The message box must not read "session ended" before anything ran.
        assert!(app.can_send());
    }

    #[test]
    fn set_input_prefills_the_message_box_for_the_operator_to_edit_or_send() {
        let mut app = App::pending(Selection::new(Provider::Codex));
        app.set_input("earlier session's transcript".into());
        assert_eq!(app.input, "earlier session's transcript");
        assert_eq!(
            app.take_message(),
            Some("earlier session's transcript".into())
        );
    }

    /// The whole point of the start screen: all three of agent, model, and
    /// effort are still open, and picking them only records what the first
    /// message will launch.
    #[test]
    fn the_launcher_records_a_provider_model_and_effort_without_launching() {
        let mut app = App::pending(Selection::new(Provider::Codex));
        assert!(app.can_configure_launch());
        app.open_launcher();
        let launcher = app.launcher.as_mut().expect("the picker is reachable");

        // Provider column: move to Claude Code.
        assert_eq!(launcher.column, LaunchColumn::Provider);
        launcher.next();
        launcher.next();
        assert_eq!(launcher.provider(), Provider::Claude);

        // Model column: every row is a model, and it opens on the agent's declared
        // default, so one step lands on the entry after that.
        launcher.next_column();
        assert_eq!(launcher.selection().model, Provider::Claude.default_model());
        launcher.next();
        let model = Provider::Claude.models()[default_model_row(Provider::Claude) + 1].to_owned();
        assert_eq!(launcher.selection().model, model);

        // Effort column, likewise — the ladder itself, no extra row.
        launcher.next_column();
        assert_eq!(launcher.column, LaunchColumn::Effort);
        let effort = Provider::Claude.efforts()[0];
        while launcher.selection().effort != effort {
            launcher.prev();
        }
        assert_eq!(launcher.selection().effort, effort);

        app.confirm_launcher();
        assert!(app.launcher.is_none());
        assert_eq!(app.selection.provider, Provider::Claude);
        assert_eq!(app.selection.model, model);
        assert_eq!(app.selection.effort, effort);
        // Nothing was launched or sent by the picking itself.
        assert_eq!(app.status, Status::Pending);
        assert!(app.session_id.is_empty());
        assert!(app.entries.is_empty());
    }

    #[test]
    fn the_launcher_opens_on_the_current_selection_and_cancelling_keeps_it() {
        let selection = Selection::parse("claude:claude-sonnet-5/xhigh").unwrap();
        let mut app = App::pending(selection.clone());
        app.open_launcher();
        let launcher = app.launcher.as_ref().unwrap();
        assert_eq!(launcher.selection(), selection, "opens on what is chosen");

        app.launcher.as_mut().unwrap().next();
        app.cancel_launcher();
        assert!(app.launcher.is_none());
        assert_eq!(app.selection, selection, "cancelling changes nothing");
    }

    /// A model the catalog no longer lists can still arrive from stored state,
    /// and confirming must not silently replace it. The picker carries it as
    /// its own row; it cannot author one.
    #[test]
    fn a_model_outside_the_catalog_is_carried_as_its_own_row() {
        let selection = Selection::parse("claude:claude-opus-4-1-20250805").unwrap();
        let mut app = App::pending(selection.clone());
        app.open_launcher();
        let launcher = app.launcher.as_ref().unwrap();
        assert_eq!(
            launcher.carried_model.as_deref(),
            Some("claude-opus-4-1-20250805")
        );
        assert_eq!(
            launcher.model,
            Provider::Claude.models().len(),
            "carried last, after the catalog"
        );
        assert_eq!(launcher.selection().model, selection.model);

        // Confirming keeps the model rather than falling back to a catalogued
        // one — but it does pin an effort, since the picker has no row for
        // leaving that to the agent. The launch asked for none, so it lands on
        // the opening row.
        app.confirm_launcher();
        assert_eq!(app.selection.model, selection.model);
        assert_eq!(app.selection.effort, Effort::High);

        // Walking up into the catalog and back down reaches the carried model
        // again: it is an ordinary row, just not one the picker could write.
        app.open_launcher();
        let launcher = app.launcher.as_mut().unwrap();
        launcher.next_column();
        launcher.prev();
        assert_eq!(
            launcher.selection().model,
            *Provider::Claude.models().last().unwrap()
        );
        launcher.next();
        assert_eq!(launcher.selection().model, selection.model);
    }

    /// With no row standing for "whatever the agent is configured for", every
    /// row of every column is a concrete choice — so whatever the picker is
    /// opened on, confirming it pins both a model and an effort.
    #[test]
    fn the_picker_always_pins_a_model_and_an_effort() {
        for provider in PROVIDERS {
            let mut launcher = Launcher::from_selection(&Selection::new(provider));
            // A selection always pins both, and the picker opens on the rows
            // naming them.
            let opened = launcher.selection();
            assert_eq!(opened.model, provider.default_model());
            assert_eq!(opened.effort, provider.default_effort());

            // And no reachable row in either column yields an absent value.
            for column in [LaunchColumn::Model, LaunchColumn::Effort] {
                launcher.column = column;
                for _ in 0..provider.models().len() + provider.efforts().len() {
                    let selection = launcher.selection();
                    assert!(
                        provider.models().contains(&selection.model.as_str()),
                        "{provider:?} {column:?} reached a model outside the catalog"
                    );
                    assert!(
                        provider.efforts().contains(&selection.effort),
                        "{provider:?} {column:?} reached an effort outside the ladder"
                    );
                    launcher.next();
                }
            }
        }
    }

    /// Switching agents drops the carried model with everything else: it named a
    /// model of the agent the picker was opened on.
    #[test]
    fn changing_provider_drops_a_carried_model() {
        let mut launcher =
            Launcher::from_selection(&Selection::parse("claude:claude-opus-4-1-20250805").unwrap());
        assert!(launcher.carried_model.is_some());

        launcher.prev(); // in the provider column, back towards codex
        assert_eq!(launcher.carried_model, None);
        // The new agent's own declared default stands in for it.
        assert_eq!(
            launcher.selection().model,
            launcher.provider().default_model()
        );
        // And the column is back to just that agent's catalog.
        launcher.next_column();
        assert_eq!(launcher.model_rows(), launcher.provider().models().len());
    }

    /// The two agents' model catalogs and effort ladders are unrelated, so a
    /// choice made for one must not carry an index across to the other.
    #[test]
    fn changing_provider_falls_back_to_the_new_agents_defaults() {
        let mut launcher =
            Launcher::from_selection(&Selection::parse("claude:claude-opus-5/max").unwrap());
        assert_eq!(launcher.selection().name(), "claude:claude-opus-5/max");

        launcher.prev(); // in the provider column, back towards codex
        let selection = launcher.selection();
        assert_ne!(selection.provider, Provider::Claude);
        // Neither the model nor the effort carries across by index: each falls
        // back to the new agent's own declared default.
        assert_eq!(selection.model, selection.provider.default_model());
        assert_eq!(selection.effort, selection.provider.default_effort());
        // And `max` is not offered at all under codex, so it cannot be reached
        // by walking the column either.
        assert!(!launcher.provider().efforts().contains(&Effort::Max));
    }

    /// Once a session is up, its agent and model are facts about a running
    /// process; the picker is a property of the *next* one.
    #[test]
    fn the_launcher_is_unreachable_once_something_has_been_launched() {
        let mut app = app();
        assert_eq!(app.status, Status::Running);
        assert!(!app.can_configure_launch());
        app.open_launcher();
        assert!(app.launcher.is_none());
    }

    /// A live or replayed Session carries the exact selection it was created
    /// with, independently of the client's saved default for new Sessions.
    #[test]
    fn a_session_keeps_its_recorded_selection() {
        let app = App::new(
            styra_server::agent::Selection::parse("claude:opus/xhigh").unwrap(),
            "session-1",
        );
        assert_eq!(
            app.selection,
            Selection::parse("claude:opus/xhigh").unwrap()
        );
    }

    /// What the status line names before the agent has spoken: the launch it was
    /// started with, model and effort included, marked as not yet confirmed.
    #[test]
    fn the_launch_label_falls_back_to_the_requested_selection() {
        let app = App::new(
            styra_server::agent::Selection::parse("claude:opus/max").unwrap(),
            "s-1",
        );
        let label = app.launch_label();
        assert_eq!(label.agent, "claude");
        assert_eq!(label.model.as_deref(), Some("opus"));
        assert_eq!(label.effort.as_deref(), Some("max"));
        assert!(
            !label.model_reported,
            "nothing has been reported by the agent yet"
        );
        assert!(!label.effort_reported);

        // Short launch syntax is normalized to the provider's declared defaults.
        let app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s-2",
        );
        let label = app.launch_label();
        assert_eq!(label.agent, "codex");
        assert_eq!(
            label.model.as_deref(),
            Some(Provider::Codex.default_model())
        );
        assert_eq!(
            label.effort.as_deref(),
            Some(Provider::Codex.default_effort().as_str())
        );
    }

    /// The agent's own report is what is actually running, so it replaces the
    /// launch request.
    #[test]
    fn a_reported_model_and_effort_replace_the_requested_ones() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s-1",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(label.effort.as_deref(), Some("high"));
        assert!(label.model_reported);
        assert!(label.effort_reported);

        // A launch that asked for something else is overruled by the fact.
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex:gpt-5.6-luna/low").unwrap(),
            "s-2",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("xhigh".into()),
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(label.effort.as_deref(), Some("xhigh"));
    }

    /// Claude Code names a model but never an effort, so the effort the session
    /// was launched with must survive its report rather than being blanked.
    #[test]
    fn an_unreported_effort_keeps_the_one_the_session_was_launched_with() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("claude:opus/max").unwrap(),
            "s-1",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "s-1".into(),
            model: Some("claude-opus-4-8".into()),
            effort: None,
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("claude-opus-4-8"));
        assert!(label.model_reported);
        // The effort is the launch's own word, and is not claimed as the
        // agent's.
        assert_eq!(label.effort.as_deref(), Some("max"));
        assert!(!label.effort_reported);

        // A thread reported with neither leaves the display as it was.
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "s-2",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: None,
            effort: None,
        });
        let label = app.launch_label();
        assert_eq!(label.model.as_deref(), Some("gpt-5.6-sol"));
        assert!(!label.model_reported);
    }

    #[test]
    fn request_workspace_sets_a_flag_for_the_event_loop_to_observe() {
        let mut app = app();
        assert!(!app.workspace_requested);
        app.request_workspace();
        assert!(app.workspace_requested);
    }

    #[test]
    fn request_interactions_and_reset_set_flags_for_the_event_loop_to_observe() {
        let mut app = app();
        assert!(!app.interactions_requested);
        assert!(!app.reset_requested);
        app.request_interactions();
        app.request_reset();
        assert!(app.interactions_requested);
        assert!(app.reset_requested);
    }

    #[test]
    fn raw_view_toggles_and_selects_from_the_tail() {
        use styra_server::{Direction, RawLine};
        let mut app = app();
        assert_eq!(app.view, View::Events);
        app.toggle_raw();
        assert_eq!(app.view, View::Raw);

        for i in 0..5 {
            app.push_raw(RawLine {
                direction: Direction::FromAgent,
                text: format!("line {i}"),
            });
        }
        assert_eq!(app.raw_selected, 4, "starts pinned to the tail");
        assert!(app.raw_follow);

        app.raw_select_prev();
        assert_eq!(app.raw_selected, 3);
        assert!(!app.raw_follow);
        // A new line while a specific line is selected keeps that same line
        // in view rather than yanking to the new tail.
        app.push_raw(RawLine {
            direction: Direction::ToAgent,
            text: "new".into(),
        });
        assert_eq!(app.raw_selected, 3);

        app.raw_select_last();
        assert_eq!(app.raw_selected, 5);
        assert!(app.raw_follow);
        app.raw_select_first();
        assert_eq!(app.raw_selected, 0);
        assert!(!app.raw_follow);
    }

    #[test]
    fn entering_raw_view_focuses_the_selected_entrys_wire_line() {
        use styra_server::{Direction, RawLine};
        let mut app = app();
        for i in 0..3 {
            app.push_raw(RawLine {
                direction: Direction::FromAgent,
                text: format!("{{\"n\":{i}}}"),
            });
            app.push_event(AgentEvent::AgentMessage {
                text: format!("message {i}"),
            });
        }
        // Step off the tail so the list is no longer following, landing on
        // the middle entry.
        app.select_prev_line();
        assert_eq!(app.selected, 1);
        assert!(!app.follow);

        app.toggle_raw();
        assert_eq!(
            app.raw_selected, 1,
            "focuses the wire line behind the selected entry"
        );
        assert!(!app.raw_follow);
    }

    #[test]
    fn log_view_toggles_independently_and_scrolls() {
        use styra_server::LogEntry;
        let mut app = app();
        app.toggle_raw();
        assert_eq!(app.view, View::Raw);
        // Toggling the log from the raw view switches to it, not back to events.
        app.toggle_log();
        assert_eq!(app.view, View::Log);
        app.toggle_log();
        assert_eq!(app.view, View::Events);

        for i in 0..4 {
            app.push_log(LogEntry::info(format!("entry {i}")));
        }
        assert_eq!(app.log_scroll_back, 0);
        app.log_scroll_up();
        assert_eq!(app.log_scroll_back, 1);
        app.push_log(LogEntry::warn("more"));
        assert_eq!(app.log_scroll_back, 2, "scrolled-up view stays put");
        app.log_to_bottom();
        assert_eq!(app.log_scroll_back, 0);
    }

    #[test]
    fn transcript_view_toggles_independently_and_scrolls_from_the_top() {
        let mut app = app();
        app.toggle_raw();
        assert_eq!(app.view, View::Raw);
        // Toggling the transcript from the raw view switches to it, not back
        // to events.
        app.toggle_transcript();
        assert_eq!(app.view, View::Transcript);
        app.toggle_transcript();
        assert_eq!(app.view, View::Events);

        app.toggle_transcript();
        assert_eq!(
            app.transcript_scroll, 0,
            "starts at the beginning, not the tail"
        );

        app.transcript_scroll_down();
        app.transcript_scroll_down();
        assert_eq!(app.transcript_scroll, 2);
        app.transcript_scroll_up();
        assert_eq!(app.transcript_scroll, 1);

        app.transcript_to_bottom();
        assert_eq!(app.transcript_scroll, u16::MAX);
        app.transcript_to_top();
        assert_eq!(app.transcript_scroll, 0);
    }

    #[test]
    fn minor_events_are_hidden_and_skipped_by_navigation() {
        let mut app = app();
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: None,
            effort: None,
        });
        // Multi-line so each entry has detail beyond its summary, and so
        // qualifies for the has-detail navigation this test also exercises.
        app.push_event(AgentEvent::AgentMessage {
            text: "a\nmore a".into(),
        });
        app.push_event(AgentEvent::TurnStarted);
        app.push_event(AgentEvent::AgentMessage {
            text: "b\nmore b".into(),
        });
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });

        // Hidden by default; no toggle needed to get here.
        assert!(!app.show_minor);

        app.select_first();
        assert_eq!(
            app.entries[app.selected].event,
            AgentEvent::AgentMessage {
                text: "a\nmore a".into()
            }
        );

        app.select_next();
        assert_eq!(
            app.entries[app.selected].event,
            AgentEvent::AgentMessage {
                text: "b\nmore b".into()
            }
        );

        // No more visible entries after "b"; select_next is a no-op.
        app.select_next();
        assert_eq!(
            app.entries[app.selected].event,
            AgentEvent::AgentMessage {
                text: "b\nmore b".into()
            }
        );

        app.select_prev();
        assert_eq!(
            app.entries[app.selected].event,
            AgentEvent::AgentMessage {
                text: "a\nmore a".into()
            }
        );

        app.toggle_minor();
        assert!(app.show_minor);
    }

    #[test]
    fn select_next_and_prev_skip_entries_with_no_detail_beyond_their_summary() {
        let mut app = app();
        // A single line of text is entirely a restatement of the summary, so
        // this entry has no arrow and should not be a stop for j/k.
        app.push_event(AgentEvent::AgentMessage {
            text: "no detail here".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "has detail\nsecond line".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "also no detail".into(),
        });
        assert!(!app.entries[0].has_detail());
        assert!(app.entries[1].has_detail());
        assert!(!app.entries[2].has_detail());

        // `select_first` (bound to `g`) is unaffected by the has-detail
        // restriction: it lands on the very first visible entry regardless.
        app.select_first();
        assert_eq!(app.selected, 0);

        // The only entry with detail is index 1; select_next skips index 0's
        // lack of detail to land there, then has nothing further to skip to.
        app.select_next();
        assert_eq!(app.selected, 1);
        app.select_next();
        assert_eq!(app.selected, 1);
        // Equally, there is no navigable entry before it to skip back to.
        app.select_prev();
        assert_eq!(app.selected, 1);

        // J/K ignore the has-detail restriction and move one line at a time.
        app.select_next_line();
        assert_eq!(app.selected, 2);
        app.select_next_line();
        assert_eq!(app.selected, 2, "already at the last visible entry");
        app.select_prev_line();
        assert_eq!(app.selected, 1);
        app.select_prev_line();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn file_events_are_navigable_even_without_fold_detail() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage {
            text: "plain summary".into(),
        });
        app.push_event(AgentEvent::FileChanged {
            id: String::new(),
            paths: vec!["notes.txt".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "another plain summary".into(),
        });
        assert!(!app.entries[1].has_detail());

        app.select_first();
        app.select_next();
        assert_eq!(app.selected, 1);
        app.select_next();
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn preview_scroll_resets_when_selection_changes() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage {
            text: "first\nbody".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "second\nbody".into(),
        });
        app.select_first();
        app.preview_page_down();
        assert_eq!(app.preview_scroll, 10);

        app.select_next();
        assert_eq!(app.selected, 1);
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn toggling_minor_off_moves_selection_off_a_hidden_entry() {
        let mut app = app();
        app.toggle_minor(); // show minor events so follow can land on one
        assert!(app.show_minor);

        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::TurnStarted);
        // Selection sits on the just-pushed minor entry via follow.
        assert_eq!(app.selected, 1);

        app.toggle_minor(); // hide them again
        assert!(!app.show_minor);
        assert!(app.is_visible(app.selected));
        assert_eq!(
            app.entries[app.selected].event,
            AgentEvent::AgentMessage { text: "a".into() }
        );
    }

    #[test]
    fn workspace_root_is_unset_until_the_host_records_it() {
        let mut app = app();
        assert_eq!(app.workspace_root, None);
        app.set_workspace_root(PathBuf::from("/home/op/project"));
        assert_eq!(app.workspace_root, Some(PathBuf::from("/home/op/project")));
    }

    #[test]
    fn driva_options_are_unset_until_the_host_records_them_and_the_view_toggles() {
        use styra_server::DrivaOptions;
        use styra_server::{Mount, MountAccess};

        let mut app = app();
        assert_eq!(app.driva_options, None);
        app.set_driva_options(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into(), "app-server".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: true,
            mounts: vec![Mount::Bind {
                source: PathBuf::from("/home/op/project"),
                destination: PathBuf::from("/tmp/styra/workspace"),
                access: MountAccess::ReadWrite,
            }],
        });
        assert_eq!(
            app.driva_options.as_ref().unwrap().isolation_backend,
            "bwrap"
        );

        assert_eq!(app.view, View::Events);
        app.toggle_driva();
        assert_eq!(app.view, View::Driva);
        app.toggle_driva();
        assert_eq!(app.view, View::Events);
    }

    #[test]
    fn preview_toggles_independently_of_other_view_state() {
        let mut app = app();
        assert!(!app.show_preview);
        app.toggle_preview();
        assert!(app.show_preview);
        app.toggle_preview();
        assert!(!app.show_preview);
    }

    #[test]
    fn fullscreen_preview_toggles_the_view_and_is_independent_of_the_side_panel() {
        let mut app = app();
        assert_eq!(app.view, View::Events);
        app.toggle_fullscreen_preview();
        assert_eq!(app.view, View::Preview);
        // The side-panel flag (bound to lowercase `p`) is a separate toggle;
        // the full-screen shortcut (`P`) does not touch it.
        assert!(!app.show_preview);
        app.toggle_fullscreen_preview();
        assert_eq!(app.view, View::Events);
    }

    #[test]
    fn focus_toggles_and_input_edits() {
        let mut app = app();
        assert_eq!(app.focus, Focus::List);
        app.enter_input();
        assert_eq!(app.focus, Focus::Input);

        app.input_char('h');
        app.input_char('i');
        app.input_newline();
        app.input_char('!');
        app.input_backspace();
        assert_eq!(app.input, "hi\n");
        assert_eq!(app.take_message(), Some("hi".into()));
        assert!(app.input.is_empty());
        assert_eq!(app.take_message(), None);
    }

    #[test]
    fn input_delete_word_removes_the_trailing_word_readline_style() {
        let mut app = app();
        app.set_input("fix the flaky test".into());
        app.input_delete_word();
        assert_eq!(app.input, "fix the flaky ");
        app.input_delete_word();
        assert_eq!(app.input, "fix the ");

        // Trailing whitespace with nothing after it is consumed first, along
        // with the word before it, in one call — not two.
        app.set_input("one two   ".into());
        app.input_delete_word();
        assert_eq!(app.input, "one ");

        // Deleting past the first word empties the buffer rather than
        // panicking or leaving a dangling boundary.
        app.input_delete_word();
        assert_eq!(app.input, "");
        app.input_delete_word();
        assert_eq!(app.input, "");

        // Spans a newline like any other whitespace.
        app.set_input("hello\nworld".into());
        app.input_delete_word();
        assert_eq!(app.input, "hello\n");
    }

    #[test]
    fn input_history_moves_through_prompts_and_restores_the_draft() {
        let mut app = app();
        app.set_input("first".into());
        assert_eq!(app.take_message(), Some("first".into()));
        app.set_input("second".into());
        assert_eq!(app.take_message(), Some("second".into()));
        app.set_input("unfinished draft".into());

        app.input_history_previous();
        assert_eq!(app.input, "second");
        app.input_history_previous();
        assert_eq!(app.input, "first");
        app.input_history_previous();
        assert_eq!(app.input, "first");
        app.input_history_next();
        assert_eq!(app.input, "second");
        app.input_history_next();
        assert_eq!(app.input, "unfinished draft");
    }

    #[test]
    fn editing_a_recalled_prompt_leaves_history_navigation() {
        let mut app = app();
        app.set_input("original".into());
        app.take_message();
        app.input_history_previous();
        app.input_char('!');
        app.input_history_next();
        assert_eq!(app.input, "original!");
    }
}
