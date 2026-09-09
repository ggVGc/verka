//! Waiting out a rate limit: when the plan's window runs dry the agent process
//! stops mid-conversation, and the turn it was working on is lost until someone
//! is at the keyboard to send it again. This is the operator saying "come back
//! to it yourself" — once, in advance, rather than by watching the clock.
//!
//! Only two things are held here: whether the operator asked for it, and the
//! turn that is waiting. Everything the decision is *made* from is already
//! recorded elsewhere and is read from there rather than copied to here — the
//! window that rejected the session and the minute it resets come from the
//! quota log ([`crate::app::App::quota`]), and the turn to send again from the
//! event list. That matters for more than tidiness: the server announces each
//! usage threshold once across every interaction it serves, so the reading that
//! explains why *this* session stopped may well have been announced to another
//! one. Reading the log instead of remembering an announcement means a session
//! can still be waited out on the strength of a reading it never saw itself.
//!
//! Time is passed in rather than read here, so the state machine can be tested
//! against a clock the test holds still.

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use styra_server::{QuotaEvent, QuotaStatus};

/// How long past the reset the waiting turn is sent.
///
/// The providers report the minute a window turns over, and a request landing
/// exactly on it is the one most likely to be rejected for being early — the
/// figure is theirs to round, and an unattended retry that is refused costs the
/// operator the whole wait again. A few minutes past it costs nothing anybody
/// is watching.
pub const GRACE: Duration = Duration::from_secs(3 * 60);

/// The turn waiting to be sent again, and when it is due.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// When to send it: the window's reset plus [`GRACE`].
    pub at_ms: u64,
    /// The provider window being waited out, so every message about the wait
    /// can say which limit it is about.
    pub window: String,
    /// The operator's last turn, to be sent again once the window has reset.
    pub message: String,
}

/// Whether stopped sessions are waited out, and the turn that is waiting.
#[derive(Default)]
pub struct Retry {
    enabled: bool,
    pending: Option<Pending>,
}

impl Retry {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Turn waiting out a rate limit on or off, and say which it now is.
    ///
    /// Turning it off drops whatever was waiting: the operator who does that
    /// while a turn is due in an hour means "do not send it", not "send it but
    /// hold this setting for next time".
    pub fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        if !self.enabled {
            self.pending = None;
        }
        self.enabled
    }

    /// The turn that is waiting, for the views that say so.
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    pub fn arm(&mut self, pending: Pending) {
        self.pending = Some(pending);
    }

    /// Drop the waiting turn, saying whether there was one. The setting itself
    /// stays on: what has been overtaken is this wait, not the operator's
    /// standing answer to what should happen at the next limit.
    pub fn cancel(&mut self) -> bool {
        self.pending.take().is_some()
    }

    /// Take the waiting turn once it is due, leaving it in place until then.
    pub fn take_due(&mut self, now_ms: u64) -> Option<Pending> {
        if self.pending.as_ref()?.at_ms > now_ms {
            return None;
        }
        self.pending.take()
    }
}

/// The wall clock in milliseconds, as the quota readings are stamped.
///
/// A machine whose clock is before the epoch reads as the epoch, which puts
/// every reset in the future and so waits rather than sending a turn early.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

/// The exhausted window that is still in force at `now_ms`, newest reading
/// first, or `None` when nothing in the log explains a stopped session.
///
/// Only `provider`'s readings count. The two agents are separate
/// subscriptions, and the log holds both accounts' readings, so a Codex window
/// running dry says nothing about why a Claude session stopped.
///
/// A reading with no reset time is no use here even though it is exhausted:
/// the whole of this is "come back when the window has turned over", and
/// without the minute it turns over there is nothing to come back at. The
/// providers do report one with a rejection; a reading that does not is left
/// for the operator rather than turned into a guessed wait.
pub fn limit_in_force<'log>(
    readings: impl DoubleEndedIterator<Item = &'log QuotaEvent>,
    provider: styra_server::agent::Provider,
    now_ms: u64,
) -> Option<&'log QuotaEvent> {
    readings.rev().find(|reading| {
        reading.provider == provider
            && reading.status == QuotaStatus::Exhausted
            && reading.resets_at_ms.is_some_and(|resets| resets > now_ms)
    })
}

/// When a turn held back by `limit` should be sent, or `None` for a reading
/// that names no reset.
pub fn due_at_ms(limit: &QuotaEvent) -> Option<u64> {
    Some(limit.resets_at_ms? + GRACE.as_millis() as u64)
}

/// How long until `at_ms`, as a message about a wait of hours says it. Rounded
/// up to the minute, so a wait that is still on says at least "1m" rather than
/// counting down to a "0m" that reads as "now".
pub fn wait_label(at_ms: u64, now_ms: u64) -> String {
    let minutes = at_ms.saturating_sub(now_ms).div_ceil(60_000);
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_server::agent::Provider;

    fn reading(
        provider: Provider,
        status: QuotaStatus,
        at_ms: u64,
        resets_at_ms: Option<u64>,
    ) -> QuotaEvent {
        QuotaEvent {
            at_ms,
            session_id: "s-1".into(),
            provider,
            window: "five_hour".into(),
            status,
            utilization: None,
            resets_at_ms,
            detail: None,
        }
    }

    #[test]
    fn toggling_returns_the_state_it_left_behind() {
        let mut retry = Retry::default();
        assert!(!retry.enabled());
        assert!(retry.toggle());
        assert!(!retry.toggle());
    }

    /// Turning it off means the turn that was waiting is not sent, rather than
    /// being sent by whatever turns it on again next.
    #[test]
    fn turning_it_off_drops_the_waiting_turn() {
        let mut retry = Retry::default();
        retry.toggle();
        retry.arm(Pending {
            at_ms: 5_000,
            window: "five_hour".into(),
            message: "carry on".into(),
        });
        assert!(retry.pending().is_some());

        retry.toggle();

        assert!(retry.pending().is_none());
        retry.toggle();
        assert!(retry.pending().is_none());
    }

    #[test]
    fn a_waiting_turn_is_held_until_it_is_due() {
        let mut retry = Retry::default();
        retry.toggle();
        retry.arm(Pending {
            at_ms: 5_000,
            window: "five_hour".into(),
            message: "carry on".into(),
        });

        assert!(retry.take_due(4_999).is_none());
        assert!(retry.pending().is_some(), "still waiting, not consumed");
        let due = retry.take_due(5_000).expect("due on the minute");
        assert_eq!(due.message, "carry on");
        assert!(
            retry.take_due(9_999).is_none(),
            "one wait sends one turn, not one per frame"
        );
    }

    /// The reading that explains a stopped session is the newest rejection of
    /// that provider's plan whose window has not turned over yet.
    #[test]
    fn the_limit_in_force_is_the_newest_unreset_rejection() {
        let readings = [
            reading(Provider::Claude, QuotaStatus::Exhausted, 1_000, Some(2_000)),
            reading(Provider::Claude, QuotaStatus::Exhausted, 3_000, Some(9_000)),
            reading(Provider::Claude, QuotaStatus::Warning, 4_000, Some(9_000)),
        ];

        let limit = limit_in_force(readings.iter(), Provider::Claude, 5_000)
            .expect("the rejection at 3_000 is still in force");

        assert_eq!(limit.at_ms, 3_000);
        assert_eq!(due_at_ms(limit), Some(9_000 + GRACE.as_millis() as u64));
    }

    /// A window that has since turned over is not what a stopped session is
    /// waiting for; there is nothing to wait out.
    #[test]
    fn a_window_that_has_already_reset_is_not_in_force() {
        let readings = [reading(
            Provider::Claude,
            QuotaStatus::Exhausted,
            1_000,
            Some(2_000),
        )];

        assert!(limit_in_force(readings.iter(), Provider::Claude, 2_001).is_none());
    }

    /// The log holds both accounts' readings, and one subscription running dry
    /// says nothing about the other.
    #[test]
    fn another_providers_exhausted_window_explains_nothing() {
        let readings = [reading(
            Provider::Codex,
            QuotaStatus::Exhausted,
            1_000,
            Some(9_000),
        )];

        assert!(limit_in_force(readings.iter(), Provider::Claude, 2_000).is_none());
        assert!(limit_in_force(readings.iter(), Provider::Codex, 2_000).is_some());
    }

    /// Without a reset time there is no minute to come back at, so the
    /// rejection is left for the operator rather than turned into a guess.
    #[test]
    fn a_rejection_naming_no_reset_is_not_waited_out() {
        let readings = [reading(
            Provider::Claude,
            QuotaStatus::Exhausted,
            1_000,
            None,
        )];

        assert!(limit_in_force(readings.iter(), Provider::Claude, 2_000).is_none());
    }

    #[test]
    fn a_wait_reads_as_minutes_and_then_as_hours() {
        assert_eq!(wait_label(60_000, 0), "1m");
        // Part of a minute still to go is a minute, never "0m".
        assert_eq!(wait_label(1, 0), "1m");
        assert_eq!(wait_label(0, 0), "0m");
        assert_eq!(wait_label(59 * 60_000, 0), "59m");
        assert_eq!(wait_label(60 * 60_000, 0), "1h00m");
        assert_eq!(wait_label(4 * 60 * 60_000 + 7 * 60_000, 0), "4h07m");
        // A wait that is already past reads as none left rather than wrapping.
        assert_eq!(wait_label(0, 10_000), "0m");
    }
}
