//! Short-lived notices about things Styra did on the operator's behalf.
//!
//! Held apart from [`App`](crate::app::App) because the queue has a rule of
//! its own that nothing else shares: each notice is displayed for its own five
//! seconds from the moment it was shown, so they expire in the order they
//! arrived and a burst of them does not collapse into one. Testing that meant
//! reaching into a public `VecDeque` and rewriting a private timestamp.

use std::collections::VecDeque;
use std::time::{Duration, Instant};
use styra_protocol::LogLevel;

/// How long each notice is displayed for.
const LIFETIME: Duration = Duration::from_secs(5);

/// A short-lived notice about something Styra did on the operator's behalf.
pub struct Notice {
    pub text: String,
    pub level: LogLevel,
    shown_at: Instant,
}

/// The notices currently on screen, oldest first.
#[derive(Default)]
pub struct Notices {
    shown: VecDeque<Notice>,
}

impl Notices {
    /// Show a notice for the next five seconds.
    pub fn show(&mut self, text: impl Into<String>) {
        self.show_level(text, LogLevel::Info);
    }

    pub fn show_level(&mut self, text: impl Into<String>, level: LogLevel) {
        self.show_at(text, level, Instant::now());
    }

    /// Show a notice as if at `at`. Only the tests need to say when; the
    /// alternative is reaching in afterwards to rewrite the timestamp.
    #[cfg(test)]
    pub fn show_since(&mut self, text: impl Into<String>, ago: Duration) {
        self.show_at(text, LogLevel::Info, Instant::now() - ago);
    }

    fn show_at(&mut self, text: impl Into<String>, level: LogLevel, at: Instant) {
        self.shown.push_back(Notice {
            text: text.into(),
            level,
            shown_at: at,
        });
    }

    /// Remove notices whose own five-second window has elapsed. Called once
    /// per event-loop iteration, just before rendering.
    ///
    /// Reports whether it removed any, because a notice leaving the screen is
    /// a change to what is on it and nothing else in the round would say so.
    pub fn expire(&mut self) -> bool {
        let now = Instant::now();
        let before = self.shown.len();
        while self
            .shown
            .front()
            .is_some_and(|notice| now.duration_since(notice.shown_at) >= LIFETIME)
        {
            self.shown.pop_front();
        }
        self.shown.len() != before
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Notice> {
        self.shown.iter()
    }
}

#[cfg(test)]
mod tests;
