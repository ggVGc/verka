//! Application state: the event list, selection, expansion, focus, the message
//! buffer, and session status.
//!
//! This module is pure state and transitions — no terminal, no threads, no IO —
//! so the whole interaction model is unit-testable. [`crate::ui`] renders it and
//! `main` feeds it input and session updates.

use crate::event::{AgentEvent, DetailBlock, TokenUsage};
use crate::session::{DrivaOptions, LogEntry, RawLine, SessionEnd};
use std::path::PathBuf;

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
    Ended { exit_code: Option<i32>, error: Option<String> },
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Pending => "not started".into(),
            Status::Running => "running".into(),
            Status::Idle => "idle".into(),
            Status::Stopped => "stopped".into(),
            Status::Ended { error: Some(_), .. } => "failed".into(),
            Status::Ended { exit_code: Some(code), .. } => format!("ended ({code})"),
            Status::Ended { .. } => "ended".into(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Status::Pending | Status::Running | Status::Idle)
    }
}

/// One event in the list, with its fold state.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub event: AgentEvent,
    pub expanded: bool,
}

impl Entry {
    /// Whether this entry has anything to show beyond its one-line summary —
    /// the same test that decides whether the list shows a fold arrow next
    /// to it. `crate::ui`'s detail rendering always drops the body's first
    /// line (it invariably restates the summary — the command, the
    /// message's first line, ...), so one line of detail alone doesn't
    /// count; this mirrors that exactly rather than checking the raw,
    /// undropped `AgentEvent::detail()` output.
    pub fn has_detail(&self) -> bool {
        detail_line_count(&self.event) > 0
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

/// The complete UI state.
pub struct App {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub focus: Focus,
    pub view: View,
    pub input: String,
    pub status: Status,
    /// When true, the selection tracks the newest entry as events arrive.
    pub follow: bool,
    /// When false, minor lifecycle events (thread/turn/usage) are hidden from
    /// the list and skipped by navigation.
    pub show_minor: bool,
    /// When true, a side panel shows the full expanded content of the
    /// selected entry, independent of whether it is folded in the list.
    pub show_preview: bool,
    pub profile_name: String,
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
    /// Lines scrolled back from the bottom of the raw view; 0 tracks the tail.
    pub raw_scroll_back: u16,
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
    /// Set when the operator asks to switch to a different stored session;
    /// the event loop observes it and opens the session picker.
    pub switch_requested: bool,
}

impl App {
    pub fn new(profile_name: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            focus: Focus::List,
            view: View::Events,
            input: String::new(),
            status: Status::Running,
            follow: true,
            show_minor: false,
            show_preview: false,
            profile_name: profile_name.into(),
            session_id: session_id.into(),
            workspace_root: None,
            driva_options: None,
            latest_usage: None,
            raw: Vec::new(),
            raw_scroll_back: 0,
            log: Vec::new(),
            log_scroll_back: 0,
            transcript_scroll: 0,
            should_quit: false,
            switch_requested: false,
        }
    }

    /// A fresh App with no agent process launched yet: no journal or session
    /// id exists until the operator submits a first message, at which point
    /// the event loop spawns the session and fills those in. Used both for a
    /// bare startup with no seed prompt and after picking a session to
    /// switch to (see `set_input` for prefilling that pick's transcript).
    /// Opens directly in input focus, since typing there is the only thing
    /// that moves the session forward.
    pub fn pending(profile_name: impl Into<String>) -> Self {
        let mut app = Self::new(profile_name, String::new());
        app.status = Status::Pending;
        app.focus = Focus::Input;
        app
    }

    /// Replace the message box's contents outright: used to prefill it with
    /// a switched-from session's rendered transcript, or to restore a
    /// message that failed to launch so it isn't lost.
    pub fn set_input(&mut self, text: String) {
        self.input = text;
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

    /// Append a verbatim wire line. When the operator has scrolled up, the
    /// view is kept pinned to the same content; otherwise it tracks the tail.
    pub fn push_raw(&mut self, line: RawLine) {
        self.raw.push(line);
        if self.raw_scroll_back > 0 {
            self.raw_scroll_back = self.raw_scroll_back.saturating_add(1);
        }
    }

    /// Toggle the raw wire view on, or back to the event list.
    pub fn toggle_raw(&mut self) {
        self.view = if self.view == View::Raw {
            View::Events
        } else {
            View::Raw
        };
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

    pub fn raw_scroll_up(&mut self) {
        let max = self.raw.len().saturating_sub(1) as u16;
        self.raw_scroll_back = self.raw_scroll_back.saturating_add(1).min(max);
    }

    pub fn raw_scroll_down(&mut self) {
        self.raw_scroll_back = self.raw_scroll_back.saturating_sub(1);
    }

    pub fn raw_to_top(&mut self) {
        self.raw_scroll_back = self.raw.len().saturating_sub(1) as u16;
    }

    pub fn raw_to_bottom(&mut self) {
        self.raw_scroll_back = 0;
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

    // --- Ingesting session updates -----------------------------------------

    /// Append a decoded event, advancing status and, while following, selection.
    pub fn push_event(&mut self, event: AgentEvent) {
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
            AgentEvent::UserMessage { .. }
            | AgentEvent::TurnStarted
            | AgentEvent::CommandStarted { .. }
            | AgentEvent::ToolStarted { .. }
            | AgentEvent::AgentMessage { .. }
            | AgentEvent::PlanUpdated { .. } => {
                if self.status.is_active() {
                    self.status = Status::Running;
                }
            }
            _ => {}
        }
        self.entries.push(Entry { event, expanded: false });
        if self.follow {
            self.selected = self.entries.len() - 1;
        }
    }

    /// Record that the session ended. This is terminal regardless of `Stopped`.
    pub fn on_ended(&mut self, end: SessionEnd) {
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
    /// a fold arrow (something beyond its bare summary). Entries with
    /// nothing to show beyond that arrow-less summary (e.g. a bare `turn
    /// started` marker) are skipped so quick review only stops on entries
    /// worth looking at; `J`/`K` still visit them one line at a time.
    fn is_navigable(&self, idx: usize) -> bool {
        self.is_visible(idx) && self.entries[idx].has_detail()
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
            if let Some(idx) = self.prev_visible(self.selected).or_else(|| self.next_visible(self.selected)) {
                self.selected = idx;
            }
        }
    }

    /// Move to the next entry with an arrow (something beyond its bare
    /// summary), skipping over ones with nothing else to show. See
    /// [`Self::select_next_line`] to instead step one entry at a time.
    pub fn select_next(&mut self) {
        if let Some(next) = self.next_navigable(self.selected + 1) {
            self.selected = next;
        }
        // Re-enable follow only when the selection reaches the navigable tail.
        self.follow = !self.entries.is_empty() && self.next_navigable(self.selected + 1).is_none();
    }

    /// Move to the previous entry with an arrow; see [`Self::select_next`].
    pub fn select_prev(&mut self) {
        if let Some(prev) = self.selected.checked_sub(1).and_then(|from| self.prev_navigable(from)) {
            self.selected = prev;
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
        }
        // Re-enable follow only when the selection reaches the visible tail.
        self.follow = !self.entries.is_empty() && self.next_visible(self.selected + 1).is_none();
    }

    /// Move to the previous visible entry; see [`Self::select_next_line`].
    pub fn select_prev_line(&mut self) {
        if let Some(prev) = self.selected.checked_sub(1).and_then(|from| self.prev_visible(from)) {
            self.selected = prev;
        }
        // Moving off the tail pins the view.
        self.follow = false;
    }

    pub fn select_first(&mut self) {
        if let Some(first) = self.next_visible(0) {
            self.selected = first;
        }
        self.follow = !self.entries.is_empty() && self.next_visible(self.selected + 1).is_none();
    }

    pub fn select_last(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if let Some(last) = self.prev_visible(self.entries.len() - 1) {
            self.selected = last;
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

    pub fn collapse_selected(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.selected) {
            entry.expanded = false;
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

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::List => Focus::Input,
            Focus::Input => Focus::List,
        };
    }

    // --- Message editing -----------------------------------------------------

    pub fn input_char(&mut self, ch: char) {
        self.input.push(ch);
    }

    pub fn input_backspace(&mut self) {
        self.input.pop();
    }

    /// Delete the word immediately before the end of the buffer (`Ctrl-W`),
    /// readline-style: trailing whitespace first, then non-whitespace back
    /// to the previous word boundary (or the start of the buffer).
    pub fn input_delete_word(&mut self) {
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
        self.input.push('\n');
    }

    pub fn input_clear(&mut self) {
        self.input.clear();
    }

    /// Take the trimmed message for sending, clearing the buffer. Returns
    /// `None` when the buffer holds only whitespace.
    pub fn take_message(&mut self) -> Option<String> {
        let message = self.input.trim().to_owned();
        self.input.clear();
        if message.is_empty() {
            None
        } else {
            Some(message)
        }
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Ask the event loop to open the session picker and, if the operator
    /// picks one, switch to it: stop this session, launch a fresh one seeded
    /// with the picked session's rendered transcript.
    pub fn request_switch(&mut self) {
        self.switch_requested = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new("codex", "session-1")
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
            app.push_event(AgentEvent::AgentMessage { text: "x\nmore x".into() });
        }
        app.select_prev();
        assert!(!app.follow);
        assert_eq!(app.selected, 1);

        // New events no longer move the selection while pinned.
        app.push_event(AgentEvent::AgentMessage { text: "x\nmore x".into() });
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
            usage: TokenUsage { input_tokens: 7, ..Default::default() },
        });
        assert_eq!(app.status, Status::Idle);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 7);

        app.push_event(AgentEvent::UserMessage { text: "more".into() });
        assert_eq!(app.status, Status::Running);
    }

    #[test]
    fn usage_updates_mid_turn_refresh_the_display_without_ending_the_turn() {
        // The app-server protocol reports a token-usage snapshot after every
        // step within a turn (each tool call, each model round), not just the
        // last one. Only a real `TurnCompleted` should flip the status to
        // idle; `UsageUpdated` must not, or the indicator falsely reads idle
        // while the agent is still actively working.
        let mut app = app();
        app.push_event(AgentEvent::CommandStarted { command: "cargo test".into() });
        assert_eq!(app.status, Status::Running);

        app.push_event(AgentEvent::UsageUpdated {
            usage: TokenUsage { input_tokens: 10, ..Default::default() },
        });
        assert_eq!(app.status, Status::Running, "a usage ping mid-turn must not end it");
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 10);

        app.push_event(AgentEvent::CommandStarted { command: "cargo build".into() });
        app.push_event(AgentEvent::UsageUpdated {
            usage: TokenUsage { input_tokens: 20, ..Default::default() },
        });
        assert_eq!(app.status, Status::Running);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 20);

        // The app-server's real end-of-turn signal carries no usage of its
        // own; the last reported usage must survive it, not reset to zero.
        app.push_event(AgentEvent::TurnCompleted { usage: TokenUsage::default() });
        assert_eq!(app.status, Status::Idle);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 20);
    }

    #[test]
    fn ending_is_terminal_and_disables_sending() {
        let mut app = app();
        app.on_ended(SessionEnd { exit_code: Some(0), error: None });
        assert_eq!(app.status, Status::Ended { exit_code: Some(0), error: None });
        assert!(!app.can_send());
        // A late event does not revive an ended session.
        app.push_event(AgentEvent::AgentMessage { text: "late".into() });
        assert!(matches!(app.status, Status::Ended { .. }));
    }

    #[test]
    fn pending_opens_in_input_focus_with_no_session_yet_and_allows_sending() {
        let app = App::pending("codex");
        assert_eq!(app.status, Status::Pending);
        assert_eq!(app.focus, Focus::Input);
        assert!(app.session_id.is_empty());
        assert!(app.input.is_empty());
        // The message box must not read "session ended" before anything ran.
        assert!(app.can_send());
    }

    #[test]
    fn set_input_prefills_the_message_box_for_the_operator_to_edit_or_send() {
        let mut app = App::pending("codex");
        app.set_input("earlier session's transcript".into());
        assert_eq!(app.input, "earlier session's transcript");
        assert_eq!(app.take_message(), Some("earlier session's transcript".into()));
    }

    #[test]
    fn request_switch_sets_a_flag_for_the_event_loop_to_observe() {
        let mut app = app();
        assert!(!app.switch_requested);
        app.request_switch();
        assert!(app.switch_requested);
    }

    #[test]
    fn raw_view_toggles_and_scrolls_from_the_tail() {
        use crate::session::{Direction, RawLine};
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
        assert_eq!(app.raw_scroll_back, 0, "starts pinned to the tail");

        app.raw_scroll_up();
        assert_eq!(app.raw_scroll_back, 1);
        // A new line while scrolled up keeps the same content in view.
        app.push_raw(RawLine { direction: Direction::ToAgent, text: "new".into() });
        assert_eq!(app.raw_scroll_back, 2);

        app.raw_to_bottom();
        assert_eq!(app.raw_scroll_back, 0);
        app.raw_to_top();
        assert_eq!(app.raw_scroll_back, app.raw.len() as u16 - 1);
    }

    #[test]
    fn log_view_toggles_independently_and_scrolls() {
        use crate::session::LogEntry;
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
        assert_eq!(app.transcript_scroll, 0, "starts at the beginning, not the tail");

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
        app.push_event(AgentEvent::ThreadStarted { thread_id: "t".into() });
        // Multi-line so each entry has detail beyond its summary, and so
        // qualifies for the has-detail navigation this test also exercises.
        app.push_event(AgentEvent::AgentMessage { text: "a\nmore a".into() });
        app.push_event(AgentEvent::TurnStarted);
        app.push_event(AgentEvent::AgentMessage { text: "b\nmore b".into() });
        app.push_event(AgentEvent::TurnCompleted { usage: TokenUsage::default() });

        // Hidden by default; no toggle needed to get here.
        assert!(!app.show_minor);

        app.select_first();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "a\nmore a".into() });

        app.select_next();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "b\nmore b".into() });

        // No more visible entries after "b"; select_next is a no-op.
        app.select_next();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "b\nmore b".into() });

        app.select_prev();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "a\nmore a".into() });

        app.toggle_minor();
        assert!(app.show_minor);
    }

    #[test]
    fn select_next_and_prev_skip_entries_with_no_detail_beyond_their_summary() {
        let mut app = app();
        // A single line of text is entirely a restatement of the summary, so
        // this entry has no arrow and should not be a stop for j/k.
        app.push_event(AgentEvent::AgentMessage { text: "no detail here".into() });
        app.push_event(AgentEvent::AgentMessage { text: "has detail\nsecond line".into() });
        app.push_event(AgentEvent::AgentMessage { text: "also no detail".into() });
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
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "a".into() });
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
        use crate::session::DrivaOptions;
        use driva::{Mount, MountAccess};

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
        assert_eq!(app.driva_options.as_ref().unwrap().isolation_backend, "bwrap");

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
        app.toggle_focus();
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
}
