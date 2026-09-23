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

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use styra_protocol::event::{AgentEvent, TokenUsage, UsageTracker};
use styra_protocol::{InteractionActivity, InteractionActivityReason, InteractionSummary};

/// The wall clock in the epoch milliseconds the server dates an interaction's
/// activity in. Only the difference between the two readings is used, so the
/// two clocks need to agree about how long a minute is, not about the hour.
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// The plan window that refused an interaction's work, as much of it as an
/// operator needs to read off a status line: which window, and when it is
/// expected back.
///
/// A trimmed [`styra_protocol::QuotaEvent`] rather than the reading itself.
/// The reading is a measurement taken at a moment — utilization, the session
/// that saw it, when it was seen — and none of that is what a status means by
/// "rate limited": that is the window and the wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// The window as the provider names it (`five_hour`) or as its length
    /// (`1h`, `7d`).
    pub window: String,
    /// When the window is expected to allow work again, in epoch
    /// milliseconds. `None` when the provider refused without saying — a wait
    /// with no end to name.
    pub resets_at_ms: Option<u64>,
}

impl From<&styra_protocol::QuotaEvent> for RateLimit {
    fn from(reading: &styra_protocol::QuotaEvent) -> Self {
        Self {
            window: reading.window.clone(),
            resets_at_ms: reading.resets_at_ms,
        }
    }
}

/// Why a live agent is sitting idle rather than working.
///
/// Every one of these leaves the same screen — an agent that takes messages
/// and is not using one — but they are not the same situation to be in, and
/// the difference is exactly what the operator would otherwise have to
/// reconstruct from the log: a turn that finished said what it found, a turn
/// they interrupted stopped halfway through it, and a turn a plan window
/// refused never ran at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdleReason {
    /// The agent finished its turn and is waiting for the next message. This
    /// is also how an agent that ended its turn with a question for the
    /// operator arrives here: asking and finishing are one event on the wire,
    /// so the distinction is in what the last message says, not in the state.
    TurnComplete,
    /// The operator interrupted the running turn (`s`). The agent stopped
    /// where it had got to and still takes messages.
    Interrupted,
    /// The turn ended in an error the agent itself survived — distinct from
    /// [`Status::Ended`], where the process is gone.
    Failed { message: String },
    /// A plan window refused the work. The agent is alive, but nothing this
    /// interaction sends will run until the window turns over.
    RateLimited(RateLimit),
    /// The background work this interaction was waiting on finished, leaving
    /// nothing running behind it. Reached from [`Status::Background`] rather
    /// than from a turn.
    BackgroundFinished,
    /// The server said nothing about why — an older server, or a reason that
    /// describes a stop rather than an idle turn.
    Reported,
}

impl IdleReason {
    /// The server's reason, read as this client's.
    ///
    /// [`InteractionActivityReason::Paused`] is deliberately not one of these.
    /// It describes an interaction that stopped, and an interaction still
    /// taking messages has not: reading it as an idle reason would put "you
    /// paused it" on a session the operator can type into.
    pub fn reported(reason: Option<&InteractionActivityReason>) -> Self {
        match reason {
            Some(InteractionActivityReason::TurnCompleted) => IdleReason::TurnComplete,
            Some(InteractionActivityReason::Interrupted) => IdleReason::Interrupted,
            Some(InteractionActivityReason::Failed { message }) => IdleReason::Failed {
                message: message.clone(),
            },
            Some(InteractionActivityReason::RateLimited {
                window,
                resets_at_ms,
            }) => IdleReason::RateLimited(RateLimit {
                window: window.clone(),
                resets_at_ms: *resets_at_ms,
            }),
            Some(InteractionActivityReason::BackgroundFinished) => IdleReason::BackgroundFinished,
            // Each of these describes an interaction that stopped rather than
            // one waiting for input, so as idle reasons they say nothing.
            Some(InteractionActivityReason::Paused)
            | Some(InteractionActivityReason::Exited { .. })
            | Some(InteractionActivityReason::ServerRestarted)
            | None => IdleReason::Reported,
        }
    }

    /// The reason as a status-line fragment, or `None` where naming it would
    /// add nothing: the ordinary end of a turn is what "idle" already means,
    /// and a reason the server never sent cannot be stated.
    pub fn label(&self) -> Option<String> {
        match self {
            IdleReason::TurnComplete | IdleReason::Reported => None,
            IdleReason::Interrupted => Some("interrupted".into()),
            IdleReason::Failed { .. } => Some("after an error".into()),
            IdleReason::RateLimited(limit) => Some(format!("rate limited ({})", limit.window)),
            IdleReason::BackgroundFinished => Some("background work finished".into()),
        }
    }
}

/// Why an interaction is stopped: no agent behind it that takes messages,
/// though the Session it served can be resumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The operator paused the interaction (`S`): the agent was told to stop
    /// and the queued messages were cleared.
    Paused,
    /// The operator finished with the interaction (`C`): stopped, and stopped
    /// because the work it was doing is done.
    Completed,
    /// A plan window refused the work and the Session is not waiting it out,
    /// so the interaction was stopped rather than left holding a process that
    /// cannot run anything.
    RateLimited(RateLimit),
    /// The interaction's last turn failed, and it stopped there.
    Failed { message: String },
    /// The agent's process ended of its own accord.
    Exited { exit_code: Option<i32> },
    /// The interaction outlived the server run that owned it: it is listed
    /// again because it was never closed, but its agent went down with that
    /// run and resuming the Session is what brings one back.
    ServerRestarted,
    /// The server still lists the interaction, but its agent no longer accepts
    /// messages and said nothing about why — a stale record a client must
    /// treat as stopped rather than queue against.
    NotAccepting,
}

impl StopReason {
    /// The server's reason for an interaction that takes no more messages.
    ///
    /// Completion is checked first and independently of `reason`: it is a
    /// property of the Session, not one of the ways a turn can end, so it can
    /// be true alongside any reason the interaction happened to stop for.
    ///
    /// The reasons that describe a turn ending — interrupted, background work
    /// running out — say nothing about why the interaction then stopped, so
    /// they come through as the bare fact that it has.
    pub fn reported(completed: bool, reason: Option<&InteractionActivityReason>) -> Self {
        if completed {
            return StopReason::Completed;
        }
        match reason {
            Some(InteractionActivityReason::Paused) => StopReason::Paused,
            Some(InteractionActivityReason::Exited { exit_code }) => StopReason::Exited {
                exit_code: *exit_code,
            },
            Some(InteractionActivityReason::Failed { message }) => StopReason::Failed {
                message: message.clone(),
            },
            Some(InteractionActivityReason::RateLimited {
                window,
                resets_at_ms,
            }) => StopReason::RateLimited(RateLimit {
                window: window.clone(),
                resets_at_ms: *resets_at_ms,
            }),
            Some(InteractionActivityReason::ServerRestarted) => StopReason::ServerRestarted,
            _ => StopReason::NotAccepting,
        }
    }

    /// The reason as a status-line fragment. Always worth naming — unlike
    /// idling, stopping is never just what a session does next.
    pub fn label(&self) -> String {
        match self {
            StopReason::Paused => "you paused it".into(),
            StopReason::Completed => "you completed it".into(),
            StopReason::RateLimited(limit) => format!("rate limited ({})", limit.window),
            StopReason::Failed { message } => format!("failed: {message}"),
            StopReason::Exited {
                exit_code: Some(code),
            } => format!("the agent exited ({code})"),
            StopReason::Exited { exit_code: None } => "the agent exited".into(),
            StopReason::ServerRestarted => "the server restarted".into(),
            StopReason::NotAccepting => "no longer accepting messages".into(),
        }
    }
}

/// Why the agent process is gone.
///
/// The exit code and error text on [`Status::Ended`] say what the process
/// reported; this says what happened to it, which the two numbers cannot: a
/// process the operator stopped and a process that exited on its own both
/// leave `Some(0)` behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The agent exited on its own, having been asked for nothing more.
    Completed,
    /// It exited non-zero, or the server reported an error with it.
    Failed,
    /// It ended because the operator stopped the interaction.
    Stopped,
    /// It ended and the server did not say why.
    Unknown,
}

impl EndReason {
    /// What an [`InteractionEnd`](styra_protocol::InteractionEnd) alone
    /// implies, for the endings nobody attributed: an error or a non-zero exit
    /// is a failure, a clean exit is a completion.
    ///
    /// A caller that knows better — the operator's own stop, which the ending
    /// only carries out — names its reason rather than going through here.
    pub fn infer(exit_code: Option<i32>, error: Option<&str>) -> Self {
        match (exit_code, error) {
            (_, Some(_)) => EndReason::Failed,
            (Some(0), None) => EndReason::Completed,
            (Some(_), None) => EndReason::Failed,
            (None, None) => EndReason::Unknown,
        }
    }

    pub fn label(&self) -> Option<String> {
        match self {
            EndReason::Completed | EndReason::Unknown => None,
            EndReason::Failed => Some("failed".into()),
            EndReason::Stopped => Some("you stopped it".into()),
        }
    }
}

/// The session's lifecycle as the operator sees it, and how it got there.
///
/// The states an interaction rests in carry the reason it came to rest: those
/// are the ones an operator walks up to and has to make sense of, and "idle"
/// or "stopped" on its own does not distinguish a finished turn from an
/// interrupted one, or a pause from a spent plan window. The states it passes
/// through do not: [`Status::Pending`] has one cause (nothing has been
/// launched yet), [`Status::Running`] has one (a message is being worked on),
/// and [`Status::Background`] states its own reason — the background work is
/// why it is not plain idle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No agent process has been launched yet; it starts on the operator's
    /// first submitted message (see `App::pending`).
    Pending,
    /// The agent is working.
    Running,
    /// The agent takes messages and is not using one; the reason says what
    /// left it there.
    Idle(IdleReason),
    /// The agent is idle, but a Claude background task is still running.
    Background,
    /// No agent behind this interaction that takes messages; the process may
    /// still be winding down, and the Session can be resumed.
    Stopped(StopReason),
    /// The agent process ended.
    Ended {
        exit_code: Option<i32>,
        error: Option<String>,
        reason: EndReason,
    },
}

impl Status {
    /// The terminal status an [`InteractionEnd`](styra_protocol::InteractionEnd)
    /// describes, with its reason read off the ending itself.
    pub fn ended(exit_code: Option<i32>, error: Option<String>) -> Self {
        let reason = EndReason::infer(exit_code, error.as_deref());
        Status::Ended {
            exit_code,
            error,
            reason,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, Status::Idle(_))
    }

    /// Why the session is in this state, as a fragment to hang off the label,
    /// or `None` where the state is its own explanation.
    pub fn reason_label(&self) -> Option<String> {
        match self {
            Status::Pending | Status::Running | Status::Background => None,
            Status::Idle(reason) => reason.label(),
            Status::Stopped(reason) => Some(reason.label()),
            Status::Ended { reason, .. } => reason.label(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Status::Pending => "not started".into(),
            Status::Running => "running".into(),
            Status::Idle(reason) => match reason.label() {
                Some(why) => format!("idle · {why}"),
                None => "idle".into(),
            },
            Status::Background => "idle · background work running".into(),
            Status::Stopped(reason) => format!("stopped · {}", reason.label()),
            Status::Ended { error: Some(_), .. } => "failed".into(),
            // An ending the operator asked for is named as such rather than by
            // its exit code: they know the agent exited, and `ended (0)` only
            // invites them to wonder what happened to it.
            Status::Ended {
                reason: EndReason::Stopped,
                ..
            } => "ended · you stopped it".into(),
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
            Status::Pending
                | Status::Running
                | Status::Idle(_)
                | Status::Background
                | Status::Stopped(_)
        )
    }
}

impl Status {
    /// What the server says about a live interaction: what it is doing, and —
    /// when the server has something to say about it — how it came to be doing
    /// it. This is what a client attaching to work someone else started reads
    /// instead of its own history, which begins at the attachment.
    ///
    /// [`Status::Ended`] is not among the answers: the wire says an
    /// interaction takes no more messages, not what became of the process, and
    /// the exit code that would distinguish the two travels in the update
    /// stream rather than in a summary.
    pub fn reported(interaction: &InteractionSummary) -> Self {
        let reason = interaction.activity_reason.as_ref();
        match interaction.activity {
            InteractionActivity::Pending => Status::Idle(IdleReason::reported(reason)),
            InteractionActivity::Running => Status::Running,
            InteractionActivity::Background => Status::Background,
            InteractionActivity::Stopped => {
                Status::Stopped(StopReason::reported(interaction.completed, reason))
            }
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
    /// What the thread has spent so far — a running total, not the last
    /// turn's own spend, so the status line keeps climbing across turns
    /// instead of resetting to whatever the latest one cost.
    pub latest_usage: Option<TokenUsage>,
    /// Reconstructs whichever half of a turn's usage its provider left out,
    /// so the end-of-turn row can state both what the turn cost and what the
    /// thread has cost. Neither figure is available from a single event.
    usage: UsageTracker,
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
    /// Set when the operator asks the running turn to stop, and cleared by the
    /// turn ending. The wire has no way to say that a turn ended early: the
    /// agent reports the same `TurnCompleted` whether it finished or was cut
    /// off, so the only thing that knows the difference is the client that
    /// asked, and it has to remember until the ending arrives.
    interrupt_requested: bool,
    /// The window that refused this interaction's work, from the moment the
    /// provider said so until the state that refusal explains has been
    /// written. A rejection arrives on its own line, before the turn it killed
    /// reports itself over — so like the interrupt above, it has to be held
    /// until there is a status to hang it on.
    refused_by: Option<RateLimit>,
    /// The error the running turn reported, if it reported one. Agents
    /// announce an error and then end the turn as usual, so this is what tells
    /// the ending apart from an ordinary one.
    turn_error: Option<String>,
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
            usage: UsageTracker::default(),
            since: Instant::now(),
            noted: Status::Running,
            last_event_at: None,
            events: 0,
            background_work: false,
            interrupt_requested: false,
            refused_by: None,
            turn_error: None,
            background_count_known: false,
        }
    }
}

impl Activity {
    /// Let a freshly arrived event state its spend in full: a turn's end says
    /// what the turn cost and what the thread has cost, whichever of the two
    /// its provider actually reported.
    pub fn fill_usage(&mut self, event: &mut AgentEvent) {
        self.usage.observe(event);
    }

    /// Notice a status change made since the last frame, so [`Self::progress`]
    /// can report how long the session has been in its current state. Called
    /// once per event-loop iteration, just before rendering.
    pub fn note_progress(&mut self) {
        if self.noted != self.status {
            self.noted = self.status.clone();
            self.since = Instant::now();
        }
    }

    /// Adopt the server's account of what a live interaction is doing and
    /// since when, as a client attaching to one does.
    ///
    /// Attaching is not an event in the interaction's life. Left to
    /// [`Self::note_progress`], the status would be dated from this client's
    /// first sight of it, and a turn that had been running for ten minutes
    /// would read as having just started — the figure would measure the
    /// watching rather than the work.
    ///
    /// `since_ms` is a moment on the server's wall clock; it becomes an age
    /// against that same clock read now, and the local monotonic clock is
    /// wound back by it.
    pub fn adopt_server_status(&mut self, status: Status, since_ms: u64) {
        self.adopt_server_status_at(status, since_ms, unix_now_ms());
    }

    fn adopt_server_status_at(&mut self, status: Status, since_ms: u64, now_ms: u64) {
        self.background_work = status == Status::Background;
        self.noted = status.clone();
        self.status = status;
        // A server too old to report the moment sends `0`, and clocks that
        // disagree can put it in the future. Both come out as no age at all,
        // which dates the status now — what this client would have assumed
        // anyway.
        let age = match since_ms {
            0 => Duration::ZERO,
            reported => Duration::from_millis(now_ms.saturating_sub(reported)),
        };
        self.since = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
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
    ///
    /// `reason` is what stopped the turn, and is carried only into the idle
    /// case: [`Status::Background`] is already the statement of why this
    /// session is not working, and how its last turn ended does not change it.
    pub fn idle_or_background(&self, reason: IdleReason) -> Status {
        if self.background_work {
            Status::Background
        } else {
            Status::Idle(reason)
        }
    }

    /// Note that the operator asked the running turn to stop, so the ending
    /// that follows is read as an interruption rather than as a turn that ran
    /// its course.
    pub fn note_interrupt_requested(&mut self) {
        self.interrupt_requested = true;
    }

    /// Note that a plan window refused this interaction's work.
    pub fn note_refused(&mut self, limit: RateLimit) {
        self.refused_by = Some(limit);
    }

    /// Note that the running turn reported an error.
    pub fn note_turn_error(&mut self, message: impl Into<String>) {
        self.turn_error = Some(message.into());
    }

    /// The same, for a failure reported by the turn's own ending. A provider
    /// that announces the error and then fails the turn describes one failure
    /// twice; the announcement came first and is usually the fuller of the
    /// two, so it stands.
    pub fn note_turn_error_unless_known(&mut self, message: impl Into<String>) {
        if self.turn_error.is_none() {
            self.turn_error = Some(message.into());
        }
    }

    /// The refusal this interaction is under, taken rather than read: it
    /// explains one state, and the state it explains has just been reached.
    pub fn take_refusal(&mut self) -> Option<RateLimit> {
        self.refused_by.take()
    }

    /// Why the turn that just ended stopped, most specific answer first: a
    /// refused window is why it ran nothing at all, an interrupt is why it
    /// stopped short, an error is why it gave up, and otherwise it finished.
    ///
    /// Everything it consults goes with it. Each of those facts was about the
    /// one turn that just ended, and a later turn ending normally must not
    /// inherit any of them.
    pub fn take_turn_end_reason(&mut self) -> IdleReason {
        let interrupted = std::mem::take(&mut self.interrupt_requested);
        match (self.refused_by.take(), interrupted, self.turn_error.take()) {
            (Some(limit), _, _) => IdleReason::RateLimited(limit),
            (None, true, _) => IdleReason::Interrupted,
            (None, false, Some(message)) => IdleReason::Failed { message },
            (None, false, None) => IdleReason::TurnComplete,
        }
    }

    /// The provider's own count of what it is running in the background. Once
    /// it has reported one, that count is the only thing that moves the flag.
    pub fn note_background_count(&mut self, running: usize) {
        self.background_count_known = true;
        self.background_work = running > 0;
        if !self.background_work && self.status == Status::Background {
            self.status = Status::Idle(IdleReason::BackgroundFinished);
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
            self.status = Status::Idle(IdleReason::BackgroundFinished);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_status(status: Status) -> Activity {
        Activity {
            status,
            ..Default::default()
        }
    }

    /// A provider that reports its background set owns the answer from then
    /// on, so a heuristic guess afterwards must not contradict it.
    #[test]
    fn a_reported_count_takes_over_from_the_tool_call_heuristic() {
        let mut activity = Activity::default();
        activity.note_background_started();
        assert_eq!(
            activity.idle_or_background(IdleReason::TurnComplete),
            Status::Background
        );

        activity.note_background_count(2);
        activity.note_background_finished();

        assert_eq!(
            activity.idle_or_background(IdleReason::TurnComplete),
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

        assert_eq!(
            activity.status,
            Status::Idle(IdleReason::BackgroundFinished)
        );
        assert_eq!(
            activity.idle_or_background(IdleReason::TurnComplete),
            Status::Idle(IdleReason::TurnComplete)
        );
    }

    /// Only a Background status is displaced by the count reaching zero; an
    /// Interaction that is mid-turn stays running.
    #[test]
    fn a_cleared_count_leaves_a_running_turn_alone() {
        let mut activity = in_status(Status::Running);

        activity.note_background_count(0);

        assert_eq!(activity.status, Status::Running);
    }

    #[test]
    fn the_heuristic_still_applies_while_the_provider_stays_silent() {
        let mut activity = Activity::default();
        activity.note_background_started();
        activity.status = Status::Background;

        activity.note_background_finished();

        assert_eq!(
            activity.status,
            Status::Idle(IdleReason::BackgroundFinished)
        );
    }

    /// Attaching to an interaction that has been working for a while shows how
    /// long *it* has been working, not how long this client has been looking.
    #[test]
    fn an_adopted_status_is_dated_from_the_servers_moment_not_the_attachment() {
        let mut activity = Activity::default();

        activity.adopt_server_status_at(Status::Running, 1_000_000, 1_600_000);

        assert_eq!(activity.status, Status::Running);
        assert!(
            activity.progress().in_status >= Duration::from_secs(600),
            "the turn started ten minutes before this client saw it"
        );
        // Adopting is the whole transition, so the next frame must not read as
        // a fresh one and reset the clock it just set.
        activity.note_progress();
        assert!(activity.progress().in_status >= Duration::from_secs(600));
    }

    /// A server too old to date the activity sends `0`, and clocks that
    /// disagree can date it in the future. Neither may be turned into an age.
    #[test]
    fn an_unreported_or_future_moment_dates_the_status_now() {
        for since_ms in [0, 2_000_000] {
            let mut activity = Activity::default();
            activity.adopt_server_status_at(Status::Running, since_ms, 1_000_000);
            assert!(activity.progress().in_status < Duration::from_secs(1));
        }
    }

    /// Adopting Background brings the flag it stands for with it: otherwise
    /// the turn that follows would fall back to plain Idle and lose the
    /// interaction's background work.
    #[test]
    fn adopting_background_carries_the_background_work_it_reports() {
        let mut activity = Activity::default();

        activity.adopt_server_status_at(Status::Background, 0, 0);

        assert_eq!(
            activity.idle_or_background(IdleReason::TurnComplete),
            Status::Background
        );

        activity.adopt_server_status_at(Status::Idle(IdleReason::Reported), 0, 0);

        assert_eq!(
            activity.idle_or_background(IdleReason::TurnComplete),
            Status::Idle(IdleReason::TurnComplete)
        );
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
        let mut activity = in_status(Status::Idle(IdleReason::TurnComplete));
        activity.note_progress();
        let first = activity.progress().in_status;

        activity.note_progress();

        assert!(
            activity.progress().in_status >= first,
            "same status, same clock"
        );
    }

    /// Two sessions both sitting idle are not in the same situation, and the
    /// status has to be able to say so — that is the whole point of carrying
    /// the reason.
    #[test]
    fn two_idle_statuses_with_different_reasons_are_different_statuses() {
        assert_ne!(
            Status::Idle(IdleReason::TurnComplete),
            Status::Idle(IdleReason::Interrupted)
        );
        assert_ne!(
            Status::Stopped(StopReason::Paused),
            Status::Stopped(StopReason::NotAccepting)
        );
        // Both are still idle, though, for every caller that only asks that.
        assert!(Status::Idle(IdleReason::Interrupted).is_idle());
    }

    /// The reason joins the word the operator already reads, and the ordinary
    /// cases stay the bare word they have always been.
    #[test]
    fn a_reason_worth_naming_reaches_the_label() {
        assert_eq!(Status::Idle(IdleReason::TurnComplete).label(), "idle");
        assert_eq!(
            Status::Idle(IdleReason::Interrupted).label(),
            "idle · interrupted"
        );
        assert_eq!(
            Status::Idle(IdleReason::RateLimited(RateLimit {
                window: "five_hour".into(),
                resets_at_ms: None,
            }))
            .label(),
            "idle · rate limited (five_hour)"
        );
        assert_eq!(
            Status::Stopped(StopReason::Paused).label(),
            "stopped · you paused it"
        );
    }

    fn reported(
        activity: InteractionActivity,
        reason: Option<InteractionActivityReason>,
    ) -> InteractionSummary {
        InteractionSummary {
            id: "s1".into(),
            name: None,
            tags: Vec::new(),
            workspace_id: "w1".into(),
            selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
            workspace: std::path::PathBuf::from("/workspace"),
            driva: Default::default(),
            activity,
            activity_reason: reason,
            activity_since_ms: 0,
            idle_unseen: false,
            uncommitted_changes: false,
            checkout: None,
            last_message: None,
            auto_retry: false,
            events: 0,
            completed: false,
        }
    }

    /// The reason the server reports is the one the operator reads. Without it
    /// a client that attached after the fact could only say "idle", which is
    /// the state it can already see.
    #[test]
    fn a_reported_reason_is_adopted_as_this_clients_own() {
        assert_eq!(
            Status::reported(&reported(
                InteractionActivity::Pending,
                Some(InteractionActivityReason::Interrupted),
            )),
            Status::Idle(IdleReason::Interrupted)
        );
        assert_eq!(
            Status::reported(&reported(
                InteractionActivity::Pending,
                Some(InteractionActivityReason::RateLimited {
                    window: "five_hour".into(),
                    resets_at_ms: Some(1_000),
                }),
            )),
            Status::Idle(IdleReason::RateLimited(limit()))
        );
    }

    /// A server that says nothing about why is not made to say something: the
    /// client states the state it was told and no more.
    #[test]
    fn a_server_that_reports_no_reason_leaves_the_status_unexplained() {
        assert_eq!(
            Status::reported(&reported(InteractionActivity::Pending, None)),
            Status::Idle(IdleReason::Reported)
        );
        assert_eq!(
            Status::reported(&reported(InteractionActivity::Stopped, None)),
            Status::Stopped(StopReason::NotAccepting)
        );
    }

    /// The reasons are a vocabulary for the whole of an interaction's life, so
    /// each state takes only the ones that can explain *it*: how a turn ended
    /// says nothing about why the interaction then stopped, and a stop is no
    /// reason to be waiting for input.
    #[test]
    fn a_state_takes_only_the_reasons_that_can_explain_it() {
        assert_eq!(
            Status::reported(&reported(
                InteractionActivity::Stopped,
                Some(InteractionActivityReason::Paused),
            )),
            Status::Stopped(StopReason::Paused)
        );
        assert_eq!(
            Status::reported(&reported(
                InteractionActivity::Stopped,
                Some(InteractionActivityReason::TurnCompleted),
            )),
            Status::Stopped(StopReason::NotAccepting)
        );
        assert_eq!(
            Status::reported(&reported(
                InteractionActivity::Pending,
                Some(InteractionActivityReason::Paused),
            )),
            Status::Idle(IdleReason::Reported)
        );
    }

    fn limit() -> RateLimit {
        RateLimit {
            window: "five_hour".into(),
            resets_at_ms: Some(1_000),
        }
    }

    /// The agent reports the same ending whichever of these happened, so what
    /// the client noticed on the way there is the only thing that can tell
    /// them apart — and it has to survive until the ending arrives.
    #[test]
    fn a_turns_ending_is_read_from_what_happened_during_it() {
        let mut activity = Activity::default();
        assert_eq!(activity.take_turn_end_reason(), IdleReason::TurnComplete);

        activity.note_interrupt_requested();
        assert_eq!(activity.take_turn_end_reason(), IdleReason::Interrupted);

        activity.note_turn_error("context window exceeded");
        assert_eq!(
            activity.take_turn_end_reason(),
            IdleReason::Failed {
                message: "context window exceeded".into()
            }
        );

        // An ending that reports its own failure fills the gap for a provider
        // that never announced one separately...
        activity.note_turn_error_unless_known("the turn failed");
        assert_eq!(
            activity.take_turn_end_reason(),
            IdleReason::Failed {
                message: "the turn failed".into()
            }
        );
        // ...but the announcement, where there was one, is the account kept.
        activity.note_turn_error("context window exceeded");
        activity.note_turn_error_unless_known("the turn failed");
        assert_eq!(
            activity.take_turn_end_reason(),
            IdleReason::Failed {
                message: "context window exceeded".into()
            }
        );

        // A refused window outranks both: nothing ran to be interrupted or to
        // fail.
        activity.note_interrupt_requested();
        activity.note_turn_error("refused");
        activity.note_refused(limit());
        assert_eq!(
            activity.take_turn_end_reason(),
            IdleReason::RateLimited(limit())
        );
    }

    /// Each of those facts was about the turn that just ended. The next turn
    /// ending normally has to read as one.
    #[test]
    fn a_turns_ending_does_not_carry_over_to_the_next_one() {
        let mut activity = Activity::default();
        activity.note_interrupt_requested();
        activity.note_turn_error("broken");
        activity.note_refused(limit());

        activity.take_turn_end_reason();

        assert_eq!(activity.take_turn_end_reason(), IdleReason::TurnComplete);
    }

    /// An ending nobody attributed still says as much as the exit code allows.
    #[test]
    fn an_ending_reads_its_reason_off_the_exit() {
        assert_eq!(EndReason::infer(Some(0), None), EndReason::Completed);
        assert_eq!(EndReason::infer(Some(1), None), EndReason::Failed);
        assert_eq!(
            EndReason::infer(Some(0), Some("broken pipe")),
            EndReason::Failed
        );
        assert_eq!(EndReason::infer(None, None), EndReason::Unknown);
    }
}
