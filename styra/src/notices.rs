//! Short-lived notices about things Styra did on the operator's behalf.
//!
//! Held apart from [`App`](crate::app::App) because the queue has a rule of
//! its own that nothing else shares: each notice is displayed for its own five
//! seconds from the moment it was shown, so they expire in the order they
//! arrived and a burst of them does not collapse into one. Testing that meant
//! reaching into a public `VecDeque` and rewriting a private timestamp.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long each notice is displayed for.
const LIFETIME: Duration = Duration::from_secs(5);

/// A short-lived notice about something Styra did on the operator's behalf.
pub struct Notice {
    pub text: String,
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
        self.show_at(text, Instant::now());
    }

    /// Show a notice as if at `at`. Only the tests need to say when; the
    /// alternative is reaching in afterwards to rewrite the timestamp.
    #[cfg(test)]
    pub fn show_since(&mut self, text: impl Into<String>, ago: Duration) {
        self.show_at(text, Instant::now() - ago);
    }

    fn show_at(&mut self, text: impl Into<String>, at: Instant) {
        self.shown.push_back(Notice {
            text: text.into(),
            shown_at: at,
        });
    }

    /// Remove notices whose own five-second window has elapsed. Called once
    /// per event-loop iteration, just before rendering.
    pub fn expire(&mut self) {
        let now = Instant::now();
        while self
            .shown
            .front()
            .is_some_and(|notice| now.duration_since(notice.shown_at) >= LIFETIME)
        {
            self.shown.pop_front();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// How many are on screen, for the panel to size itself by.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Notice> {
        self.shown.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(notices: &Notices) -> Vec<&str> {
        notices.iter().map(|notice| notice.text.as_str()).collect()
    }

    /// Styra can do several things in one keystroke, and each is worth saying.
    #[test]
    fn notices_accumulate_in_the_order_they_were_shown() {
        let mut notices = Notices::default();
        assert!(notices.is_empty());

        notices.show("first action");
        notices.show("second action");

        assert_eq!(texts(&notices), vec!["first action", "second action"]);
    }

    #[test]
    fn a_notice_is_gone_once_its_five_seconds_are_up() {
        let mut notices = Notices::default();
        notices.show_since("old action", Duration::from_secs(5));

        notices.expire();

        assert!(notices.is_empty());
    }

    /// Each notice gets its own window from when it was shown, so a later one
    /// is not cut short by an earlier one running out.
    #[test]
    fn an_expiring_notice_does_not_take_a_newer_one_with_it() {
        let mut notices = Notices::default();
        notices.show_since("old action", Duration::from_secs(5));
        notices.show("new action");

        notices.expire();

        assert_eq!(texts(&notices), vec!["new action"]);
    }

    /// Expiry walks from the front and stops at the first notice still within
    /// its window, which is correct only because they are in order.
    #[test]
    fn a_notice_still_within_its_window_holds_the_queue() {
        let mut notices = Notices::default();
        notices.show_since("recent", Duration::from_secs(1));
        notices.show_since("older but behind it", Duration::from_secs(9));

        notices.expire();

        assert_eq!(
            texts(&notices),
            vec!["recent", "older but behind it"],
            "nothing behind a live notice is examined"
        );
    }
}
