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
use styra_protocol::{
    CompletionState, Integration, InteractionActivity, InteractionActivityReason,
    InteractionSummary,
};

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
            | Some(InteractionActivityReason::ServerRestartedMidTurn)
            | Some(InteractionActivityReason::Branched)
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
            IdleReason::Failed { .. } => Some("error".into()),
            IdleReason::RateLimited(limit) => Some(format!("rate limited ({})", limit.window)),
            IdleReason::BackgroundFinished => Some("background finished".into()),
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
    /// The operator gave up on the interaction (`Z`): stopped, and stopped
    /// because the work it was doing is not worth finishing.
    Abandoned,
    /// Integration committed the interaction's work to the branch it came
    /// from. `sealed` once integration has also cleaned up after it — removed
    /// its worktree and sealed the Session; until then, retrying integration
    /// is what finishes the job.
    Integrated {
        integration: Integration,
        sealed: bool,
    },
    /// The operator sealed the interaction: stopped, done, and — unlike
    /// [`Self::Completed`] — not something resuming or a client action can
    /// undo.
    Sealed,
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
    /// As [`StopReason::ServerRestarted`], but the run went down partway
    /// through a turn, so that turn's work was cut off rather than finished.
    ServerRestartedMidTurn,
    /// The interaction is a branch just taken from another one: listed beside
    /// its source, with no agent started for it yet.
    Branched,
    /// The server still lists the interaction, but its agent no longer accepts
    /// messages and said nothing about why — a stale record a client must
    /// treat as stopped rather than queue against.
    NotAccepting,
}

impl StopReason {
    /// The server's reason for an interaction that takes no more messages.
    ///
    /// Integration and completion are checked first and independently of
    /// `reason`: they are properties of the Session, not ways a turn can end,
    /// so they can be true alongside any reason the interaction happened to
    /// stop for.
    ///
    /// The reasons that describe a turn ending — interrupted, background work
    /// running out — say nothing about why the interaction then stopped, so
    /// they come through as the bare fact that it has.
    pub fn reported(
        completed: CompletionState,
        reason: Option<&InteractionActivityReason>,
        integration: Option<&Integration>,
    ) -> Self {
        // Integration ahead of completion: it is what sealed the Session, and
        // until it has, finishing it is what the Session is waiting on.
        if let Some(integrated) = Self::integrated(completed, integration) {
            return integrated;
        }
        match completed {
            CompletionState::Sealed => return StopReason::Sealed,
            CompletionState::Completed => return StopReason::Completed,
            CompletionState::Abandoned => return StopReason::Abandoned,
            CompletionState::Active => {}
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
            Some(InteractionActivityReason::ServerRestartedMidTurn) => {
                StopReason::ServerRestartedMidTurn
            }
            Some(InteractionActivityReason::Branched) => StopReason::Branched,
            _ => StopReason::NotAccepting,
        }
    }

    /// What integration made of a Session, once it has landed the work.
    pub fn integrated(
        completed: CompletionState,
        integration: Option<&Integration>,
    ) -> Option<Self> {
        integration.map(|integration| StopReason::Integrated {
            integration: integration.clone(),
            sealed: completed == CompletionState::Sealed,
        })
    }

    /// Whether nobody chose this stop and nothing went wrong: the interaction
    /// is idle with no agent behind it, and the next message resumes one. See
    /// [`InteractionActivityReason::leaves_dormant`].
    pub fn is_dormant(&self) -> bool {
        matches!(
            self,
            StopReason::ServerRestarted | StopReason::ServerRestartedMidTurn | StopReason::Branched
        )
    }

    /// The reason as a status-line fragment. Always worth naming — unlike
    /// idling, stopping is never just what a session does next.
    pub fn label(&self) -> String {
        match self {
            StopReason::Paused => "paused".into(),
            StopReason::Completed => "completed".into(),
            StopReason::Abandoned => "abandoned".into(),
            StopReason::Sealed => "sealed".into(),
            StopReason::Integrated {
                integration,
                sealed: true,
            } => {
                let commit = short_commit(&integration.commit);
                match &integration.branch {
                    Some(branch) => format!("integrated into {branch} · {commit}"),
                    None => format!("integrated · {commit}"),
                }
            }
            StopReason::Integrated { sealed: false, .. } => "integrated · cleanup pending".into(),
            StopReason::RateLimited(limit) => format!("rate limited ({})", limit.window),
            StopReason::Failed { message } => format!("failed: {message}"),
            StopReason::Exited {
                exit_code: Some(code),
            } => format!("exited ({code})"),
            StopReason::Exited { exit_code: None } => "exited".into(),
            StopReason::ServerRestarted => "no agent since server restart".into(),
            StopReason::ServerRestartedMidTurn => "interrupted".into(),
            StopReason::Branched => "branched, not started".into(),
            StopReason::NotAccepting => "not accepting messages".into(),
        }
    }
}

/// A commit id as short as `git log --oneline` writes it.
fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
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
            EndReason::Stopped => Some("stopped".into()),
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
            // Integration is the outcome, not one more way of stopping: "stopped"
            // in front of it would bury what the operator came to read.
            Status::Stopped(reason @ StopReason::Integrated { .. }) => reason.label(),
            // Dormant work is waiting on the operator, as idle work is; only
            // the agent is missing, and the next message brings it back.
            Status::Stopped(reason) if reason.is_dormant() => format!("idle · {}", reason.label()),
            Status::Stopped(reason) => format!("stopped · {}", reason.label()),
            Status::Ended { error: Some(_), .. } => "failed".into(),
            // An ending the operator asked for is named as such rather than by
            // its exit code: they know the agent exited, and `ended (0)` only
            // invites them to wonder what happened to it.
            Status::Ended {
                reason: EndReason::Stopped,
                ..
            } => "ended · stopped".into(),
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
            InteractionActivity::Stopped => Status::Stopped(StopReason::reported(
                interaction.completed,
                reason,
                interaction.integration.as_ref(),
            )),
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
    ///
    /// Reports whether the status had in fact changed. The loop draws only
    /// when something did, and a status written from one of the several places
    /// that write it is exactly such a something — this is already the one
    /// place that notices, so it is also the one place that can say so.
    pub fn note_progress(&mut self) -> bool {
        if self.noted != self.status {
            self.noted = self.status.clone();
            self.since = Instant::now();
            return true;
        }
        false
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
mod tests;
