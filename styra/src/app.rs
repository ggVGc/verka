//! Application state: the event list, selection, expansion, focus, the message
//! buffer, and session status.
//!
//! This module is pure state and transitions — no terminal, no threads, no IO —
//! so the whole interaction model is unit-testable. [`crate::ui`] renders it and
//! `main` feeds it input and session updates.

use crate::event::{AgentEvent, TokenUsage};
use crate::session::{LogEntry, RawLine, SessionEnd};
use std::path::PathBuf;

/// Which region receives keys, like vim's normal/insert split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Navigate and fold the event list.
    List,
    /// Type into the message box.
    Input,
}

/// What the main region shows: the decoded event list, the raw wire stream, or
/// the diagnostic log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Events,
    Raw,
    Log,
}

/// The session's lifecycle as the operator sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The agent is working.
    Running,
    /// A turn completed; the agent is idle, awaiting input.
    Waiting,
    /// The operator stopped the session; the process may still be winding down.
    Stopped,
    /// The agent process ended.
    Ended { exit_code: Option<i32>, error: Option<String> },
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Running => "running".into(),
            Status::Waiting => "waiting".into(),
            Status::Stopped => "stopped".into(),
            Status::Ended { error: Some(_), .. } => "failed".into(),
            Status::Ended { exit_code: Some(code), .. } => format!("ended ({code})"),
            Status::Ended { .. } => "ended".into(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Status::Running | Status::Waiting)
    }
}

/// One event in the list, with its fold state.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub event: AgentEvent,
    pub expanded: bool,
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
    pub latest_usage: Option<TokenUsage>,
    /// The verbatim wire interaction, in occurrence order.
    pub raw: Vec<RawLine>,
    /// Lines scrolled back from the bottom of the raw view; 0 tracks the tail.
    pub raw_scroll_back: u16,
    /// Diagnostic log entries, in occurrence order.
    pub log: Vec<LogEntry>,
    /// Lines scrolled back from the bottom of the log view; 0 tracks the tail.
    pub log_scroll_back: u16,
    /// Set when the operator asks to quit; the event loop observes it.
    pub should_quit: bool,
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
            show_minor: true,
            show_preview: false,
            profile_name: profile_name.into(),
            session_id: session_id.into(),
            workspace_root: None,
            latest_usage: None,
            raw: Vec::new(),
            raw_scroll_back: 0,
            log: Vec::new(),
            log_scroll_back: 0,
            should_quit: false,
        }
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

    /// True when the operator can still send messages.
    pub fn can_send(&self) -> bool {
        self.status.is_active()
    }

    // --- Ingesting session updates -----------------------------------------

    /// Append a decoded event, advancing status and, while following, selection.
    pub fn push_event(&mut self, event: AgentEvent) {
        match &event {
            AgentEvent::TurnCompleted { usage } => {
                self.latest_usage = Some(usage.clone());
                if self.status.is_active() {
                    self.status = Status::Waiting;
                }
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

    /// The nearest visible index at or after `from`, if any.
    fn next_visible(&self, from: usize) -> Option<usize> {
        (from..self.entries.len()).find(|&i| self.is_visible(i))
    }

    /// The nearest visible index at or before `from`, if any.
    fn prev_visible(&self, from: usize) -> Option<usize> {
        (0..=from).rev().find(|&i| self.is_visible(i))
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

    /// Toggle whether minor lifecycle events (thread/turn/usage) are shown.
    pub fn toggle_minor(&mut self) {
        self.show_minor = !self.show_minor;
        if !self.entries.is_empty() && !self.is_visible(self.selected) {
            if let Some(idx) = self.prev_visible(self.selected).or_else(|| self.next_visible(self.selected)) {
                self.selected = idx;
            }
        }
    }

    pub fn select_next(&mut self) {
        if let Some(next) = self.next_visible(self.selected + 1) {
            self.selected = next;
        }
        // Re-enable follow only when the selection reaches the visible tail.
        self.follow = !self.entries.is_empty() && self.next_visible(self.selected + 1).is_none();
    }

    pub fn select_prev(&mut self) {
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
        for _ in 0..3 {
            app.push_event(AgentEvent::TurnStarted);
        }
        app.select_prev();
        assert!(!app.follow);
        assert_eq!(app.selected, 1);

        // New events no longer move the selection while pinned.
        app.push_event(AgentEvent::AgentMessage { text: "x".into() });
        assert_eq!(app.selected, 1);

        // Walking back down to the tail re-enables follow.
        app.select_next();
        app.select_next();
        app.select_next();
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
        assert_eq!(app.status, Status::Waiting);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 7);

        app.push_event(AgentEvent::UserMessage { text: "more".into() });
        assert_eq!(app.status, Status::Running);
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
    fn minor_events_are_hidden_and_skipped_by_navigation() {
        let mut app = app();
        app.push_event(AgentEvent::ThreadStarted { thread_id: "t".into() });
        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::TurnStarted);
        app.push_event(AgentEvent::AgentMessage { text: "b".into() });
        app.push_event(AgentEvent::TurnCompleted { usage: TokenUsage::default() });

        app.toggle_minor();
        assert!(!app.show_minor);

        app.select_first();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "a".into() });

        app.select_next();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "b".into() });

        // No more visible entries after "b"; select_next is a no-op.
        app.select_next();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "b".into() });

        app.select_prev();
        assert_eq!(app.entries[app.selected].event, AgentEvent::AgentMessage { text: "a".into() });

        app.toggle_minor();
        assert!(app.show_minor);
    }

    #[test]
    fn toggling_minor_off_moves_selection_off_a_hidden_entry() {
        let mut app = app();
        app.push_event(AgentEvent::AgentMessage { text: "a".into() });
        app.push_event(AgentEvent::TurnStarted);
        // Selection sits on the just-pushed minor entry via follow.
        assert_eq!(app.selected, 1);

        app.toggle_minor();
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
    fn preview_toggles_independently_of_other_view_state() {
        let mut app = app();
        assert!(!app.show_preview);
        app.toggle_preview();
        assert!(app.show_preview);
        app.toggle_preview();
        assert!(!app.show_preview);
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
}
