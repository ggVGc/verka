//! How the Interaction on screen is going: its lifecycle status, what it is
//! doing in the background, and how long it has been doing it.
//!
//! Held apart from [`App`](crate::app::App) because it is a state machine
//! rather than a set of fields. Status is written from several places — an
//! applied update, a queued send, a key handler — and three separate pieces of
//! bookkeeping hang off it: when it last changed, whether the provider has
//! reported background work, and how much has arrived from the agent. Those
//! were private [`App`](crate::app::App) fields reached through `pub(crate)`
//! back doors from [`crate::ingest`]; here they are this type's own.

use std::time::{Duration, Instant};

use styra_server::event::TokenUsage;

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
    /// The agent is idle, but a Claude background task is still running.
    Background,
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
            Status::Background => "idle · background work running".into(),
            Status::Stopped => "stopped".into(),
            Status::Ended { error: Some(_), .. } => "failed".into(),
            Status::Ended {
                exit_code: Some(code),
                ..
            } => format!("ended ({code})"),
            Status::Ended { .. } => "ended".into(),
        }
    }

    /// A single character standing in for the state, for lists that mark every
    /// row with it in a one-column gutter. It carries the same color the label
    /// does, so the glyph and the word never disagree.
    ///
    /// Deliberately ASCII: these are exactly one cell wide in every terminal
    /// and font, so nothing to the right of the gutter can drift out of
    /// alignment the way it can behind a double-width or missing glyph.
    pub fn glyph(&self) -> char {
        match self {
            Status::Pending => '.',
            Status::Running => '>',
            Status::Idle => 'o',
            Status::Background => '*',
            Status::Stopped => '#',
            Status::Ended { error: Some(_), .. } => '!',
            Status::Ended { .. } => 'x',
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Status::Pending | Status::Running | Status::Idle | Status::Background | Status::Stopped
        )
    }
}

impl From<styra_server::InteractionActivity> for Status {
    fn from(activity: styra_server::InteractionActivity) -> Self {
        match activity {
            styra_server::InteractionActivity::Pending => Self::Idle,
            styra_server::InteractionActivity::Running => Self::Running,
            styra_server::InteractionActivity::Background => Self::Background,
        }
    }
}
/// How long the session has been in its current state, and how long since
/// anything last arrived from the agent.
///
/// A turn can spend minutes inside one tool call, during which nothing on the
/// screen changes; without these two numbers a working session and a hung one
/// look identical.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    /// Time since the status last changed — for a running turn, how long it
    /// has been running; for an idle one, how long it has been waiting.
    pub in_status: Duration,
    /// Time since the last event was received, or `None` if none has been.
    pub since_event: Option<Duration>,
    /// How many events have arrived from the agent. The spinner steps with
    /// this rather than with the clock, so its motion means "something came
    /// back" instead of "the frame was redrawn".
    pub events: usize,
}
/// The Interaction's status and the bookkeeping that hangs off it.
pub struct Activity {
    /// Where the Interaction is in its lifecycle. A plain value with no
    /// invariant of its own: what needs guarding is everything below, which
    /// is why they are private and this is not.
    pub status: Status,
    /// The usage figure from the most recent turn, or the running total
    /// mid-turn for providers that report as they go.
    pub latest_usage: Option<TokenUsage>,
    /// When the status last changed, and the status that was current then.
    /// Status is written from several places, so the moment it changed is
    /// noticed in one place — [`Activity::note_progress`] — rather than at
    /// each assignment.
    since: Instant,
    noted: Status,
    /// When the last event arrived from the agent. `None` until one has.
    last_event_at: Option<Instant>,
    /// How many events have arrived from the agent, for the spinner's phase.
    events: usize,
    background_work: bool,
    /// Set once the provider has reported its background-task set. From then
    /// on that count is the only thing that moves `background_work`; the
    /// tool-call heuristics are a fallback for providers that stay silent.
    background_count_known: bool,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            status: Status::Running,
            latest_usage: None,
            since: Instant::now(),
            noted: Status::Running,
            last_event_at: None,
            events: 0,
            background_work: false,
            background_count_known: false,
        }
    }
}

impl Activity {
    /// Notice a status change made since the last frame, so [`Self::progress`]
    /// can report how long the session has been in its current state. Called
    /// once per event-loop iteration, just before rendering.
    pub fn note_progress(&mut self) {
        if self.noted != self.status {
            self.noted = self.status.clone();
            self.since = Instant::now();
        }
    }

    pub fn progress(&self) -> Progress {
        Progress {
            in_status: self.since.elapsed(),
            since_event: self.last_event_at.map(|at| at.elapsed()),
            events: self.events,
        }
    }

    /// Note that something arrived from the agent, whatever it was: the spinner
    /// steps with the count and the "nothing for a while" figure with the time.
    pub fn note_event_received(&mut self) {
        self.last_event_at = Some(Instant::now());
        self.events += 1;
    }

    /// Where a completed turn leaves the session: still working on something in
    /// the background, or genuinely waiting for the operator.
    pub fn idle_or_background(&self) -> Status {
        if self.background_work {
            Status::Background
        } else {
            Status::Idle
        }
    }

    /// Reconcile a reconstructed interaction view with the server's current
    /// activity summary. A bounded preview deliberately omits lifecycle
    /// events, so replaying its conversation tail alone cannot distinguish an
    /// idle interaction from one that is still working.
    ///
    /// Background state is supplied independently because foreground and
    /// background work can coexist while the activity is `Running`.
    pub fn sync_to_interaction(
        &mut self,
        activity: styra_server::InteractionActivity,
        background_work: bool,
    ) {
        self.background_work = background_work;
        self.status = match activity {
            styra_server::InteractionActivity::Pending => Status::Idle,
            styra_server::InteractionActivity::Running => Status::Running,
            styra_server::InteractionActivity::Background => Status::Background,
        };
    }

    /// The provider's own count of what it is running in the background. Once
    /// it has reported one, that count is the only thing that moves the flag.
    pub fn note_background_count(&mut self, running: usize) {
        self.background_count_known = true;
        self.background_work = running > 0;
        if !self.background_work && self.status == Status::Background {
            self.status = Status::Idle;
        }
    }

    /// A tool call that looks like it started background work, for providers
    /// that never report a count.
    pub fn note_background_started(&mut self) {
        self.background_work = true;
    }

    /// The same heuristic in reverse, and ignored once the provider has spoken
    /// for itself.
    pub fn note_background_finished(&mut self) {
        if !self.background_count_known {
            self.background_work = false;
            self.status = Status::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that reports its background set owns the answer from then
    /// on, so a heuristic guess afterwards must not contradict it.
    #[test]
    fn a_reported_count_takes_over_from_the_tool_call_heuristic() {
        let mut activity = Activity::default();
        activity.note_background_started();
        assert_eq!(activity.idle_or_background(), Status::Background);

        activity.note_background_count(2);
        activity.note_background_finished();

        assert_eq!(
            activity.idle_or_background(),
            Status::Background,
            "one finished tool call does not clear a reported set of two"
        );
    }

    #[test]
    fn a_reported_empty_set_clears_a_background_status_on_the_spot() {
        let mut activity = Activity::default();
        activity.note_background_started();
        activity.status = Status::Background;

        activity.note_background_count(0);

        assert_eq!(activity.status, Status::Idle);
        assert_eq!(activity.idle_or_background(), Status::Idle);
    }

    /// Only a Background status is displaced by the count reaching zero; an
    /// Interaction that is mid-turn stays running.
    #[test]
    fn a_cleared_count_leaves_a_running_turn_alone() {
        let mut activity = Activity::default();
        activity.status = Status::Running;

        activity.note_background_count(0);

        assert_eq!(activity.status, Status::Running);
    }

    #[test]
    fn the_heuristic_still_applies_while_the_provider_stays_silent() {
        let mut activity = Activity::default();
        activity.note_background_started();
        activity.status = Status::Background;

        activity.note_background_finished();

        assert_eq!(activity.status, Status::Idle);
    }

    /// A bounded preview carries conversation rows but not the lifecycle
    /// events the status is otherwise reconstructed from, so the server's own
    /// summary is what settles it.
    #[test]
    fn a_server_summary_settles_a_status_a_preview_could_not_reconstruct() {
        let mut activity = Activity::default();

        activity.sync_to_interaction(styra_server::InteractionActivity::Running, true);

        assert_eq!(activity.status, Status::Running);
        assert_eq!(
            activity.idle_or_background(),
            Status::Background,
            "foreground and background work coexist"
        );

        activity.sync_to_interaction(styra_server::InteractionActivity::Pending, false);

        assert_eq!(activity.status, Status::Idle);
        assert_eq!(activity.idle_or_background(), Status::Idle);
    }

    #[test]
    fn progress_counts_what_has_arrived_and_when_the_status_last_changed() {
        let mut activity = Activity::default();
        assert_eq!(activity.progress().events, 0);
        assert!(activity.progress().since_event.is_none());

        activity.note_event_received();
        activity.note_event_received();

        assert_eq!(activity.progress().events, 2);
        assert!(activity.progress().since_event.is_some());
    }

    /// The clock restarts only when the status actually changes, so a frame
    /// that changed nothing does not read as a fresh transition.
    #[test]
    fn the_status_clock_restarts_only_on_a_real_change() {
        let mut activity = Activity::default();
        activity.status = Status::Idle;
        activity.note_progress();
        let first = activity.progress().in_status;

        activity.note_progress();

        assert!(
            activity.progress().in_status >= first,
            "same status, same clock"
        );
    }
}
