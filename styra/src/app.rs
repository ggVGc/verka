//! Application state: the event list, selection, expansion, focus, the message
//! buffer, and session status.
//!
//! This module is pure state and transitions — no terminal, no threads, no IO —
//! so the whole interaction model is unit-testable. [`crate::ui`] renders it and
//! `main` feeds it input and session updates.

use crate::event::{StyraEvent, TokenUsage};
use crate::session::SessionEnd;

/// Which region receives keys, like vim's normal/insert split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Navigate and fold the event list.
    List,
    /// Type into the message box.
    Input,
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
    pub event: StyraEvent,
    pub expanded: bool,
}

/// The complete UI state.
pub struct App {
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub focus: Focus,
    pub input: String,
    pub status: Status,
    /// When true, the selection tracks the newest entry as events arrive.
    pub follow: bool,
    pub profile_name: String,
    pub session_id: String,
    pub latest_usage: Option<TokenUsage>,
    /// Set when the operator asks to quit; the event loop observes it.
    pub should_quit: bool,
}

impl App {
    pub fn new(profile_name: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            focus: Focus::List,
            input: String::new(),
            status: Status::Running,
            follow: true,
            profile_name: profile_name.into(),
            session_id: session_id.into(),
            latest_usage: None,
            should_quit: false,
        }
    }

    /// True when the operator can still send messages.
    pub fn can_send(&self) -> bool {
        self.status.is_active()
    }

    // --- Ingesting session updates -----------------------------------------

    /// Append a decoded event, advancing status and, while following, selection.
    pub fn push_event(&mut self, event: StyraEvent) {
        match &event {
            StyraEvent::TurnCompleted { usage } => {
                self.latest_usage = Some(usage.clone());
                if self.status.is_active() {
                    self.status = Status::Waiting;
                }
            }
            StyraEvent::UserMessage { .. }
            | StyraEvent::TurnStarted
            | StyraEvent::CommandStarted { .. }
            | StyraEvent::ToolStarted { .. }
            | StyraEvent::AgentMessage { .. }
            | StyraEvent::PlanUpdated { .. } => {
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

    pub fn select_next(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
        // Re-enable follow only when the selection reaches the tail.
        self.follow = !self.entries.is_empty() && self.selected + 1 == self.entries.len();
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        // Moving off the tail pins the view.
        self.follow = false;
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.follow = self.entries.len() <= 1;
    }

    pub fn select_last(&mut self) {
        if !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
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
        app.push_event(StyraEvent::TurnStarted);
        app.push_event(StyraEvent::AgentMessage { text: "hi".into() });
        assert!(app.follow);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn moving_up_pins_the_view_and_reaching_the_tail_resumes_follow() {
        let mut app = app();
        for _ in 0..3 {
            app.push_event(StyraEvent::TurnStarted);
        }
        app.select_prev();
        assert!(!app.follow);
        assert_eq!(app.selected, 1);

        // New events no longer move the selection while pinned.
        app.push_event(StyraEvent::AgentMessage { text: "x".into() });
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
        app.push_event(StyraEvent::AgentMessage { text: "a".into() });
        app.push_event(StyraEvent::AgentMessage { text: "b".into() });

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
        app.push_event(StyraEvent::TurnCompleted {
            usage: TokenUsage { input_tokens: 7, ..Default::default() },
        });
        assert_eq!(app.status, Status::Waiting);
        assert_eq!(app.latest_usage.as_ref().unwrap().input_tokens, 7);

        app.push_event(StyraEvent::UserMessage { text: "more".into() });
        assert_eq!(app.status, Status::Running);
    }

    #[test]
    fn ending_is_terminal_and_disables_sending() {
        let mut app = app();
        app.on_ended(SessionEnd { exit_code: Some(0), error: None });
        assert_eq!(app.status, Status::Ended { exit_code: Some(0), error: None });
        assert!(!app.can_send());
        // A late event does not revive an ended session.
        app.push_event(StyraEvent::AgentMessage { text: "late".into() });
        assert!(matches!(app.status, Status::Ended { .. }));
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
