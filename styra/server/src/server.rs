//! Styra's Unix-socket server and server-owned interaction manager.

use crate::agent::{MountSpec, SandboxLayout, Selection};
use crate::interaction::{
    capture_driva_options, Interaction, InteractionSpec, ResolvedTemplate, SandboxBroker,
};
use crate::journal::{self, Journal};
use crate::naming::Topic;
use crate::protocol::WorkspaceSummary;
use crate::protocol::{
    Answer, CheckoutState, Contract, DrivaOptions, InteractionActivity, InteractionActivityReason,
    InteractionSummary, InteractionUpdate, LaunchMount, LaunchPolicy, LogEntry, QueuedMessage,
    SendMessage, SessionOrigin, SessionSummary, TemplateSummary,
};
use crate::protocol::{
    CreateSession, CreateWorkspace, Health, LoadedInteraction, Request, Response, ResumeSession,
    SequencedUpdate, SessionInfo, ShellInfo, StoredSession, Updates, WireResponse,
};
use crate::transport::MAX_REQUEST_BYTES;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Stands in for the session id in a planned launch: the directories it names
/// — the broker's control directory, the interaction's worktree — are only
/// created once the session exists.
const PENDING_SESSION_ID: &str = "<pending>";

#[derive(Clone)]
pub struct ServerState {
    inner: Arc<ServerInner>,
}

struct ServerInner {
    store_root: PathBuf,
    /// How this server asks Git about the operator's checkouts. A real server
    /// spawns `git`; a test builds the same server around an in-memory Git and
    /// so exercises every launch path without a repository on disk.
    git: Arc<dyn crate::git::Git>,
    /// The socket the server is bound to, removed on an explicit shutdown so
    /// the next client sees no stale socket to trip over. `None` for a server
    /// running in its client's process, which has no socket to clean up and no
    /// life of its own to end.
    socket: Option<PathBuf>,
    /// Where per-session broker control directories are staged. Beside the
    /// socket when there is one, since that is already a private per-user
    /// runtime directory; see [`ServerState::with_socket`] for the standalone
    /// server's equivalent.
    control_root: PathBuf,
    interactions: Mutex<HashMap<String, Arc<ManagedInteraction>>>,
    /// Holds the standalone store's advisory lock for this server's lifetime.
    /// Socket servers have no lock here.
    _standalone_lock: Option<std::fs::File>,
    /// Workspace metadata is one JSON document. Serialize read-modify-write
    /// launch edits so concurrent clients cannot overwrite each other's
    /// templates or mounts with stale intermediate documents.
    workspace_metadata: Mutex<()>,
    /// Set by a [`Request::Shutdown`]; the connection thread checks it after
    /// acknowledging and then exits the process.
    shutdown: AtomicBool,
    /// Plan-quota readings seen on any interaction's wire. Server-wide rather
    /// than per-interaction because the quota is the account's, so a reading
    /// taken on one session is what every other session is also spending, and
    /// kept in the store so a restart reopens knowing where each window stood.
    quota: Arc<crate::quota::QuotaLog>,
    /// The open-Interaction list, mirrored into the store so a restart lists
    /// the conversations the operator never closed rather than starting empty.
    /// Holds the rows a previous run left; see [`crate::roster`].
    roster: crate::roster::Roster,
}

/// The server owns every process represented by its interaction map. When the
/// last owner of an in-process server goes away, close their stdin here and let
/// the interactions' own destructors join their worker threads. This is also a
/// safe graceful teardown for a socket server whose serve loop returns.
///
/// Deliberately do not remove interactions through `CloseInteraction`: that
/// operation clears queued messages, while shutting down a server must leave
/// its durable queues available to the next run.
impl Drop for ServerInner {
    fn drop(&mut self) {
        let interactions = self
            .interactions
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Mirrored one last time before the agents go: each row as it finally
        // stood, so the next run lists what the operator was actually left
        // with rather than what each interaction looked like when it opened.
        self.roster.publish(
            interactions
                .values()
                .map(|managed| (managed.session_path.clone(), managed.summary()))
                .collect(),
        );
        for interaction in interactions.values() {
            interaction.stop();
        }
    }
}

/// How recently a client must have asked for an interaction's updates for it
/// to still count as being on someone's screen. A client rendering an
/// interaction polls several times a second, so this only has to outlast a
/// frame it spent redrawing or blocked on another request.
const WATCHED_FOR: Duration = Duration::from_secs(3);

/// How long after an operator focuses an interaction its checkout is asked
/// about again — see [`WorkingTree::recheck_after`].
///
/// Long enough that the reading is not simply the one the load already
/// reported back, short enough that the operator has not looked away again by
/// the time the row corrects itself.
const RECHECK_AFTER_FOCUS: Duration = Duration::from_secs(2);

/// The "newly idle" notification for one interaction: whether the interaction
/// reaching its input-waiting state is still unacknowledged news.
///
/// It is news only to an operator who is not looking at the interaction. A
/// client streaming its updates draws the turn finishing as it happens, so
/// raising a notification for it would tell them to go where they already are —
/// and the count in their footer would never fall back to nothing. So the two
/// halves are kept together: what a client is watching decides what going idle
/// records.
///
/// Only a `LoadInteraction` clears an already-raised notification. A summary is
/// a notification, not proof that an operator has actually looked at this
/// interaction.
#[derive(Default)]
struct IdleNotice {
    unseen: AtomicBool,
    /// When a client last asked for this interaction's updates, which is what
    /// a client showing it does continuously.
    watched: Mutex<Option<Instant>>,
}

impl IdleNotice {
    fn new(unseen: bool) -> Self {
        Self {
            unseen: AtomicBool::new(unseen),
            watched: Mutex::new(None),
        }
    }

    /// The interaction has reached its input-waiting state.
    fn became_idle(&self) {
        if self.watched() {
            return;
        }
        self.unseen.store(true, Ordering::Release);
    }

    fn unseen(&self) -> bool {
        self.unseen.load(Ordering::Acquire)
    }

    fn mark_seen(&self) {
        self.unseen.store(false, Ordering::Release);
    }

    /// Record that a client is streaming this interaction's updates, which is
    /// what a client showing it does continuously. Being on a client's screen
    /// is what seeing an interaction means, so this also acknowledges a
    /// notification raised before that client arrived.
    fn note_watched(&self) {
        *self
            .watched
            .lock()
            .expect("interaction watch lock poisoned") = Some(Instant::now());
        self.mark_seen();
    }

    fn watched(&self) -> bool {
        self.watched
            .lock()
            .expect("interaction watch lock poisoned")
            .is_some_and(|at| at.elapsed() < WATCHED_FOR)
    }
}

/// Everything an interaction reaching idle records: that going idle is news
/// for a client that was not watching, and what the agent left uncommitted in
/// the checkout.
///
/// One handle rather than two because the two are the same event. The
/// collector thread notices an interaction stop working in three places — a
/// turn completing, a background task set emptying, a background poll
/// finishing — and each of them has to record both, so the pair is held
/// together where forgetting one is not possible.
struct GoneIdle {
    notice: Arc<IdleNotice>,
    working_tree: Arc<WorkingTree>,
}

impl GoneIdle {
    fn became_idle(&self) {
        self.notice.became_idle();
        self.working_tree.reread();
    }
}

/// What Git says about the checkout an interaction works in, as it stood when
/// the interaction last stopped working: the branch and worktree the agent was
/// in, and whether it left work uncommitted there.
///
/// Read at that moment and held, rather than answered when a client asks,
/// because each question costs a `git` process: a navigator listing a dozen
/// interactions several times a second would spawn one per interaction per
/// refresh to learn something that only changes while an agent is running.
/// The agent has stopped by the time this is read, so the answers stay true
/// for as long as anyone is looking at them — until the operator commits or
/// switches branches, which is what they were being told to consider.
///
/// The branch is read from Git rather than taken from the name Styra gave the
/// checkout: an agent can `git checkout` its way somewhere else, and a
/// Workspace that never had a worktree made for it has a branch all the same.
struct WorkingTree {
    git: Arc<dyn crate::git::Git>,
    checkout: PathBuf,
    uncommitted: AtomicBool,
    state: Mutex<Option<CheckoutState>>,
    /// Whether a delayed re-reading is already on its way, so an operator
    /// moving in and out of an interaction queues one `git` process rather
    /// than one per visit.
    rechecking: AtomicBool,
}

impl WorkingTree {
    fn new(git: Arc<dyn crate::git::Git>, checkout: PathBuf) -> Self {
        Self {
            git,
            checkout,
            uncommitted: AtomicBool::new(false),
            state: Mutex::new(None),
            rechecking: AtomicBool::new(false),
        }
    }

    /// Ask Git again shortly, because the operator has just arrived at this
    /// interaction and was told it left work uncommitted.
    ///
    /// Arriving is often arriving to deal with it: they commit in a terminal,
    /// or discard, and the row they came from would otherwise go on claiming
    /// uncommitted work until the agent ran another turn. The reading is
    /// delayed rather than taken on the spot because the load that triggers it
    /// has just reported the current answer — a reading in the same instant
    /// could only repeat it.
    ///
    /// Nothing is scheduled unless there is something to correct: a checkout
    /// that is clean, or was never read, has no claim standing that a visit
    /// should refresh. If a turn starts during the wait the reading is a
    /// mid-turn one, which is harmless — a working interaction reports no
    /// uncommitted work at all, and going idle reads the checkout again.
    fn recheck_after(self: &Arc<Self>, delay: Duration) {
        if !self.uncommitted() {
            return;
        }
        if self.rechecking.swap(true, Ordering::AcqRel) {
            return;
        }
        let tree = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            tree.reread();
            tree.rechecking.store(false, Ordering::Release);
        });
    }

    /// Ask Git again, because the interaction has stopped writing to the
    /// checkout.
    ///
    /// A workspace outside a repository fails the questions rather than
    /// answering them, and that failure is the answer this reports: there is
    /// no history here to have left work out of, and no branch to be on, so
    /// there is nothing to say.
    fn reread(&self) {
        let uncommitted = self
            .git
            .has_uncommitted_changes(&self.checkout)
            .unwrap_or(false);
        self.uncommitted.store(uncommitted, Ordering::Release);
        // A repository that has become unreadable since the last reading
        // leaves the last reading in place rather than blanking it: the
        // operator is better served by where the work was than by nothing.
        if let Some(state) = self.read_checkout() {
            *self.state.lock().expect("checkout state lock poisoned") = Some(state);
        }
    }

    /// Where the agent is working, in Git's terms, or `None` when the
    /// workspace is not inside a working tree.
    fn read_checkout(&self) -> Option<CheckoutState> {
        let repository = self.git.discover(&self.checkout).ok().flatten()?;
        // For a main checkout the common directory is its own `.git`, so its
        // parent is that checkout; for a linked worktree it is the main
        // checkout's, so the parent is the main checkout. Either way the
        // parent is the repository the worktree belongs to.
        let main = repository
            .common_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| repository.root.clone());
        Some(CheckoutState {
            branch: self.git.current_branch(&repository.root).ok().flatten(),
            worktree: repository.root,
            repository: main,
        })
    }

    fn uncommitted(&self) -> bool {
        self.uncommitted.load(Ordering::Acquire)
    }

    fn checkout_state(&self) -> Option<CheckoutState> {
        self.state
            .lock()
            .expect("checkout state lock poisoned")
            .clone()
    }
}

/// What an interaction is doing, how it came to be doing it, and the moment it
/// started. The three are held together because the last two are only ever
/// read as the first's clock and the first's explanation, and a transition
/// that set one without the others would leave a turn dated by the one before
/// it, or described by it.
///
/// The moment is the server's own, in epoch milliseconds, because the work
/// began when the server saw it begin: a client attaching mid-turn has to
/// report how long the agent has been at it, and its first sight of the turn
/// is no answer to that. The reason is the server's for the same kind of
/// reason: it is a fact about a moment that has passed, and only the process
/// that was there when it passed can state it.
#[derive(Clone, Default)]
struct ActivityState {
    activity: InteractionActivity,
    reason: Option<InteractionActivityReason>,
    since_ms: u64,
}

/// One interaction's [`ActivityState`], shared between the threads that move
/// it: the request handlers, and the collector reading the agent's output.
#[derive(Default)]
struct CurrentActivity {
    state: Mutex<ActivityState>,
}

impl CurrentActivity {
    fn new() -> Self {
        Self {
            state: Mutex::new(ActivityState {
                activity: InteractionActivity::Pending,
                reason: None,
                since_ms: journal::now_ms(),
            }),
        }
    }

    fn get(&self) -> ActivityState {
        self.state
            .lock()
            .expect("interaction activity lock poisoned")
            .clone()
    }

    fn activity(&self) -> InteractionActivity {
        self.get().activity
    }

    /// Move to `next`, for the reason that took it there. The clock restarts
    /// only on a real change, so a turn that reasserts what it is already
    /// doing keeps the moment it started — but the reason is adopted either
    /// way, since a new one is news about now whether or not the state moved.
    fn set(&self, next: InteractionActivity, reason: Option<InteractionActivityReason>) {
        let mut state = self
            .state
            .lock()
            .expect("interaction activity lock poisoned");
        if state.activity != next {
            state.since_ms = journal::now_ms();
            state.activity = next;
        }
        state.reason = reason;
    }

    /// Record why the interaction is where it is without claiming it has moved
    /// — a plan window refusing its work is news before it is a transition,
    /// and an agent that has not yet ended is still doing whatever it was.
    fn note_reason(&self, reason: InteractionActivityReason) {
        self.state
            .lock()
            .expect("interaction activity lock poisoned")
            .reason = Some(reason);
    }

    /// Move to [`InteractionActivity::Stopped`]: the agent takes no more
    /// messages, for `ending`.
    ///
    /// A reason already on record that explains the stop outranks `ending`. An
    /// agent a plan window refused, or one the operator paused, exits as a
    /// consequence a moment later, and "the process exited" is not the news —
    /// the window is, and it is what a reset comes back to.
    fn stopped(&self, ending: InteractionActivityReason) {
        let mut state = self
            .state
            .lock()
            .expect("interaction activity lock poisoned");
        if state.activity != InteractionActivity::Stopped {
            state.since_ms = journal::now_ms();
            state.activity = InteractionActivity::Stopped;
        }
        if !state
            .reason
            .as_ref()
            .is_some_and(InteractionActivityReason::explains_stopping)
        {
            state.reason = Some(ending);
        }
    }

    /// Move to `next` only if the interaction is still doing `when`, so the
    /// answer to a stale question cannot displace a newer state — a background
    /// set reported empty says nothing about a turn that has since started.
    fn replace_if(
        &self,
        when: InteractionActivity,
        next: InteractionActivity,
        reason: Option<InteractionActivityReason>,
    ) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("interaction activity lock poisoned");
        if state.activity != when {
            return false;
        }
        if state.activity != next {
            state.since_ms = journal::now_ms();
            state.activity = next;
        }
        state.reason = reason;
        true
    }
}

struct ManagedInteraction {
    interaction: Interaction,
    updates: Arc<Mutex<Vec<SequencedUpdate>>>,
    activity: Arc<CurrentActivity>,
    /// Whether this interaction going idle is still news, and what makes it
    /// news at all: see [`IdleNotice`].
    idle: Arc<IdleNotice>,
    /// What the agent left uncommitted in the workspace when it last stopped
    /// working: see [`WorkingTree`].
    working_tree: Arc<WorkingTree>,
    /// How many agent events this interaction has produced. Counted as they
    /// arrive rather than derived from `updates` on each listing, so a summary
    /// costs a load instead of a scan of the whole history.
    events: Arc<AtomicUsize>,
    /// Captured at spawn so the interaction can be listed and reattached to without
    /// re-deriving them: the agent selection, host workspace, and launch policy.
    workspace_id: String,
    name: Mutex<Option<String>>,
    /// What the interaction is running under *now*: the operator can switch
    /// model mid-session, and every such switch is mirrored to `session.json`
    /// so reattaching or resuming picks up the switch rather than the launch.
    selection: Mutex<Selection>,
    workspace: PathBuf,
    driva: DrivaOptions,
    shell: ShellInfo,
    /// Operator messages not yet sent to the agent, durably mirrored into
    /// `session_path` on every mutation so the queue survives the operator
    /// closing the Styra UI (or the daemon restarting) before it drains.
    queue: Mutex<std::collections::VecDeque<QueuedMessage>>,
    /// Whether a plan window refusing this interaction's work should be waited
    /// out and the work asked again; mirrored into `session_path`, since the
    /// retry resumes the Session as a new interaction and the setting has to
    /// survive that. See [`ServerState::retry_after_reset`].
    auto_retry: Arc<AtomicBool>,
    /// Whether the operator has finished with this interaction's Session;
    /// mirrored into `session_path` since it is a property of the Session,
    /// not of this interaction — see [`crate::protocol::SessionSummary::completed`].
    /// Read back on resume, which is what clears it.
    completed: Arc<AtomicBool>,
    /// The window that refused this interaction's work, once one has — the
    /// reason it stopped, as opposed to a figure about how full it was. What
    /// makes this interaction one a reset should come back to.
    refused_by: Arc<Mutex<Option<crate::protocol::QuotaEvent>>>,
    /// Whether the operator has asked the running turn to stop, until the turn
    /// ends and it is read. An interrupted turn reports the same ending as one
    /// that ran its course, so nothing downstream can tell them apart; only
    /// the request itself can, and it is made on another thread than the one
    /// that sees the ending.
    interrupt_requested: Arc<AtomicBool>,
    /// This interaction's own half of the launch policy, as the client asked
    /// for it. Kept so a retry can resume the Session under the policy the
    /// operator actually granted — the mounts and templates they added for
    /// this conversation — rather than the Workspace's standing policy alone.
    launch: LaunchPolicy,
    /// The session's durable directory: its journal, metadata and queue.
    session_path: PathBuf,
}

/// How often the quota log is asked whether a plan window has come back. A
/// wait for one is minutes to hours long, so this is a cheap poll rather than
/// a scheduled wake-up; see [`ServerState::sweep_quota_resets`].
const RESET_SWEEP: Duration = Duration::from_secs(15);

/// One interaction a plan window is holding, and what asking it again takes.
struct HeldBack {
    /// The Session to resume, which is also the interaction's id.
    id: String,
    /// The interaction's own half of the launch policy, so the Session comes
    /// back in the sandbox the operator granted it rather than a plainer one.
    launch: LaunchPolicy,
    /// Whether the agent outlived the refusal and still takes messages, in
    /// which case the turn goes straight to it rather than through a resume.
    alive: bool,
}

/// Whether a window coming back is the one that refused this work.
///
/// Both halves matter: the providers are separate subscriptions, and each
/// reports several windows of its own, so a Codex hour turning over releases
/// nothing a Claude week is holding.
fn reset_releases(
    refused_by: &crate::protocol::QuotaEvent,
    reset: &crate::quota::WindowReset,
) -> bool {
    refused_by.provider == reset.provider && refused_by.window == reset.window
}

/// The three facts behind [`ManagedInteraction::awaiting_window`], asked apart
/// from the interaction holding them so the decision can be stated — and
/// tested — on its own.
fn awaiting_window(
    auto_retry: bool,
    activity: InteractionActivity,
    refused_by: Option<&crate::protocol::QuotaEvent>,
) -> Option<crate::protocol::QuotaEvent> {
    if !auto_retry {
        return None;
    }
    // An interaction with a turn under way is not waiting for anything: the
    // provider is serving it, whatever refused it earlier. Sending the held
    // back turn now would ask it a second time, beside the one running.
    if matches!(
        activity,
        InteractionActivity::Running | InteractionActivity::Background
    ) {
        return None;
    }
    // Being idle rather than stopped is not being unrefused. Claude reports a
    // rate limit and keeps its process, so a refused session sits there taking
    // messages that nothing will run until the window turns over — exactly the
    // session a reset has to come back to.
    refused_by.cloned()
}

/// Why the turn that just ended stopped, most specific answer first: a refused
/// window is why it ran nothing at all, an interrupt is why it stopped short,
/// an error is why it gave up, and otherwise it finished.
///
/// None of this is on the wire the agent ends its turn on — it reports the same
/// completion however the turn went — so each of these was noticed earlier and
/// held for this moment. They are taken rather than read, because each was
/// about the one turn that has just ended.
fn turn_end_reason(
    refused: Option<InteractionActivityReason>,
    interrupted: bool,
    failed: Option<String>,
) -> InteractionActivityReason {
    match (refused, interrupted, failed) {
        (Some(refusal), _, _) => refusal,
        (None, true, _) => InteractionActivityReason::Interrupted,
        (None, false, Some(message)) => InteractionActivityReason::Failed { message },
        (None, false, None) => InteractionActivityReason::TurnCompleted,
    }
}

fn update_finishes_background(update: &InteractionUpdate) -> bool {
    matches!(update, InteractionUpdate::Event(event) if event.finishes_background_task())
}

impl ManagedInteraction {
    /// Adopt the concrete model an agent resolved at startup. This is the
    /// interaction's selection, not display-only metadata: summaries and a
    /// later attachment must open on the same model the live process uses.
    fn note_reported_selection(&self, model: &str, effort: Option<&str>) {
        let mut selection = self
            .selection
            .lock()
            .expect("interaction selection lock poisoned");
        let previous = selection.clone();
        selection.model = model.to_owned();
        if let Some(effort) = effort
            .and_then(|effort| crate::agent::Effort::parse(effort).ok())
            .filter(|effort| selection.provider.efforts().contains(effort))
        {
            selection.effort = effort;
        }
        if *selection != previous {
            // Failure to update the durable mirror must not hide the live
            // interaction's resolved selection from a connected client.
            let _ = journal::store_session_selection(&self.session_path, &selection);
        }
    }

    fn summary(&self) -> InteractionSummary {
        let state = self.activity.get();
        let activity = state.activity;
        InteractionSummary {
            id: self.interaction.session_id().to_owned(),
            name: self
                .name
                .lock()
                .expect("session name lock poisoned")
                .clone(),
            tags: journal::session_summary_at(&self.session_path, &self.workspace_id)
                .map(|summary| summary.tags)
                .unwrap_or_default(),
            workspace_id: self.workspace_id.clone(),
            selection: self.selection(),
            workspace: self.workspace.clone(),
            driva: self.driva.clone(),
            idle_unseen: activity == InteractionActivity::Pending && self.idle.unseen(),
            // Only reported where it was read: an interaction that is working
            // again has an answer from before the turn it is running, and an
            // operator cannot act on a checkout the agent is still writing to.
            uncommitted_changes: activity == InteractionActivity::Pending
                && self.working_tree.uncommitted(),
            // Reported whatever the interaction is doing: see
            // `InteractionSummary::checkout`. Where the work is happening is
            // still the answer while the agent is working on it.
            checkout: self.working_tree.checkout_state(),
            activity,
            activity_reason: state.reason,
            activity_since_ms: state.since_ms,
            last_message: self.last_message(),
            auto_retry: self.auto_retry.load(Ordering::Acquire),
            events: self.events.load(Ordering::Acquire),
            completed: self.completed.load(Ordering::Acquire),
        }
    }

    /// Note that the operator has asked the running turn to stop, for the
    /// collector thread to read when that turn reports itself over.
    fn note_interrupt_requested(&self) {
        self.interrupt_requested.store(true, Ordering::Release);
    }

    fn mark_idle_seen(&self) {
        self.idle.mark_seen();
    }

    /// Answer "keep at it after a rate limit" for this interaction's Session,
    /// and record it where a resumed interaction will read it back.
    ///
    /// Setting it while the interaction is already stopped behind a window is
    /// the ordinary case rather than an edge: the operator finds out that a
    /// limit stopped their work by seeing it stopped.
    fn set_auto_retry(&self, enabled: bool) -> Result<()> {
        journal::store_session_auto_retry(&self.session_path, enabled)?;
        self.auto_retry.store(enabled, Ordering::Release);
        Ok(())
    }

    /// Record which window refused this interaction's work, and — when the
    /// operator has asked to keep at it — say in the interaction's own stream
    /// that the wait has started, so being stopped by a limit does not read as
    /// being stopped for good.
    fn note_refused(&self, reading: crate::protocol::QuotaEvent) {
        if self.auto_retry.load(Ordering::Acquire) {
            self.push_update(InteractionUpdate::Log(LogEntry::warn(format!(
                "{} — waiting for it to reset, then asking again",
                reading.describe()
            ))));
        }
        *self
            .refused_by
            .lock()
            .expect("interaction refusal lock poisoned") = Some(reading);
    }

    /// Forget the refusal on record, because the provider behind it is serving
    /// this interaction again.
    ///
    /// A refusal is only interesting until it stops being true, and it stops
    /// being true silently: the window that refused a Codex turn is filed under
    /// the plan itself, which no later reading ever reports on, and Claude's
    /// next permitted reading names its window without saying anything about
    /// the refusal before it. So a reading of any window from the same provider
    /// that is not itself a refusal is the provider saying it is answering —
    /// and a refusal left standing past that would have a later reset send a
    /// turn this interaction has long since had an answer to.
    fn note_serving(&self, provider: crate::agent::Provider) {
        let mut refused = self
            .refused_by
            .lock()
            .expect("interaction refusal lock poisoned");
        if refused
            .as_ref()
            .is_some_and(|refusal| refusal.provider == provider)
        {
            *refused = None;
        }
    }

    /// Add one update to this interaction's history, as the collector thread
    /// does — for the things the server itself has to say into a stream the
    /// agent is no longer producing.
    fn push_update(&self, update: InteractionUpdate) {
        let mut history = self
            .updates
            .lock()
            .expect("interaction update lock poisoned");
        let sequence = history.len() as u64 + 1;
        history.push(SequencedUpdate { sequence, update });
    }

    /// The last thing the operator asked, verbatim as it went out — framing
    /// and all, for a turn that asked its reply for a shape, so asking again
    /// asks the same question rather than an untyped version of it.
    ///
    /// Read from the interaction's history, which for a resumed interaction is
    /// seeded with the stored journal, so this answers for the conversation
    /// rather than for what this process happened to see.
    fn last_operator_turn(&self) -> Option<String> {
        let updates = self.updates.lock().expect("interaction updates poisoned");
        updates
            .iter()
            .rev()
            .find_map(|sequenced| match &sequenced.update {
                InteractionUpdate::Event(crate::event::AgentEvent::UserMessage { text }) => {
                    Some(text.clone())
                }
                _ => None,
            })
    }

    /// The window that refused this interaction's work, if this interaction is
    /// one a reset should come back to: it has to have been refused, to have
    /// been told to keep at it, and not to be running a turn already.
    ///
    /// All three are asked here rather than at the call site, because they are
    /// one question — "is this interaction waiting for a window" — and it is
    /// asked of every interaction on every reset.
    fn awaiting_window(&self) -> Option<crate::protocol::QuotaEvent> {
        awaiting_window(
            self.auto_retry.load(Ordering::Acquire),
            self.activity.activity(),
            self.refused_by
                .lock()
                .expect("interaction refusal lock poisoned")
                .as_ref(),
        )
    }

    /// The last thing the agent said, as one clipped line. Scanned from the
    /// tail so a long-running interaction pays for the trailing tool traffic
    /// only, not for its whole history.
    fn last_message(&self) -> Option<String> {
        let updates = self.updates.lock().expect("interaction updates poisoned");
        updates
            .iter()
            .rev()
            .find_map(|sequenced| match &sequenced.update {
                InteractionUpdate::Event(crate::event::AgentEvent::AgentMessage { text }) => {
                    Some(one_line(text))
                }
                _ => None,
            })
    }
}

/// Collapse a message to a single display line: whitespace runs (including the
/// newlines of a multi-paragraph answer) become single spaces, and the result
/// is clipped so one verbose agent cannot cost every listing its bandwidth.
fn one_line(text: &str) -> String {
    const LIMIT: usize = 200;
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flattened.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}…", &flattened[..end]),
        None => flattened,
    }
}

impl ManagedInteraction {
    fn selection(&self) -> Selection {
        self.selection
            .lock()
            .expect("interaction selection lock poisoned")
            .clone()
    }

    /// Move the interaction onto `selection`: apply it to the running agent,
    /// then record it, so both this attachment and the next one agree on what
    /// the session is running.
    ///
    /// Only the model can change. The provider defines the process itself, and
    /// Claude Code fixes reasoning effort for the life of the session, so those
    /// are rejected rather than silently dropped.
    fn set_selection(&self, selection: Selection) -> Result<()> {
        let mut current = self
            .selection
            .lock()
            .expect("interaction selection lock poisoned");
        if selection == *current {
            return Ok(());
        }
        if selection.provider != current.provider {
            anyhow::bail!("changing agent provider requires a new session");
        }
        if selection.provider == crate::agent::Provider::Claude
            && selection.effort != current.effort
        {
            anyhow::bail!("Claude Code fixes reasoning effort for the life of a session");
        }
        self.interaction.set_selection(&selection, &current)?;
        journal::store_session_selection(&self.session_path, &selection)?;
        *current = selection;
        Ok(())
    }

    fn set_working_directory(&self, requested: PathBuf) -> Result<()> {
        if self.activity.activity() != InteractionActivity::Pending {
            anyhow::bail!(
                "wait for the current turn to finish before changing its working directory"
            );
        }
        let host_directory = if requested.is_absolute() {
            requested
        } else {
            self.workspace.join(requested)
        }
        .canonicalize()
        .context("working directory must exist")?;
        let relative = host_directory.strip_prefix(&self.workspace).map_err(|_| {
            anyhow::anyhow!("working directory must be inside this interaction's Workspace")
        })?;
        let sandbox_directory = self.driva.working_directory.join(relative);
        self.interaction.set_working_directory(sandbox_directory)?;
        self.interaction.report_working_directory(host_directory);
        Ok(())
    }

    fn send(&self, text: &str) -> Result<()> {
        if !self.activity.activity().accepting() {
            anyhow::bail!(
                "session {} is not accepting messages",
                self.interaction.session_id()
            );
        }
        self.interaction.send(text)?;
        Ok(())
    }

    fn send_message(&self, message: SendMessage) -> Result<()> {
        if let Some(selection) = message.selection {
            self.set_selection(selection)?;
        }
        if !self.activity.activity().accepting() {
            anyhow::bail!(
                "session {} is not accepting messages",
                self.interaction.session_id()
            );
        }
        let text = match message.contract {
            Some(contract) => {
                journal::store_session_contract(&self.session_path, contract)?;
                crate::contract::frame(&message.text, contract)
            }
            None => message.text,
        };
        self.interaction
            .send_with_selection(&text, Some(&self.selection()))?;
        // The server accepts the turn before the provider echoes its
        // UserMessage event. Date it here so a client that attaches in that
        // interval still sees when this work actually started.
        // Working needs no reason beyond the message that started it, and
        // whatever ended the last turn is now answered.
        self.activity.set(InteractionActivity::Running, None);
        Ok(())
    }

    fn stop(&self) {
        self.activity.stopped(InteractionActivityReason::Paused);
        self.interaction.stop();
    }

    /// Set whether the operator has finished with this interaction's Session,
    /// mirroring the flag to `session_path` first so a crash between the two
    /// never leaves the live summary claiming a state the store disagrees
    /// with. Marking it complete also stops the interaction — there is
    /// nothing left for its agent to do — with the ordinary [`Self::stop`]
    /// reason: completion is a fact about the Session, not a new way for an
    /// interaction to be stopped.
    fn set_completed(&self, completed: bool) -> Result<()> {
        journal::store_session_completed(&self.session_path, completed)?;
        self.completed.store(completed, Ordering::Release);
        if completed {
            self.stop();
        }
        Ok(())
    }

    fn persist_queue(&self, queue: &std::collections::VecDeque<QueuedMessage>) -> Result<()> {
        let messages: Vec<QueuedMessage> = queue.iter().cloned().collect();
        journal::write_queued_messages(&self.session_path, &messages)
    }

    /// Queue a message as it was composed, contract and all: it is sent later
    /// but asked for now, and the shape belongs to the question.
    fn queue_message(&self, message: &SendMessage) -> Result<Vec<QueuedMessage>> {
        let mut queue = self.queue.lock().expect("interaction queue lock poisoned");
        queue.push_back(QueuedMessage::new(&message.text).asking_for(message.contract));
        if let Err(error) = self.persist_queue(&queue) {
            queue.pop_back();
            return Err(error);
        }
        Ok(queue.iter().cloned().collect())
    }

    fn send_queued_message(&self) -> Result<(Option<QueuedMessage>, Vec<QueuedMessage>)> {
        let mut queue = self.queue.lock().expect("interaction queue lock poisoned");
        let Some(next) = queue.pop_front() else {
            return Ok((None, Vec::new()));
        };
        if let Err(error) = self.persist_queue(&queue) {
            queue.push_front(next);
            return Err(error);
        }

        let message = SendMessage {
            text: next.text.clone(),
            selection: None,
            contract: next.contract,
        };
        if let Err(error) = self.send_message(message) {
            queue.push_front(next);
            self.persist_queue(&queue)
                .context("restore queued message after send failed")?;
            return Err(error);
        }
        let remaining = queue.iter().cloned().collect();
        Ok((Some(next), remaining))
    }

    fn clear_queue(&self) -> Result<usize> {
        let mut queue = self.queue.lock().expect("interaction queue lock poisoned");
        let count = queue.len();
        let previous = queue.clone();
        queue.clear();
        if let Err(error) = self.persist_queue(&queue) {
            *queue = previous;
            return Err(error);
        }
        Ok(count)
    }

    fn queued_messages(&self) -> Vec<QueuedMessage> {
        self.queue
            .lock()
            .expect("interaction queue lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

impl ServerState {
    /// Prepare linked worktrees for a Git-backed Workspace.
    ///
    /// `None` means launches here work in the Workspace directory itself,
    /// either because the operator has not opted in or because the directory is
    /// not inside a Git working tree and so has nothing to branch from.
    fn workspace_worktrees(
        &self,
        workspace: &WorkspaceSummary,
        create_worktree: bool,
    ) -> Result<Option<crate::worktree::Worktrees>> {
        if !create_worktree {
            return Ok(None);
        }
        let Some(repository) = self.inner.git.discover(&workspace.host_path)? else {
            return Ok(None);
        };
        crate::worktree::Worktrees::prepare(
            self.inner.git.clone(),
            repository,
            crate::workspace::worktrees_dir(&self.inner.store_root, &workspace.id),
        )
        .map(Some)
    }

    /// The checkout Session `id` works in, or `None` when it works in its
    /// Workspace directory.
    ///
    /// A worktree belongs to the Session that explicitly created it, so this
    /// is what a launch path asks before deciding to make one — it must not
    /// prepare anything, which is why it never goes through
    /// [`Self::workspace_worktrees`].
    ///
    /// The Session's own record answers first. A Session launched before that
    /// record existed has none, and for it the checkout is still found by the
    /// scan — and then written down, so each such Session is asked about the
    /// filesystem exactly once more.
    fn session_checkout(
        &self,
        session_path: &Path,
        workspace_id: &str,
        id: &str,
    ) -> Result<Option<crate::worktree::Checkout>> {
        if let Some(stored) = journal::read_session_checkout(session_path)? {
            return Ok(Some(stored));
        }
        let Some(path) = crate::worktree::existing_checkout(
            &crate::workspace::worktrees_dir(&self.inner.store_root, workspace_id),
            id,
        ) else {
            return Ok(None);
        };
        let checkout = crate::worktree::Checkout::at(path);
        journal::store_session_checkout(session_path, &checkout)?;
        Ok(Some(checkout))
    }

    /// Resolve a checkout another Session asked to share with a new one.
    ///
    /// The durable Session id is the authority: clients never send a host
    /// path, and a Session from another Workspace cannot smuggle one into
    /// this launch. A source without a checkout simply preserves the ordinary
    /// Workspace launch, which lets `n` name every current Session without
    /// first needing a separate checkout-inspection API.
    fn inherited_checkout(
        &self,
        workspace_id: &str,
        source_id: Option<&str>,
    ) -> Result<Option<crate::worktree::Checkout>> {
        let Some(source_id) = source_id else {
            return Ok(None);
        };
        let source = self.stored_summary(source_id)?;
        anyhow::ensure!(
            source.workspace_id == workspace_id,
            "cannot reuse checkout from Session {source_id:?}: it belongs to Workspace {:?}, not {workspace_id:?}",
            source.workspace_id
        );
        self.session_checkout(&source.path, workspace_id, source_id)
    }

    /// Give a Session that launched without one a checkout of its own.
    fn create_session_worktree(&self, id: &str) -> Result<()> {
        let session = self.stored_summary(id)?;
        let workspace = crate::workspace::get(&self.inner.store_root, &session.workspace_id)?;
        if self
            .session_checkout(&session.path, &workspace.id, id)?
            .is_some()
        {
            anyhow::bail!(
                "this Session already has a linked workspace; creating another is not possible"
            );
        }
        let Some(worktrees) = self.workspace_worktrees(&workspace, true)? else {
            anyhow::bail!("the Workspace is not inside a Git working tree");
        };
        let checkout = crate::worktree::Checkout::at(worktrees.checkout(id, None)?);
        journal::store_session_checkout(&session.path, &checkout)?;
        Ok(())
    }
    pub fn new(store_root: PathBuf, socket: PathBuf) -> Self {
        Self::with_socket(
            crate::git::SystemGit::shared(),
            store_root,
            Some(socket),
            None,
        )
    }

    /// A server for a host process that drives it directly, with no socket
    /// bound and so nothing listening for other clients.
    pub(crate) fn in_process(store_root: PathBuf, lock: std::fs::File) -> Self {
        Self::with_socket(
            crate::git::SystemGit::shared(),
            store_root,
            None,
            Some(lock),
        )
    }

    /// A server whose Git is supplied rather than spawned, for tests. Every
    /// other part of the server is the real one, so what a test drives here is
    /// the launch logic itself and not a reimplementation of it.
    pub fn with_git(git: Arc<dyn crate::git::Git>, store_root: PathBuf) -> Self {
        Self::with_socket(git, store_root, None, None)
    }

    fn with_socket(
        git: Arc<dyn crate::git::Git>,
        store_root: PathBuf,
        socket: Option<PathBuf>,
        standalone_lock: Option<std::fs::File>,
    ) -> Self {
        // With no socket to sit beside, the brokers go to the per-user runtime
        // directory when there is one and into the store otherwise, so a
        // standalone server still works on a host that has no runtime
        // directory at all — the case that rules out the socket in the first
        // place.
        let control_root = match socket.as_deref().and_then(Path::parent) {
            Some(parent) => parent.to_path_buf(),
            None => crate::paths::default_socket()
                .ok()
                .and_then(|socket| socket.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| store_root.clone()),
        }
        .join("sandboxes");
        let state = Self {
            inner: Arc::new(ServerInner {
                quota: Arc::new(crate::quota::QuotaLog::open(&store_root)),
                roster: crate::roster::Roster::open(&store_root),
                git,
                store_root,
                socket,
                control_root,
                interactions: Mutex::new(HashMap::new()),
                _standalone_lock: standalone_lock,
                workspace_metadata: Mutex::new(()),
                shutdown: AtomicBool::new(false),
            }),
        };
        // A plan window coming back is the server's business even while no
        // client is connected: the interaction it refused is the server's, and
        // it is the one that has to be picked up again.
        state.watch_quota_resets();
        state
    }

    pub fn store_root(&self) -> &Path {
        &self.inner.store_root
    }

    /// If a client asked the server to shut down, remove the socket and exit.
    /// Called by a connection thread only after its acknowledgement has been
    /// flushed, so the requester learns the daemon is going down.
    fn shutdown_if_requested(&self) {
        if self.inner.shutdown.load(Ordering::Acquire) {
            if let Some(socket) = &self.inner.socket {
                std::fs::remove_file(socket).ok();
            }
            std::process::exit(0);
        }
    }

    fn create_session(&self, request: CreateSession) -> Result<SessionInfo> {
        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &request.workspace_id)?;
        // Ctrl-Enter remains an explicit request for a fresh checkout even on
        // a blank screen reached with `n` from a Session that already has one.
        let inherited_checkout = if request.create_worktree {
            None
        } else {
            self.inherited_checkout(&owning_workspace.id, request.checkout_from.as_deref())?
        };
        let repository_mounts = owning_workspace
            .git_repository
            .as_deref()
            .map(|root| self.inner.git.mounts(root))
            .transpose()?
            .unwrap_or_default();
        let worktrees = self.workspace_worktrees(
            &owning_workspace,
            request.create_worktree || inherited_checkout.is_some(),
        )?;
        let automatic_mounts = worktree_mounts(worktrees.as_ref());
        let workspace = owning_workspace.host_path;
        let layout = launch_layout(worktrees.as_ref(), &workspace);
        let selection = request.selection;
        let requested_name = journal::normalize_session_name(request.name.as_deref())?;
        // What this launch is about, in one errand: the readable half of the
        // branch name and, unless the client named the Session itself, the
        // Session's own name. Skipped when it would be read by nobody — a
        // Workspace that makes no branch has no use for the branch half, and a
        // named launch has no use for the Session-name half — so an operator
        // only waits on a naming run whose answer they are going to see.
        let topic = (request.create_worktree && requested_name.is_none())
            .then(|| crate::naming::topic_for_prompt(&selection, request.message.as_deref()))
            .flatten();
        let name = requested_name
            .or_else(|| topic.as_ref().and_then(Topic::title).map(str::to_owned))
            .or_else(|| journal::name_from_message(request.message.as_deref()));
        // The Workspace's standing policy with this launch's own over it. Done
        // here rather than in the client so every launch path resolves the same
        // way, and a client cannot launch under something the plan did not show.
        let launch = LaunchPolicy::merge(&owning_workspace.launch, &request.launch);
        let mut profile = crate::agent::resolve_profile(&selection, &layout)?;
        profile.network = profile.network || launch.grants_network();
        let template = resolve_templates(&workspace, &launch.templates)?;
        let extra_mounts = resolve_launch_mounts(&launch.mounts)?;
        // Resolve host tooling before creating durable session state, so a
        // missing tmux or broker executable cannot leave an empty journal.
        let tmux = genta::agent::resolve_executable(Path::new("tmux"))
            .context("tmux is required for Styra session shells")?;
        let base = launch_base(&workspace, template.as_ref())?;
        let tooling_mounts = tooling_mounts(&profile, &tmux, &base)?;
        let (journal, id) = Journal::create_in_workspace(
            &self.inner.store_root,
            &request.workspace_id,
            &profile,
            &selection,
            name.clone(),
        )?;
        let journal_path = journal.path().to_path_buf();
        let diagnostics = journal_path
            .parent()
            .unwrap_or(&self.inner.store_root)
            .join("diagnostics.log");
        // The directory this interaction will actually work in, ready before
        // the agent is: its own checkout when the Workspace makes worktrees,
        // the Workspace directory itself otherwise. A checkout is named after
        // the work its first prompt describes — the topic resolved above — so
        // the branch it leaves behind is recognisable in the operator's own
        // `git branch`.
        let checkout = match (&inherited_checkout, &worktrees) {
            // A shared checkout is written down for this Session too, so it
            // answers for the directory it works in on its own rather than
            // through the Session it was started from — which may be closed,
            // renamed, or given a checkout of its own later.
            (Some(inherited), _) => {
                journal::store_session_checkout(&journal_path, inherited)?;
                inherited.path.clone()
            }
            (None, Some(worktrees)) => {
                let made = crate::worktree::Checkout::at(
                    worktrees.checkout(&id, topic.as_ref().map(Topic::branch))?,
                );
                // Written down here, while the name and the branch are known
                // first-hand, rather than left to be recognised later from
                // the shape of a directory name.
                journal::store_session_checkout(&journal_path, &made)?;
                made.path
            }
            (None, None) => workspace.clone(),
        };
        let spec = InteractionSpec {
            profile,
            resume_provider_session_id: None,
            working_directory: layout.workspace.clone(),
            workspace: MountSpec {
                source: checkout.clone(),
                destination: layout.workspace.clone(),
                writable: launch.grants_writable_workspace(),
            },
            repository_mounts,
            automatic_mounts,
            tooling_mounts,
            base: base.clone(),
            temporary_mounts: Vec::new(),
            extra_mounts,
            template,
            broker: Some(self.prepare_broker(&id, tmux)?),
        };
        let backend = driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: None,
            base,
        };
        let driva = capture_driva_options(&spec, &backend)?;
        let prepared_broker = spec.broker.as_ref().expect("broker was prepared");
        let shell = ShellInfo {
            tmux: prepared_broker.tmux.clone(),
            socket: prepared_broker.control.source.join("tmux.sock"),
        };
        // A policy Driva would refuse is rejected here, before anything is
        // spawned, and takes the same control-directory cleanup a failed spawn
        // does so a rejected launch leaves nothing behind.
        if let Err(error) = ensure_distinct_destinations(&driva) {
            if let Some(control) = shell.socket.parent() {
                std::fs::remove_dir_all(control).ok();
            }
            return Err(error);
        }
        let backend = Box::new(backend);
        let (interaction, receiver) =
            match Interaction::spawn(spec, backend, journal, id.clone(), diagnostics) {
                Ok(spawned) => spawned,
                Err(error) => {
                    if let Some(control) = shell.socket.parent() {
                        std::fs::remove_dir_all(control).ok();
                    }
                    return Err(error);
                }
            };
        let updates = Arc::new(Mutex::new(Vec::new()));
        let activity = Arc::new(CurrentActivity::new());
        let idle = Arc::new(IdleNotice::new(true));
        let events = Arc::new(AtomicUsize::new(0));
        let background_work = Arc::new(AtomicBool::new(false));
        let interrupt_requested = Arc::new(AtomicBool::new(false));
        let working_tree = Arc::new(WorkingTree::new(
            Arc::clone(&self.inner.git),
            checkout.clone(),
        ));
        let managed = Arc::new(ManagedInteraction {
            interaction,
            updates: Arc::clone(&updates),
            activity: Arc::clone(&activity),
            idle: Arc::clone(&idle),
            working_tree: Arc::clone(&working_tree),
            events: Arc::clone(&events),
            workspace_id: request.workspace_id.clone(),
            name: Mutex::new(name.clone()),
            selection: Mutex::new(selection.clone()),
            workspace: checkout.clone(),
            driva: driva.clone(),
            shell,
            queue: Mutex::new(std::collections::VecDeque::new()),
            auto_retry: Arc::new(AtomicBool::new(false)),
            completed: Arc::new(AtomicBool::new(false)),
            refused_by: Arc::new(Mutex::new(None)),
            interrupt_requested: Arc::clone(&interrupt_requested),
            launch: request.launch.clone(),
            session_path: journal_path
                .parent()
                .unwrap_or(&self.inner.store_root)
                .to_path_buf(),
        });
        let reported_selection = Arc::downgrade(&managed);
        let refused = Arc::downgrade(&managed);
        let interrupted = Arc::clone(&interrupt_requested);
        let quota = Arc::clone(&self.inner.quota);
        let idle = GoneIdle {
            notice: Arc::clone(&idle),
            working_tree: Arc::clone(&working_tree),
        };
        let quota_session = id.clone();
        let quota_provider = selection.provider;
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                // Once the provider reports its background-task set, that
                // count alone drives `background_work`; the tool-call
                // heuristics below are a fallback for quieter providers.
                let mut background_count_known = false;
                let mut background_polls = HashSet::new();
                // What the running turn has already told this thread about how
                // it is going to end. Neither reaches the agent's own ending
                // report, so both are held here until one arrives.
                let mut refused_window: Option<InteractionActivityReason> = None;
                let mut turn_error: Option<String> = None;
                while let Ok(update) = receiver.recv() {
                    match &update {
                        InteractionUpdate::Event(crate::event::AgentEvent::ThreadStarted {
                            model: Some(model),
                            effort,
                            ..
                        }) => {
                            if let Some(managed) = reported_selection.upgrade() {
                                managed.note_reported_selection(model, effort.as_deref());
                            }
                        }
                        InteractionUpdate::Event(event)
                            if event.background_tasks_running().is_some() =>
                        {
                            let running = event
                                .background_tasks_running()
                                .expect("guard checked the running count is present");
                            background_count_known = true;
                            background_work.store(running > 0, Ordering::Release);
                            if running == 0
                                && activity.replace_if(
                                    InteractionActivity::Background,
                                    InteractionActivity::Pending,
                                    Some(InteractionActivityReason::BackgroundFinished),
                                )
                            {
                                idle.became_idle();
                            }
                        }
                        InteractionUpdate::Event(event) if event.starts_background_task() => {
                            background_work.store(true, Ordering::Release);
                            activity.set(InteractionActivity::Running, None);
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolStarted {
                            id,
                            name,
                            ..
                        }) if matches!(
                            name.as_str(),
                            "TaskOutput" | "TaskGet" | "task_output" | "task_get"
                        ) =>
                        {
                            background_polls.insert(id.clone());
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::UserMessage {
                            ..
                        }) => {
                            // A turn under way answers for itself, and the last
                            // one's ending is no longer what this interaction is.
                            refused_window = None;
                            turn_error = None;
                            activity.set(InteractionActivity::Running, None);
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::TurnCompleted {
                            ..
                        }) => {
                            let next = if background_work.load(Ordering::Acquire) {
                                InteractionActivity::Background
                            } else {
                                InteractionActivity::Pending
                            };
                            // Each of these is taken with the turn it was
                            // about, so the next one starts clean.
                            let reason = turn_end_reason(
                                refused_window.take(),
                                interrupted.swap(false, Ordering::AcqRel),
                                turn_error.take(),
                            );
                            activity.set(next, Some(reason));
                            if next == InteractionActivity::Pending {
                                idle.became_idle();
                            }
                        }
                        // An error does not end the turn on the wire — the
                        // agent reports it and then completes as usual — so it
                        // is held until that completion says where it left the
                        // interaction. Only while a turn is running: an error
                        // reported to an idle interaction belongs to whatever
                        // failed then, not to the next turn it is sent.
                        InteractionUpdate::Event(crate::event::AgentEvent::Error { message })
                            if activity.activity() == InteractionActivity::Running =>
                        {
                            turn_error = Some(message.clone());
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolCompleted {
                            id,
                            ..
                        }) if background_polls.remove(id) => {
                            if !background_count_known && update_finishes_background(&update) {
                                background_work.store(false, Ordering::Release);
                                activity.set(
                                    InteractionActivity::Pending,
                                    Some(InteractionActivityReason::BackgroundFinished),
                                );
                                idle.became_idle();
                            }
                        }
                        _ => {}
                    }
                    // Quota figures live only on the verbatim line: the
                    // decoder keeps a token-count event's counts and drops the
                    // limits beside them, and Claude's rate-limit event decodes
                    // to its wire type alone.
                    let observed = match &update {
                        InteractionUpdate::Raw(line)
                            if line.direction == crate::protocol::Direction::FromAgent =>
                        {
                            quota.observe(&quota_session, quota_provider, line.at_ms, &line.text)
                        }
                        _ => crate::quota::Observed::default(),
                    };
                    // Which window refused the work is the interaction's own
                    // business, unlike the figures: it is why this one is about
                    // to stop, and what a later reset comes back to.
                    if let Some(refusal) = observed.rejected {
                        let reason = InteractionActivityReason::RateLimited {
                            window: refusal.window.clone(),
                            resets_at_ms: refusal.resets_at_ms,
                        };
                        // Recorded now as well as kept for the turn's ending: a
                        // refused agent often ends instead of completing a
                        // turn, and a client looking at a stopped interaction
                        // has to be told what stopped it either way.
                        activity.note_reason(reason.clone());
                        refused_window = Some(reason);
                        if let Some(managed) = refused.upgrade() {
                            managed.note_refused(refusal);
                        }
                    } else if observed.serving {
                        // The provider answered, so whatever refused this
                        // interaction earlier is history and must not send a
                        // turn again when it resets.
                        if let Some(managed) = refused.upgrade() {
                            managed.note_serving(quota_provider);
                        }
                    }
                    let announcements = observed.announce;
                    if let InteractionUpdate::Ended(end) = &update {
                        // The agent is gone: nothing more can be sent to it,
                        // whatever it was doing a moment ago.
                        activity.stopped(match &end.error {
                            Some(message) => InteractionActivityReason::Failed {
                                message: message.clone(),
                            },
                            None => InteractionActivityReason::Exited {
                                exit_code: end.exit_code,
                            },
                        });
                    }
                    // Counted here, beside the history it is a count of, so a
                    // listing client's running indicator steps once per event
                    // this interaction produced — whatever the event was.
                    if matches!(update, InteractionUpdate::Event(_)) {
                        events.fetch_add(1, Ordering::Release);
                    }
                    let mut history = updates.lock().expect("interaction update lock poisoned");
                    let sequence = history.len() as u64 + 1;
                    history.push(SequencedUpdate { sequence, update });
                    // A window filling up has to reach the operator rather than
                    // wait to be looked up, so it joins the update stream they
                    // are already reading.
                    for reading in announcements {
                        let sequence = history.len() as u64 + 1;
                        history.push(SequencedUpdate {
                            sequence,
                            update: InteractionUpdate::Quota(reading),
                        });
                    }
                }
            })
            .context("starting the interaction update collector")?;
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .insert(id.clone(), Arc::clone(&managed));

        if let Some(message) = request
            .message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            // The seed message is a turn like any other, so a contract on it is
            // framed and recorded exactly as `SendMessage` does. Doing it here
            // is what makes a typed one-shot a single request.
            let message = match request.contract {
                Some(contract) => {
                    journal::store_session_contract(&managed.session_path, contract)?;
                    crate::contract::frame(message, contract)
                }
                None => message.to_owned(),
            };
            if let Err(error) = managed.send(&message) {
                managed.stop();
                self.inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned")
                    .remove(&id);
                self.publish_roster();
                return Err(error);
            }
        }
        // Mirrored once the launch has actually taken, so a Session whose seed
        // turn could not be sent — and which is not in the list — is not left
        // in the store's copy of it either.
        self.publish_roster();

        Ok(SessionInfo {
            id,
            name,
            workspace_id: request.workspace_id,
            selection,
            workspace: checkout,
            journal_path,
            driva,
            updates_after: 0,
            queued: Vec::new(),
        })
    }

    /// Describe the sandbox a [`Self::create_session`] with these inputs would
    /// launch, without creating a session, a journal, or a control directory.
    /// It resolves the profile, template overlay and mounts the same way the
    /// real launch does, so what the operator is shown before their first
    /// message is what they will get. The one thing it cannot name is the
    /// session id, so the mounts a launch derives from it — the broker's
    /// control directory, and the worktree a Workspace that makes them would
    /// check out — carry a placeholder for the directory the launch will make.
    fn plan_session(&self, request: crate::protocol::PlanSession) -> Result<DrivaOptions> {
        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &request.workspace_id)?;
        let inherited_checkout = if request.create_worktree {
            None
        } else {
            self.inherited_checkout(&owning_workspace.id, request.checkout_from.as_deref())?
        };
        let repository_mounts = owning_workspace
            .git_repository
            .as_deref()
            .map(|root| self.inner.git.mounts(root))
            .transpose()?
            .unwrap_or_default();
        let worktrees = self.workspace_worktrees(
            &owning_workspace,
            request.create_worktree || inherited_checkout.is_some(),
        )?;
        let automatic_mounts = worktree_mounts(worktrees.as_ref());
        let workspace = owning_workspace.host_path;
        let layout = launch_layout(worktrees.as_ref(), &workspace);
        let checkout = match (&inherited_checkout, &worktrees) {
            (Some(checkout), _) => checkout.path.clone(),
            (None, Some(worktrees)) => worktrees.path(PENDING_SESSION_ID),
            (None, None) => workspace.clone(),
        };
        let launch = LaunchPolicy::merge(&owning_workspace.launch, &request.launch);
        let mut profile = crate::agent::resolve_profile(&request.selection, &layout)?;
        profile.network = profile.network || launch.grants_network();
        let template = resolve_templates(&workspace, &launch.templates)?;
        let extra_mounts = resolve_launch_mounts(&launch.mounts)?;
        let tmux = genta::agent::resolve_executable(Path::new("tmux"))
            .context("tmux is required for Styra session shells")?;
        let base = launch_base(&workspace, template.as_ref())?;
        let tooling_mounts = tooling_mounts(&profile, &tmux, &base)?;
        let spec = InteractionSpec {
            profile,
            resume_provider_session_id: None,
            working_directory: layout.workspace.clone(),
            workspace: MountSpec {
                source: checkout,
                destination: layout.workspace.clone(),
                writable: launch.grants_writable_workspace(),
            },
            repository_mounts,
            automatic_mounts,
            tooling_mounts,
            base: base.clone(),
            temporary_mounts: Vec::new(),
            extra_mounts,
            template,
            broker: Some(self.describe_broker(PENDING_SESSION_ID, tmux)),
        };
        let options = capture_driva_options(
            &spec,
            &driva::BwrapIsolation {
                executable: "bwrap".into(),
                rootfs: None,
                base,
            },
        )?;
        // Planning is also where a policy gets checked: an operator editing
        // mounts before launch learns that two of them collide now, from the
        // view they are editing, rather than from a failed launch later.
        ensure_distinct_destinations(&options)?;
        Ok(options)
    }

    /// The Driva templates a launch in this Workspace could name.
    fn list_templates(&self, workspace_id: &str) -> Result<Vec<TemplateSummary>> {
        let owning_workspace = crate::workspace::get(&self.inner.store_root, workspace_id)?;
        Ok(workspace_driva_config(&owning_workspace.host_path)?
            .effective_templates()
            .into_iter()
            .map(|(name, template)| TemplateSummary {
                name,
                description: template.description,
            })
            .collect())
    }

    fn resume_session(&self, request: ResumeSession) -> Result<SessionInfo> {
        if self
            .inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .get(&request.id)
            .is_some_and(|managed| managed.activity.activity().accepting())
        {
            anyhow::bail!("session {:?} already has a live interaction", request.id);
        }

        let summary = self.stored_summary(&request.id)?;
        // A resume revives the session on the selection the caller names, so a
        // model or effort chosen while nothing was running is what comes back
        // up. The agent is not open to the same choice: this hands the provider
        // its own native transcript, and no other provider can read it — that
        // is what converting a Session is for.
        let stored_selection = summary.selection;
        let selection = match request.selection {
            Some(selection) if selection.provider != stored_selection.provider => {
                anyhow::bail!(
                    "session {:?} holds a {} transcript and cannot be resumed as {}; convert the session instead",
                    request.id,
                    stored_selection.provider.as_str(),
                    selection.provider.as_str()
                )
            }
            Some(selection) => {
                crate::agent::validate_selection(&selection)?;
                selection
            }
            None => stored_selection.clone(),
        };
        let provider_session_id = journal::read_provider_session_id(&summary.path)?
            .with_context(|| {
                format!(
                    "session {:?} has no stored provider session id; it can be viewed but not resumed",
                    request.id
                )
            })?;
        ensure_native_session_exists(stored_selection.provider, &provider_session_id)?;
        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &summary.workspace_id)?;
        let repository_mounts = owning_workspace
            .git_repository
            .as_deref()
            .map(|root| self.inner.git.mounts(root))
            .transpose()?
            .unwrap_or_default();
        // The checkout this Session says it works in. Never create one merely
        // because this Workspace once had the old preference: a resume returns
        // to the checkout the Session has, or to the Workspace directory it
        // has always worked in.
        let stored_checkout =
            self.session_checkout(&summary.path, &owning_workspace.id, &request.id)?;
        let worktrees = self.workspace_worktrees(&owning_workspace, stored_checkout.is_some())?;
        let automatic_mounts = worktree_mounts(worktrees.as_ref());
        let workspace = owning_workspace.host_path;
        let layout = launch_layout(worktrees.as_ref(), &workspace);
        // Where the Session says its branch and its uncommitted work are, used
        // as stated: a checkout that has been renamed is still this Session's,
        // and nothing here re-derives it from the id its directory ends with.
        // Only a record pointing at a directory that is no longer there falls
        // through to making one, which is also the path a Session opted in
        // after it last ran takes.
        let checkout = match (&worktrees, &stored_checkout) {
            (Some(_), Some(stored)) if stored.path.is_dir() => stored.path.clone(),
            (Some(worktrees), _) => worktrees.checkout(&request.id, None)?,
            (None, _) => workspace.clone(),
        };
        let launch = LaunchPolicy::merge(&owning_workspace.launch, &request.launch);
        let mut profile = crate::agent::resolve_profile(&selection, &layout)?;
        profile.resume(selection.provider, &provider_session_id)?;
        profile.network = profile.network || launch.grants_network();
        let template = resolve_templates(&workspace, &launch.templates)?;
        let extra_mounts = resolve_launch_mounts(&launch.mounts)?;

        let tmux = genta::agent::resolve_executable(Path::new("tmux"))
            .context("tmux is required for Styra session shells")?;
        let base = launch_base(&workspace, template.as_ref())?;
        let tooling_mounts = tooling_mounts(&profile, &tmux, &base)?;
        // Seed the live update stream before the provider starts. Clients
        // attaching from cursor zero then receive the stored conversation and
        // all subsequent native-resume traffic as one sequence. The client
        // initiating this resume already displays the journal, so it starts
        // after this explicit boundary.
        let seeded_updates = replayed_session_updates(
            &summary.path,
            profile.protocol,
            WorkspaceMount {
                host: &checkout,
                sandbox: &layout.workspace,
            },
        )?;
        let updates_after = seeded_updates.len() as u64;
        let mut journal = Journal::open(&summary.path)?;
        // A resume onto a different model or effort is recorded the same way a
        // live switch is (see `Interaction::set_selection`): the Session's
        // stored selection is the one it was created with, so the journal's own
        // model-change records are what later say what it last ran on. The
        // comparison is against what the history says the last run used, not
        // against that stored selection — otherwise a resume that changed
        // nothing would still announce a change, once per revival. Written
        // after the stream was seeded, so the initiating client — which asked
        // for this selection and already shows it — is not told again.
        let previous = replayed_selection(&seeded_updates, &stored_selection);
        if selection != previous {
            journal.record_model_change(
                (selection.model != previous.model).then_some(selection.model.as_str()),
                (selection.effort != previous.effort).then_some(selection.effort.as_str()),
            )?;
        }
        let journal_path = journal.path().to_path_buf();
        let diagnostics = summary.path.join("diagnostics.log");
        let spec = InteractionSpec {
            profile,
            resume_provider_session_id: Some(provider_session_id),
            working_directory: layout.workspace.clone(),
            workspace: MountSpec {
                source: checkout.clone(),
                destination: layout.workspace.clone(),
                writable: launch.grants_writable_workspace(),
            },
            repository_mounts,
            automatic_mounts,
            tooling_mounts,
            base: base.clone(),
            temporary_mounts: Vec::new(),
            extra_mounts,
            template,
            broker: Some(self.prepare_broker(&request.id, tmux)?),
        };
        let backend = driva::BwrapIsolation {
            executable: "bwrap".into(),
            rootfs: None,
            base,
        };
        let driva = capture_driva_options(&spec, &backend)?;
        let prepared_broker = spec.broker.as_ref().expect("broker was prepared");
        let shell = ShellInfo {
            tmux: prepared_broker.tmux.clone(),
            socket: prepared_broker.control.source.join("tmux.sock"),
        };
        if let Err(error) = ensure_distinct_destinations(&driva) {
            if let Some(control) = shell.socket.parent() {
                std::fs::remove_dir_all(control).ok();
            }
            return Err(error);
        }
        let backend = Box::new(backend);
        let (interaction, receiver) =
            Interaction::spawn(spec, backend, journal, request.id.clone(), diagnostics)?;
        // The replayed conversation counts: a resumed interaction carries on
        // from the phase its history left the indicator at rather than
        // restarting it.
        let replayed_events = seeded_updates
            .iter()
            .filter(|sequenced| matches!(sequenced.update, InteractionUpdate::Event(_)))
            .count();
        let updates = Arc::new(Mutex::new(seeded_updates));
        let activity = Arc::new(CurrentActivity::new());
        let idle = Arc::new(IdleNotice::new(true));
        let events = Arc::new(AtomicUsize::new(replayed_events));
        let background_work = Arc::new(AtomicBool::new(false));
        // A resumed Session may carry over messages that were durably queued
        // on a previous attachment (one stopped before the interaction went
        // idle enough to send them), so reload them rather than starting empty.
        let queued = journal::read_queued_messages(&summary.path)?;
        let auto_retry = journal::read_session_auto_retry(&summary.path)?;
        // Resuming is what undoes completion: an interaction working on the
        // Session again is not one the operator is finished with, and there
        // is no separate client action to clear the flag.
        journal::store_session_completed(&summary.path, false)?;
        let interrupt_requested = Arc::new(AtomicBool::new(false));
        let working_tree = Arc::new(WorkingTree::new(
            Arc::clone(&self.inner.git),
            checkout.clone(),
        ));
        let managed = Arc::new(ManagedInteraction {
            interaction,
            updates: Arc::clone(&updates),
            activity: Arc::clone(&activity),
            idle: Arc::clone(&idle),
            working_tree: Arc::clone(&working_tree),
            events: Arc::clone(&events),
            workspace_id: summary.workspace_id.clone(),
            name: Mutex::new(summary.name.clone()),
            selection: Mutex::new(selection.clone()),
            workspace: checkout.clone(),
            driva: driva.clone(),
            shell,
            queue: Mutex::new(queued.into_iter().collect()),
            // Read back rather than defaulted: the retry that may have caused
            // this resume is the operator's standing answer to a rate limit,
            // and a Session that keeps hitting the window has to keep it.
            auto_retry: Arc::new(AtomicBool::new(auto_retry)),
            completed: Arc::new(AtomicBool::new(false)),
            refused_by: Arc::new(Mutex::new(None)),
            interrupt_requested: Arc::clone(&interrupt_requested),
            launch: request.launch.clone(),
            session_path: summary.path.clone(),
        });
        let reported_selection = Arc::downgrade(&managed);
        let refused = Arc::downgrade(&managed);
        let interrupted = Arc::clone(&interrupt_requested);
        let id = request.id.clone();
        let quota = Arc::clone(&self.inner.quota);
        let idle = GoneIdle {
            notice: Arc::clone(&idle),
            working_tree: Arc::clone(&working_tree),
        };
        let quota_session = id.clone();
        let quota_provider = selection.provider;
        std::thread::Builder::new()
            .name(format!("styra-updates-{id}"))
            .spawn(move || {
                // Once the provider reports its background-task set, that
                // count alone drives `background_work`; the tool-call
                // heuristics below are a fallback for quieter providers.
                let mut background_count_known = false;
                let mut background_polls = HashSet::new();
                // What the running turn has already told this thread about how
                // it is going to end. Neither reaches the agent's own ending
                // report, so both are held here until one arrives.
                let mut refused_window: Option<InteractionActivityReason> = None;
                let mut turn_error: Option<String> = None;
                while let Ok(update) = receiver.recv() {
                    match &update {
                        InteractionUpdate::Event(crate::event::AgentEvent::ThreadStarted {
                            model: Some(model),
                            effort,
                            ..
                        }) => {
                            if let Some(managed) = reported_selection.upgrade() {
                                managed.note_reported_selection(model, effort.as_deref());
                            }
                        }
                        InteractionUpdate::Event(event)
                            if event.background_tasks_running().is_some() =>
                        {
                            let running = event
                                .background_tasks_running()
                                .expect("guard checked the running count is present");
                            background_count_known = true;
                            background_work.store(running > 0, Ordering::Release);
                            if running == 0
                                && activity.replace_if(
                                    InteractionActivity::Background,
                                    InteractionActivity::Pending,
                                    Some(InteractionActivityReason::BackgroundFinished),
                                )
                            {
                                idle.became_idle();
                            }
                        }
                        InteractionUpdate::Event(event) if event.starts_background_task() => {
                            background_work.store(true, Ordering::Release);
                            activity.set(InteractionActivity::Running, None);
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolStarted {
                            id,
                            name,
                            ..
                        }) if matches!(
                            name.as_str(),
                            "TaskOutput" | "TaskGet" | "task_output" | "task_get"
                        ) =>
                        {
                            background_polls.insert(id.clone());
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::UserMessage {
                            ..
                        }) => {
                            // A turn under way answers for itself, and the last
                            // one's ending is no longer what this interaction is.
                            refused_window = None;
                            turn_error = None;
                            activity.set(InteractionActivity::Running, None);
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::TurnCompleted {
                            ..
                        }) => {
                            let next = if background_work.load(Ordering::Acquire) {
                                InteractionActivity::Background
                            } else {
                                InteractionActivity::Pending
                            };
                            // Each of these is taken with the turn it was
                            // about, so the next one starts clean.
                            let reason = turn_end_reason(
                                refused_window.take(),
                                interrupted.swap(false, Ordering::AcqRel),
                                turn_error.take(),
                            );
                            activity.set(next, Some(reason));
                            if next == InteractionActivity::Pending {
                                idle.became_idle();
                            }
                        }
                        // An error does not end the turn on the wire — the
                        // agent reports it and then completes as usual — so it
                        // is held until that completion says where it left the
                        // interaction. Only while a turn is running: an error
                        // reported to an idle interaction belongs to whatever
                        // failed then, not to the next turn it is sent.
                        InteractionUpdate::Event(crate::event::AgentEvent::Error { message })
                            if activity.activity() == InteractionActivity::Running =>
                        {
                            turn_error = Some(message.clone());
                        }
                        InteractionUpdate::Event(crate::event::AgentEvent::ToolCompleted {
                            id,
                            ..
                        }) if background_polls.remove(id) => {
                            if !background_count_known && update_finishes_background(&update) {
                                background_work.store(false, Ordering::Release);
                                activity.set(
                                    InteractionActivity::Pending,
                                    Some(InteractionActivityReason::BackgroundFinished),
                                );
                                idle.became_idle();
                            }
                        }
                        _ => {}
                    }
                    // Quota figures live only on the verbatim line: the
                    // decoder keeps a token-count event's counts and drops the
                    // limits beside them, and Claude's rate-limit event decodes
                    // to its wire type alone.
                    let observed = match &update {
                        InteractionUpdate::Raw(line)
                            if line.direction == crate::protocol::Direction::FromAgent =>
                        {
                            quota.observe(&quota_session, quota_provider, line.at_ms, &line.text)
                        }
                        _ => crate::quota::Observed::default(),
                    };
                    // Which window refused the work is the interaction's own
                    // business, unlike the figures: it is why this one is about
                    // to stop, and what a later reset comes back to.
                    if let Some(refusal) = observed.rejected {
                        let reason = InteractionActivityReason::RateLimited {
                            window: refusal.window.clone(),
                            resets_at_ms: refusal.resets_at_ms,
                        };
                        // Recorded now as well as kept for the turn's ending: a
                        // refused agent often ends instead of completing a
                        // turn, and a client looking at a stopped interaction
                        // has to be told what stopped it either way.
                        activity.note_reason(reason.clone());
                        refused_window = Some(reason);
                        if let Some(managed) = refused.upgrade() {
                            managed.note_refused(refusal);
                        }
                    } else if observed.serving {
                        // The provider answered, so whatever refused this
                        // interaction earlier is history and must not send a
                        // turn again when it resets.
                        if let Some(managed) = refused.upgrade() {
                            managed.note_serving(quota_provider);
                        }
                    }
                    let announcements = observed.announce;
                    if let InteractionUpdate::Ended(end) = &update {
                        // The agent is gone: nothing more can be sent to it,
                        // whatever it was doing a moment ago.
                        activity.stopped(match &end.error {
                            Some(message) => InteractionActivityReason::Failed {
                                message: message.clone(),
                            },
                            None => InteractionActivityReason::Exited {
                                exit_code: end.exit_code,
                            },
                        });
                    }
                    // Counted here, beside the history it is a count of, so a
                    // listing client's running indicator steps once per event
                    // this interaction produced — whatever the event was.
                    if matches!(update, InteractionUpdate::Event(_)) {
                        events.fetch_add(1, Ordering::Release);
                    }
                    let mut history = updates.lock().expect("interaction update lock poisoned");
                    let sequence = history.len() as u64 + 1;
                    history.push(SequencedUpdate { sequence, update });
                    // A window filling up has to reach the operator rather than
                    // wait to be looked up, so it joins the update stream they
                    // are already reading.
                    for reading in announcements {
                        let sequence = history.len() as u64 + 1;
                        history.push(SequencedUpdate {
                            sequence,
                            update: InteractionUpdate::Quota(reading),
                        });
                    }
                }
            })
            .context("starting the resumed interaction update collector")?;
        let queued = managed.queued_messages();
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .insert(request.id.clone(), managed);
        // This run owns the Session again, so a row a previous run left for it
        // is superseded rather than listed twice.
        self.inner.roster.forget(&request.id);
        self.publish_roster();

        Ok(SessionInfo {
            id: request.id,
            name: summary.name,
            workspace_id: summary.workspace_id,
            selection,
            workspace: checkout,
            journal_path,
            driva,
            updates_after,
            queued,
        })
    }

    /// Convert a stored Session's native transcript to the other interactive
    /// provider, keeping its whole history. Sugar over [`Self::branch_session`]
    /// for the common case, kept as its own operation so a caller does not
    /// need to name the general one just to flip providers.
    fn convert_session_provider(&self, id: &str) -> Result<SessionSummary> {
        let summary = self.stored_summary(id)?;
        let to_provider = other_interactive_provider(summary.selection.provider)?;
        self.branch_session(
            id,
            None,
            crate::protocol::BranchHistory::ThroughSelected,
            Some(to_provider),
        )
        .with_context(|| {
            format!(
                "converting session {id:?} from {} to {}",
                summary.selection.provider.as_str(),
                to_provider.as_str()
            )
        })
    }

    /// Branch a stored Session into a new sibling Session in the same
    /// Workspace, seeded with its history up to `at_ms` (the whole history
    /// when `None`), optionally under a different provider. The source
    /// Session, its native transcript, and its Styra journal are left
    /// untouched — a branch is always a fresh copy, never a live reference,
    /// so nothing the source does afterwards is visible on the branch and
    /// nothing the branch does is visible on the source.
    ///
    /// The branch always gets a fresh native provider session id, even when
    /// the provider does not change: Genta's own conversion only generates
    /// one when the format changes, but a same-provider branch still needs
    /// its own id, or the provider's own `--resume` lookup — which searches
    /// its whole session tree by id — could not tell the two apart.
    fn branch_session(
        &self,
        id: &str,
        at_ms: Option<u64>,
        history: crate::protocol::BranchHistory,
        provider: Option<crate::agent::Provider>,
    ) -> Result<SessionSummary> {
        let summary = self.stored_summary(id)?;
        let from_provider = summary.selection.provider;
        let to_provider = provider.unwrap_or(from_provider);
        if !crate::agent::PROVIDERS.contains(&to_provider) {
            anyhow::bail!(
                "provider {:?} is not an interactive provider Styra can branch into",
                to_provider.as_str()
            );
        }
        let provider_session_id = journal::read_provider_session_id(&summary.path)?
            .with_context(|| {
                format!("session {id:?} has no stored provider session id; there is no native transcript to branch from")
            })?;
        let source_path = find_native_session_file(from_provider, &provider_session_id)?;
        let source = std::fs::read_to_string(&source_path)
            .with_context(|| format!("reading {}", source_path.display()))?;

        let owning_workspace =
            crate::workspace::get(&self.inner.store_root, &summary.workspace_id)?;
        let cwd = owning_workspace.host_path;
        let new_native_id = uuid::Uuid::new_v4().to_string();
        // What the branch point lands on, when it lands on an operator
        // message: the provider stamps its copy of that message after the
        // host sent it, so the text is what identifies it across the two
        // clocks.
        let selected_operator_message = at_ms
            .map(|cutoff| journal::operator_message_at(&summary.path, cutoff))
            .transpose()?
            .flatten();
        let branched = branch_native_session(
            &source,
            native_session_format(from_provider),
            native_session_format(to_provider),
            &new_native_id,
            &cwd,
            BranchPoint {
                at_ms,
                history,
                selected_operator_message: selected_operator_message.as_deref(),
            },
        )?;

        let destination = native_session_destination(to_provider, &new_native_id, &cwd)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&destination, &branched)
            .with_context(|| format!("writing {}", destination.display()))?;

        // A same-provider branch is a checkpoint: it keeps running under the
        // exact model and effort the source had, not the provider's defaults.
        let selection = if to_provider == from_provider {
            summary.selection.clone()
        } else {
            Selection::new(to_provider)
        };
        let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&cwd))?;
        let (mut journal, new_id) = Journal::create_in_workspace(
            &self.inner.store_root,
            &summary.workspace_id,
            &profile,
            &selection,
            summary.name.clone(),
        )?;
        // The marker goes in before the copied history: it is the branch's
        // first line, so reading the new Session from the top starts with
        // where its conversation came from.
        journal.record_branch(
            crate::event::BranchDirection::From,
            id,
            summary.name.as_deref(),
        )?;
        let source_protocol = journal::read_session_meta(&summary.path)?.protocol;
        journal.copy_branch_from(&summary.path, source_protocol, at_ms, history)?;
        let directory = journal
            .path()
            .parent()
            .context("a freshly created session journal has a parent directory")?;
        journal::store_provider_session_id(directory, &new_native_id)?;
        journal::store_session_origin(
            directory,
            SessionOrigin {
                session_id: id.to_owned(),
                provider: from_provider,
                at_ms,
                history,
            },
        )?;
        self.record_source_branch(id, &summary.path, &new_id, summary.name.as_deref())?;

        self.stored_summary(&new_id)
    }

    /// Mark, in the source Session, that its history was continued in
    /// `branch_id`. A live source records through its interaction, so an
    /// attached client sees the marker arrive in its event stream; a source
    /// that is only stored gets the record appended to its journal, where its
    /// next replay picks it up. The source keeps running either way — a
    /// branch takes a copy and leaves the conversation it came from intact.
    fn record_source_branch(
        &self,
        id: &str,
        source_path: &Path,
        branch_id: &str,
        branch_name: Option<&str>,
    ) -> Result<()> {
        if let Ok(interaction) = self.interaction(id) {
            return interaction
                .interaction
                .record_branch(branch_id, branch_name);
        }
        let directory = if source_path.is_dir() {
            source_path.to_path_buf()
        } else {
            source_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        };
        Journal::open(&directory)?.record_branch(
            crate::event::BranchDirection::To,
            branch_id,
            branch_name,
        )
    }

    fn interaction(&self, id: &str) -> Result<Arc<ManagedInteraction>> {
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .get(id)
            .cloned()
            .with_context(|| format!("no live interaction for session {id:?}"))
    }

    /// Whether an agent this server owns is serving `id`. Distinguishes a
    /// Session with a process behind it from one that only exists on disk.
    fn has_live_interaction(&self, id: &str) -> bool {
        self.inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .contains_key(id)
    }

    /// Open a row a previous run left, if `id` names one.
    ///
    /// There is no agent and no live update stream behind it, so the history
    /// is replayed out of the Session's journal — the same reconstruction a
    /// resume seeds its stream with. What the operator gets is the
    /// conversation, readable, marked stopped; sending to it takes a resume,
    /// which is the request that actually starts an agent.
    fn load_restored(&self, id: &str) -> Result<Option<LoadedInteraction>> {
        let Some((summary, session_path)) = self.inner.roster.restored_session(id) else {
            return Ok(None);
        };
        let meta = journal::read_session_meta(&session_path)?;
        // The mount the previous run actually launched under, taken from the
        // row it mirrored rather than re-derived: what a launch *now* would
        // bind is a different question, and the directories this row's journal
        // names are the old sandbox's.
        let replayed = replayed_session_updates(
            &session_path,
            meta.protocol,
            WorkspaceMount {
                host: &summary.workspace,
                sandbox: &summary.driva.working_directory,
            },
        )?;
        let next = replayed
            .last()
            .map(|update| update.sequence)
            .unwrap_or_default();
        Ok(Some(LoadedInteraction {
            summary,
            updates: Updates {
                updates: replayed,
                next,
            },
            // Messages queued against the previous run's Interaction are
            // waiting for an agent that a resume will launch; that resume
            // reloads them itself, so nothing is lost by not showing them on
            // an Interaction that cannot send anything.
            queued: Vec::new(),
        }))
    }

    /// Mirror the open-Interaction list into the store. Called wherever that
    /// list changes — an Interaction opening, resuming, or being closed — and
    /// again as the server goes down; see [`crate::roster`].
    fn publish_roster(&self) {
        let rows = {
            let interactions = self
                .inner
                .interactions
                .lock()
                .expect("server interaction lock poisoned");
            interactions
                .values()
                .map(|managed| (managed.session_path.clone(), managed.summary()))
                .collect()
        };
        self.inner.roster.publish(rows);
    }

    /// Give the quota log's clock a turn: pick up whatever the windows that
    /// have just come back were holding.
    ///
    /// Polled rather than scheduled. A wait for a plan window is minutes to
    /// hours long, so being a few seconds late costs nothing, and the check is
    /// a flag read while no window is being waited for at all — which is
    /// almost always.
    fn sweep_quota_resets(&self) {
        if !self.inner.quota.is_awaiting_reset() {
            return;
        }
        for reset in self.inner.quota.resets(journal::now_ms()) {
            self.retry_after_reset(&reset);
        }
    }

    /// Ask again everything `reset`'s window refused.
    ///
    /// The candidates are collected before any of them is resumed, because
    /// resuming takes the same lock that listing them does — and takes as long
    /// as launching a sandbox, which is not a lock to hold across.
    ///
    /// A refusal does not always take the agent with it — Claude reports the
    /// limit and keeps its process, leaving the session idle behind a window
    /// that will not run anything — so one that is still alive is asked again
    /// where it stands, and only a session whose agent is gone is revived
    /// first.
    fn retry_after_reset(&self, reset: &crate::quota::WindowReset) {
        for held in self.held_back_by(reset) {
            let HeldBack { id, launch, alive } = held;
            if !alive {
                if let Err(error) = self.resume_session(ResumeSession {
                    id: id.clone(),
                    launch,
                    // An unattended retry revives the Session exactly as it
                    // was held back: there is no operator here to have chosen
                    // otherwise.
                    selection: None,
                }) {
                    // The refused interaction is still the one in the map, so
                    // its own stream is where an operator will look for the
                    // reason their session did not come back after all.
                    self.say(
                        &id,
                        LogEntry::error(format!(
                            "the {} window has reset, but this Session could not be resumed: {error:#}",
                            reset.window
                        )),
                    );
                    continue;
                }
            }
            self.say(
                &id,
                LogEntry::info(format!(
                    "the {} window has reset — asking again",
                    reset.window
                )),
            );
            let sent = self.interaction(&id).and_then(|interaction| {
                // Not the refused turn itself, verbatim: the agent was already
                // told what to do, and a rate limit is not a reason to repeat
                // it. "continue" is enough to pick the same turn back up.
                interaction.send_message(SendMessage::new("continue"))?;
                // Asked again, so the refusal has been acted on: what happens
                // to this turn is the new turn's business.
                interaction.note_serving(reset.provider);
                Ok(())
            });
            if let Err(error) = sent {
                self.say(
                    &id,
                    LogEntry::error(format!("could not ask again after the reset: {error:#}")),
                );
            }
        }
    }

    /// The interactions `reset`'s window is holding: ones it refused and that
    /// were told to keep at it — with the turn each was refused in the middle
    /// of, the policy it was launched under, and whether its agent is still
    /// there to be asked.
    fn held_back_by(&self, reset: &crate::quota::WindowReset) -> Vec<HeldBack> {
        let interactions = self
            .inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned");
        let mut held = Vec::new();
        for managed in interactions.values() {
            let Some(refusal) = managed.awaiting_window() else {
                continue;
            };
            if !reset_releases(&refusal, reset) {
                continue;
            }
            let id = managed.interaction.session_id().to_owned();
            if managed.last_operator_turn().is_none() {
                // Nothing was asked, so there is nothing to continue. Said in
                // the interaction's own stream, since the operator asked for a
                // retry and is owed the reason there was none.
                managed.push_update(InteractionUpdate::Log(LogEntry::warn(format!(
                    "the {} window has reset, but this Session has no turn to ask again",
                    reset.window
                ))));
                continue;
            }
            held.push(HeldBack {
                id,
                launch: managed.launch.clone(),
                alive: managed.activity.activity().accepting(),
            });
        }
        held
    }

    /// Say something in one interaction's own update stream, if it still has
    /// one. Used for what the server says on an interaction's behalf while no
    /// agent of its own is producing anything.
    fn say(&self, id: &str, entry: LogEntry) {
        if let Ok(interaction) = self.interaction(id) {
            interaction.push_update(InteractionUpdate::Log(entry));
        }
    }

    /// Watch for plan windows coming back, for as long as this server exists.
    ///
    /// The thread holds a weak reference: it must not keep an in-process
    /// server alive after its client has gone, and must not outlive one by
    /// more than a tick.
    fn watch_quota_resets(&self) {
        let inner = Arc::downgrade(&self.inner);
        let watching = std::thread::Builder::new()
            .name("styra-quota-resets".into())
            .spawn(move || loop {
                std::thread::sleep(RESET_SWEEP);
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                Self { inner }.sweep_quota_resets();
            });
        if let Err(error) = watching {
            // Worth starting without: every other thing the server does still
            // works, and a rate limit then waits for the operator as it did
            // before there was anything to wait for them.
            eprintln!("styra-server: not watching for plan-window resets: {error}");
        }
    }

    /// Where a session's broker lives and how it is mounted, named but not yet
    /// created. Splitting this from [`Self::prepare_broker`] lets the launch
    /// policy be described — for a session that does not exist yet — without
    /// staging anything on disk for it.
    fn describe_broker(&self, id: &str, tmux: PathBuf) -> SandboxBroker {
        let control_root = &self.inner.control_root;
        SandboxBroker {
            executable: PathBuf::from("/tmp/styra/control/styra-broker"),
            tmux,
            control: MountSpec {
                source: control_root.join(id),
                destination: PathBuf::from("/tmp/styra/control"),
                writable: true,
            },
            socket: PathBuf::from("/tmp/styra/control/tmux.sock"),
        }
    }

    fn prepare_broker(&self, id: &str, tmux: PathBuf) -> Result<SandboxBroker> {
        let broker = self.describe_broker(id, tmux);
        let control = broker.control.source.clone();
        std::fs::create_dir_all(&control)
            .with_context(|| format!("creating sandbox control directory {}", control.display()))?;
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o700)).with_context(
            || {
                format!(
                    "restricting sandbox control directory {}",
                    control.display()
                )
            },
        )?;
        // The server is deliberately long-lived, while its installed binary
        // may be replaced by a rebuild or upgrade. Do not hand Driva that
        // potentially stale pathname: keep an executable copy owned by this
        // interaction. On Linux `/proc/self/exe` remains readable even after
        // the original directory entry has been unlinked.
        let executable = control.join("styra-broker");
        let running_executable = std::env::current_exe()
            .ok()
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("/proc/self/exe"));
        std::fs::copy(&running_executable, &executable).with_context(|| {
            format!(
                "staging the Styra sandbox broker from {} to {}",
                running_executable.display(),
                executable.display()
            )
        })?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .with_context(|| {
                format!("making sandbox broker {} executable", executable.display())
            })?;
        Ok(broker)
    }

    fn shell(&self, id: &str) -> Result<ShellInfo> {
        let interaction = self.interaction(id)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if interaction.shell.socket.exists() {
                return Ok(interaction.shell.clone());
            }
            if !interaction.activity.activity().accepting() {
                anyhow::bail!("session {id:?} has ended; its sandbox shell is no longer running");
            }
            if Instant::now() >= deadline {
                anyhow::bail!("session {id:?} sandbox shell did not become ready");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Parse a session's most recent agent message as a typed answer.
    ///
    /// A live interaction is answered from the updates it has already
    /// collected, which are the same events the interface is rendering; a
    /// session with no live interaction is answered by replaying its journal.
    /// Both paths reach the same decoded [`AgentEvent`]s, so an answer does not
    /// depend on whether the session that produced it is still running.
    fn turn_answer(&self, id: &str, contract: Option<Contract>) -> Result<Answer> {
        let live = self
            .inner
            .interactions
            .lock()
            .expect("server interaction lock poisoned")
            .get(id)
            .cloned();
        let (events, session_path) = match live {
            Some(interaction) => {
                let events = interaction
                    .updates
                    .lock()
                    .expect("interaction update lock poisoned")
                    .iter()
                    .filter_map(|update| match &update.update {
                        InteractionUpdate::Event(event) => Some(event.clone()),
                        _ => None,
                    })
                    .collect();
                (events, interaction.session_path.clone())
            }
            None => {
                let summary = self.stored_summary(id)?;
                let meta = journal::read_session_meta(&summary.path)?;
                (journal::replay(&summary.path, meta.protocol)?, summary.path)
            }
        };
        // An explicit contract re-reads an existing answer as another shape,
        // which is also the only way to type a turn that was sent untyped.
        let contract = match contract {
            Some(contract) => contract,
            None => journal::read_session_contract(&session_path)?.with_context(|| {
                format!("session {id:?} has no typed turn to answer; name a contract to parse its last message as one")
            })?,
        };
        crate::contract::answer_from_events(&events, contract)
    }

    fn stored_summary(&self, id: &str) -> Result<SessionSummary> {
        // Probe each Workspace for this exact id rather than listing every
        // Session in it: the directory name *is* the id, so a stat per
        // Workspace replaces reading every stored session's metadata.
        for workspace in crate::workspace::list(&self.inner.store_root)? {
            if let Some(session) =
                journal::find_workspace_session(&self.inner.store_root, &workspace.id, id)?
            {
                return Ok(session);
            }
        }
        anyhow::bail!("stored session {id:?} was not found")
    }

    /// The two ends of the Workspace mount a stored Session ran under: the host
    /// directory bound into it, and where the agent saw it.
    ///
    /// Read off what the Session and its Workspace already say, never by
    /// preparing anything: this answers a question about a run that is over, and
    /// a Session holding a checkout that has since been removed is one whose
    /// journal still names directories inside it. The rule is
    /// [`launch_layout`]'s — a checkout is bound at the fixed layout because its
    /// path under the store means nothing to the agent, a Workspace directory at
    /// its own host path — applied to stored state rather than to worktrees this
    /// Session may no longer have.
    fn stored_workspace_mount(&self, summary: &SessionSummary) -> Result<(PathBuf, PathBuf)> {
        if let Some(checkout) = journal::read_session_checkout(&summary.path)? {
            return Ok((checkout.path, SandboxLayout::default().workspace));
        }
        let workspace = crate::workspace::get(&self.inner.store_root, &summary.workspace_id)?;
        Ok((
            workspace.host_path.clone(),
            workspace_layout(&workspace.host_path).workspace,
        ))
    }

    fn list_tags(&self) -> Result<Vec<String>> {
        let mut tags = std::collections::BTreeSet::new();
        for workspace in crate::workspace::list(&self.inner.store_root)? {
            for session in journal::list_workspace_sessions(&self.inner.store_root, &workspace.id)?
            {
                tags.extend(session.tags);
            }
        }
        Ok(tags.into_iter().collect())
    }

    /// Serve one request. Available to a host running the server in-process so
    /// it can call the same dispatch without a socket (see
    /// [`crate::Client::in_process`]).
    pub(crate) fn handle(&self, request: Request) -> Result<Response> {
        match request {
            Request::Health => Ok(Response::Health(Health {
                service: "styra".into(),
            })),
            Request::CreateWorkspace(CreateWorkspace {
                host_path,
                name,
                git_repository,
            }) => Ok(Response::WorkspaceCreated(
                crate::workspace::create_with_repository(
                    self.inner.git.as_ref(),
                    &self.inner.store_root,
                    &host_path,
                    name,
                    git_repository.as_deref(),
                )?,
            )),
            Request::ListWorkspaces => Ok(Response::Workspaces(crate::workspace::list(
                &self.inner.store_root,
            )?)),
            Request::Workspace { id } => {
                let _metadata = self
                    .inner
                    .workspace_metadata
                    .lock()
                    .expect("server workspace metadata lock poisoned");
                Ok(Response::Workspace(crate::workspace::access(
                    &self.inner.store_root,
                    &id,
                )?))
            }
            Request::WorkspaceForPath { path } => Ok(Response::WorkspaceForPath(
                crate::workspace::for_path(&self.inner.store_root, &path)?,
            )),
            Request::SetWorkspaceGitRepository {
                workspace_id,
                git_repository,
            } => {
                let _metadata = self
                    .inner
                    .workspace_metadata
                    .lock()
                    .expect("server workspace metadata lock poisoned");
                Ok(Response::WorkspaceGitRepositoryUpdated(
                    crate::workspace::set_git_repository(
                        self.inner.git.as_ref(),
                        &self.inner.store_root,
                        &workspace_id,
                        git_repository.as_deref(),
                    )?,
                ))
            }
            Request::WorkspaceLaunch { workspace_id } => {
                let _metadata = self
                    .inner
                    .workspace_metadata
                    .lock()
                    .expect("server workspace metadata lock poisoned");
                Ok(Response::WorkspaceLaunch(crate::workspace::launch(
                    &self.inner.store_root,
                    &workspace_id,
                )?))
            }
            Request::CreateSession(request) => {
                Ok(Response::SessionCreated(self.create_session(request)?))
            }
            Request::PlanSession(request) => Ok(Response::SessionPlan(self.plan_session(request)?)),
            Request::ListTemplates { workspace_id } => {
                Ok(Response::Templates(self.list_templates(&workspace_id)?))
            }
            Request::ResumeSession(request) => {
                Ok(Response::SessionResumed(self.resume_session(request)?))
            }
            Request::CreateSessionWorktree { id } => {
                self.create_session_worktree(&id)?;
                Ok(Response::SessionWorktreeCreated)
            }
            Request::ConvertSessionProvider { id } => Ok(Response::SessionConverted(
                self.convert_session_provider(&id)?,
            )),
            Request::BranchSession {
                id,
                at_ms,
                history,
                provider,
            } => Ok(Response::SessionBranched(
                self.branch_session(&id, at_ms, history, provider)?,
            )),
            Request::RenameSession(request) => {
                let summary = self.stored_summary(&request.id)?;
                let name = journal::store_session_name(&summary.path, request.name.as_deref())?;
                if let Some(interaction) = self
                    .inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned")
                    .get(&request.id)
                {
                    *interaction.name.lock().expect("session name lock poisoned") = name;
                }
                Ok(Response::SessionRenamed(self.stored_summary(&request.id)?))
            }
            Request::SetSessionTags(request) => {
                let summary = self.stored_summary(&request.id)?;
                journal::store_session_tags(&summary.path, &request.tags)?;
                Ok(Response::SessionTagsUpdated(
                    self.stored_summary(&request.id)?,
                ))
            }
            Request::ListTags => Ok(Response::Tags(self.list_tags()?)),
            Request::ChangeWorkspaceLaunch {
                workspace_id,
                change,
            } => {
                let _metadata = self
                    .inner
                    .workspace_metadata
                    .lock()
                    .expect("server workspace metadata lock poisoned");
                Ok(Response::WorkspaceLaunchUpdated(
                    crate::workspace::change_launch(&self.inner.store_root, &workspace_id, change)?,
                ))
            }
            Request::SendMessage { id, message } => {
                let interaction = self.interaction(&id)?;
                interaction.send_message(message)?;
                Ok(Response::Accepted)
            }
            Request::SetSessionSelection { id, selection } => {
                crate::agent::validate_selection(&selection)?;
                self.interaction(&id)?.set_selection(selection)?;
                Ok(Response::Accepted)
            }
            Request::SetInteractionWorkingDirectory { id, directory } => {
                self.interaction(&id)?.set_working_directory(directory)?;
                Ok(Response::Accepted)
            }
            Request::SetInteractionAutoRetry { id, enabled } => {
                self.interaction(&id)?.set_auto_retry(enabled)?;
                Ok(Response::Accepted)
            }
            Request::QueueMessage { id, message } => Ok(Response::QueuedMessages(
                self.interaction(&id)?.queue_message(&message)?,
            )),
            Request::SendQueuedMessage { id } => {
                let (sent, queued) = self.interaction(&id)?.send_queued_message()?;
                Ok(Response::SentQueuedMessage(sent, queued))
            }
            Request::ClearQueuedMessages { id } => {
                Ok(Response::Queued(self.interaction(&id)?.clear_queue()?))
            }
            Request::InterruptInteraction { id } => {
                let interaction = self.interaction(&id)?;
                interaction.interaction.interrupt()?;
                // Only once the interrupt has actually gone out: a turn that
                // ends after a refused request ended on its own.
                interaction.note_interrupt_requested();
                Ok(Response::Accepted)
            }
            Request::StopInteraction { id } => {
                self.interaction(&id)?.stop();
                Ok(Response::Accepted)
            }
            Request::SetSessionCompleted { id, completed } => {
                // Restored rows already stopped when the previous server
                // exited. They are still selectable in the navigator but are
                // not represented by a live `ManagedInteraction` here, so the
                // flag is written straight to the Session's stored metadata
                // and the mirrored row is told to catch up.
                //
                // A Session that never ran under this server is not even
                // mirrored in the roster — the stored-sessions picker lists it
                // straight off disk — and completion is still its own stored
                // property, so the same write serves it with no row to update.
                if !self.has_live_interaction(&id) {
                    let summary = self.stored_summary(&id)?;
                    journal::store_session_completed(&summary.path, completed)?;
                    if self.inner.roster.set_completed(&id, completed) {
                        self.publish_roster();
                    }
                    return Ok(Response::Accepted);
                }
                self.interaction(&id)?.set_completed(completed)?;
                self.publish_roster();
                Ok(Response::Accepted)
            }
            Request::CloseInteraction { id } => {
                // Closing a row a previous run left is the whole of the work:
                // there is no agent to stop and no queue to clear, only the
                // listing to take away — and it must not come back.
                if self.inner.roster.holds(&id) {
                    self.inner.roster.forget(&id);
                    self.publish_roster();
                    return Ok(Response::Accepted);
                }
                let interaction = self.interaction(&id)?;
                interaction.stop();
                // Queued messages would otherwise be waiting for an interaction
                // that no longer exists, exactly as pausing discards them.
                interaction.clear_queue()?;
                self.inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned")
                    .remove(&id);
                self.publish_roster();
                Ok(Response::Accepted)
            }
            Request::LoadInteraction { id } => {
                if let Some(loaded) = self.load_restored(&id)? {
                    return Ok(Response::InteractionLoaded(loaded));
                }
                let interaction = self.interaction(&id)?;
                // Loading is the point at which an operator can actually see
                // the interaction. Listing it must leave an idle notification
                // intact, but the focused screen acknowledges it.
                interaction.mark_idle_seen();
                // And it is the point at which they can act on what the agent
                // left behind, which is usually why they came — so the
                // checkout is asked about once more just after they arrive,
                // rather than standing unread until the next turn ends.
                interaction.working_tree.recheck_after(RECHECK_AFTER_FOCUS);
                let summary = interaction.summary();
                let all = interaction
                    .updates
                    .lock()
                    .expect("interaction update lock poisoned");
                let updates = Updates {
                    updates: all.clone(),
                    next: all.last().map(|update| update.sequence).unwrap_or(0),
                };
                drop(all);
                let queued = interaction.queued_messages();
                Ok(Response::InteractionLoaded(LoadedInteraction {
                    summary,
                    updates,
                    queued,
                }))
            }
            Request::Updates { id, after, raw } => {
                // A row a previous run left has no stream of its own: its
                // whole history arrived with the load, and no agent is going
                // to add to it. Say so plainly rather than failing, since a
                // client showing it polls this every frame.
                if self.inner.roster.holds(&id) {
                    return Ok(Response::Updates(Updates {
                        updates: Vec::new(),
                        next: after,
                    }));
                }
                let interaction = self.interaction(&id)?;
                // Asking for an interaction's stream is a client drawing it, so
                // this is where the server learns which interactions are in
                // front of an operator — and going idle in front of one is not
                // news to report.
                interaction.idle.note_watched();
                let all = interaction
                    .updates
                    .lock()
                    .expect("interaction update lock poisoned");
                // A client that renders no raw view (the picker preview) asks
                // for none: raw lines are the bulk of an interaction's volume,
                // and cloning then shipping them only to be dropped is what
                // made replaying a long session from zero slow.
                let updates = all
                    .iter()
                    .filter(|update| update.sequence > after)
                    .filter(|update| raw || !matches!(update.update, InteractionUpdate::Raw(_)))
                    .cloned()
                    .collect();
                let next = all.last().map(|update| update.sequence).unwrap_or(after);
                Ok(Response::Updates(Updates { updates, next }))
            }
            Request::ListInteractions => {
                let interactions = self
                    .inner
                    .interactions
                    .lock()
                    .expect("server interaction lock poisoned");
                let mut summaries: Vec<InteractionSummary> = interactions
                    .values()
                    .map(|managed| managed.summary())
                    .collect();
                drop(interactions);
                // Interactions a previous run had open and the operator never
                // closed. They are listed beside this run's own, stopped: the
                // conversation is still theirs to go back to, and only the
                // agent is gone. See [`crate::roster`].
                summaries.extend(self.inner.roster.restored());
                // Newest first: the id embeds a millisecond timestamp, so a
                // descending id sort orders interactions by creation time.
                summaries.sort_by(|a, b| b.id.cmp(&a.id));
                Ok(Response::Interactions(summaries))
            }
            Request::ListSessions { workspace_id } => Ok(Response::StoredSessions(
                journal::list_workspace_sessions(self.store_root(), &workspace_id)?,
            )),
            Request::StoredSession { id, raw: want_raw } => {
                let summary = self.stored_summary(&id)?;
                let meta = journal::read_session_meta(&summary.path)?;
                let events = journal::replay(&summary.path, meta.protocol)?;
                // Reconstructing the raw lines re-reads and re-parses the whole
                // journal, so only pay for it when the caller renders them.
                let raw = if want_raw {
                    journal::replay_raw(&summary.path)?
                } else {
                    Vec::new()
                };
                // Where it was working, which a replay screen has no other way
                // to learn: it has no live Interaction to be told by, and the
                // directory is not in any event it is about to show.
                let (host, sandbox) = self.stored_workspace_mount(&summary)?;
                let working_directory = replayed_working_directory(
                    &summary.path,
                    meta.protocol,
                    WorkspaceMount {
                        host: &host,
                        sandbox: &sandbox,
                    },
                );
                Ok(Response::StoredSession(StoredSession {
                    summary,
                    events,
                    raw,
                    working_directory,
                }))
            }
            Request::ProviderRaw { id } => {
                let summary = self.stored_summary(&id)?;
                let provider_session_id = journal::read_provider_session_id(&summary.path)?
                    .with_context(|| format!("session {id:?} has no stored provider session id"))?;
                let path =
                    find_native_session_file(summary.selection.provider, &provider_session_id)?;
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                Ok(Response::ProviderRaw(crate::protocol::ProviderRaw {
                    provider: summary.selection.provider,
                    text,
                }))
            }
            Request::Shell { id } => Ok(Response::Shell(self.shell(&id)?)),
            Request::TurnAnswer { id, contract } => {
                Ok(Response::Answer(self.turn_answer(&id, contract)?))
            }
            // Flag the shutdown; the connection thread acts on it once this
            // acknowledgement has gone back over the wire.
            Request::QuotaLog => Ok(Response::QuotaLog(self.inner.quota.entries())),
            Request::Shutdown => {
                self.inner.shutdown.store(true, Ordering::Release);
                Ok(Response::Accepted)
            }
        }
    }
}

fn replayed_session_updates(
    path: &Path,
    protocol: crate::event::Protocol,
    workspace: WorkspaceMount<'_>,
) -> Result<Vec<SequencedUpdate>> {
    let events = journal::replay(path, protocol)?;
    let raw = journal::replay_raw(path)?;
    let mut updates = Vec::with_capacity(events.len() + raw.len());
    for event in events {
        // App-server control traffic is carried by the raw view but omitted
        // from the live event list, matching normal Interaction behavior.
        if !matches!(event, crate::event::AgentEvent::Unknown { .. }) {
            push_sequenced(&mut updates, InteractionUpdate::Event(event));
        }
    }
    for line in raw {
        push_sequenced(&mut updates, InteractionUpdate::Raw(line));
    }
    // Last, because it is not a moment in the history but the state the history
    // leaves the Session in: a client that applies these in order ends up
    // standing where the Session was working, not where it was launched.
    if let Some(directory) = replayed_working_directory(path, protocol, workspace) {
        push_sequenced(
            &mut updates,
            InteractionUpdate::WorkingDirectoryChanged(directory),
        );
    }
    Ok(updates)
}

/// The two ends of the Workspace mount a Session ran under: the host directory
/// bound into it, and the path the agent saw it at. Kept together because a
/// reported directory means nothing without both.
#[derive(Clone, Copy)]
struct WorkspaceMount<'a> {
    host: &'a Path,
    sandbox: &'a Path,
}

/// Where a stopped Session was working when it stopped, on the host.
///
/// A Session that moved — a Codex told to work elsewhere, a Claude Code sent
/// into a worktree of its own — says so only on the wire, so the journal is the
/// only place that directory survives the process that moved there. Without
/// this, reopening such a Session names the directory it was *launched* in
/// until the agent happens to mention a directory again, which for a stopped
/// one is never.
///
/// `None` when the journal named no directory, when the one it named is not
/// inside the Workspace mount (see
/// [`crate::interaction::host_working_directory`]), or when it is the root of
/// that mount — which is where a client already stands on opening a Session,
/// and so is not news. A journal that cannot be read is not an error here: the
/// history it holds is what the caller came for, and a missing directory costs
/// only the fallback to that root.
fn replayed_working_directory(
    path: &Path,
    protocol: crate::event::Protocol,
    workspace: WorkspaceMount<'_>,
) -> Option<PathBuf> {
    let reported = journal::last_reported_cwd(path, protocol).ok().flatten()?;
    let host =
        crate::interaction::host_working_directory(&reported, workspace.sandbox, workspace.host)?;
    (host != workspace.host).then_some(host)
}

/// What a Session's replayed history says it last ran on.
///
/// `stored` is the selection the Session was *created* with and never moves, so
/// the model and effort actually in use at the end of the history are whatever
/// the agent reported at each thread start and whatever later switches were
/// recorded. Folding those in order is the only way to say what a resume onto a
/// named selection would be changing.
///
/// A converted Session replays the agent it came from, model reports included,
/// so a report naming a model only another agent declares is skipped whole —
/// the same rule the client applies when it replays the identical history.
fn replayed_selection(updates: &[SequencedUpdate], stored: &Selection) -> Selection {
    let mut selection = stored.clone();
    for sequenced in updates {
        let (model, effort) = match &sequenced.update {
            InteractionUpdate::Event(crate::event::AgentEvent::ThreadStarted {
                model,
                effort,
                ..
            })
            | InteractionUpdate::Event(crate::event::AgentEvent::ModelChanged { model, effort }) => {
                (model, effort)
            }
            _ => continue,
        };
        if let Some(model) = model {
            if !stored.provider.could_run(model) {
                continue;
            }
            selection.model = model.clone();
        }
        if let Some(effort) = effort
            .as_deref()
            .and_then(|effort| crate::agent::Effort::parse(effort).ok())
            .filter(|effort| stored.provider.efforts().contains(effort))
        {
            selection.effort = effort;
        }
    }
    selection
}

fn push_sequenced(updates: &mut Vec<SequencedUpdate>, update: InteractionUpdate) {
    let sequence = updates.len() as u64 + 1;
    updates.push(SequencedUpdate { sequence, update });
}

/// Fail before launching a sandbox when the provider has already discarded
/// the conversation Styra was asked to resume. The journal remains usable for
/// viewing regardless.
fn ensure_native_session_exists(
    provider: crate::agent::Provider,
    provider_session_id: &str,
) -> Result<()> {
    find_native_session_file(provider, provider_session_id).map(|_| ())
}

/// The provider's own on-disk roots for its native, resumable session
/// transcripts.
fn native_session_roots(provider: crate::agent::Provider) -> Result<Vec<PathBuf>> {
    let home = std::env::var_os("HOME").context("HOME is required to locate provider sessions")?;
    let home = PathBuf::from(home);
    Ok(match provider {
        crate::agent::Provider::Codex => vec![
            home.join(".codex/sessions"),
            home.join(".codex/archived_sessions"),
        ],
        crate::agent::Provider::Claude => vec![home.join(".claude/projects")],
        crate::agent::Provider::CodexExec => {
            anyhow::bail!("provider codex-exec does not support resuming sessions")
        }
    })
}

/// Find a provider's native transcript file by id, searching its known
/// storage roots. Sessions are exempt from styra's picture of where within a
/// root they live (Codex nests by date; Claude nests by project), so this
/// walks the whole tree rather than assuming a layout.
fn find_native_session_file(
    provider: crate::agent::Provider,
    provider_session_id: &str,
) -> Result<PathBuf> {
    native_session_roots(provider)?
        .iter()
        .find_map(|root| find_session_file(root, provider_session_id))
        .with_context(|| {
            format!(
                "{} session {:?} does not exist anymore; the Styra transcript is still available read-only",
                provider.as_str(),
                provider_session_id
            )
        })
}

fn find_session_file(root: &Path, provider_session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            if let Some(found) = find_session_file(&path, provider_session_id) {
                return Some(found);
            }
        } else if entry
            .file_name()
            .to_string_lossy()
            .contains(provider_session_id)
        {
            return Some(path);
        }
    }
    None
}

/// The other of Styra's two interactive providers — conversion always flips
/// between exactly these two, since Genta's batch-only `codex-exec` has no
/// resumable native session to convert to or from.
fn other_interactive_provider(provider: crate::agent::Provider) -> Result<crate::agent::Provider> {
    match provider {
        crate::agent::Provider::Codex => Ok(crate::agent::Provider::Claude),
        crate::agent::Provider::Claude => Ok(crate::agent::Provider::Codex),
        crate::agent::Provider::CodexExec => {
            anyhow::bail!("provider codex-exec does not support session conversion")
        }
    }
}

/// The native transcript format Genta's session conversion reads and writes
/// for a given provider.
fn native_session_format(provider: crate::agent::Provider) -> genta::session::SessionFormat {
    match provider {
        crate::agent::Provider::Codex | crate::agent::Provider::CodexExec => {
            genta::session::SessionFormat::Codex
        }
        crate::agent::Provider::Claude => genta::session::SessionFormat::Claude,
    }
}

/// How many of a native transcript's leading messages have a timestamp at or
/// before `cutoff`. Used to turn a UI-selected moment (a [`RawLine::at_ms`],
/// milliseconds since the epoch) into Genta's `keep_messages` count: the two
/// histories are decoded differently — Styra's journal replays its own
/// records, this parses the provider's own transcript — so time is the only
/// axis they agree on. Messages are recorded in order, so this is the length
/// of their leading run at or before the cutoff, not a filtered count; a
/// message with no timestamp, or one Styra cannot parse, is kept rather than
/// used to end that run, since excluding it could silently drop history the
/// operator meant to keep.
fn messages_up_to(
    source: &str,
    format: genta::session::SessionFormat,
    cutoff: u64,
    selected_operator_message: Option<&str>,
) -> Result<usize> {
    let session = genta::session::parse(source, format)?;
    let kept = session
        .messages
        .iter()
        .take_while(|message| {
            message
                .timestamp
                .as_deref()
                .and_then(parse_rfc3339_ms)
                .is_none_or(|at_ms| at_ms <= cutoff)
        })
        .count();
    // The cutoff is a host timestamp; the provider stamps its copy of an
    // operator message only once it has read it, which puts that message just
    // *past* a cutoff resolved from it. Take the next message too when it is
    // the very message the operator selected, so branching through their own
    // turn does not silently branch through the one before it.
    let selected_next = selected_operator_message.is_some_and(|selected| {
        session.messages.get(kept).is_some_and(|message| {
            message.role == genta::session::Role::User && message.text.trim() == selected.trim()
        })
    });
    Ok(kept + usize::from(selected_next))
}

/// Where in a source's history a branch is taken, and how much of it the
/// branch keeps: the whole point of the operation, in one argument.
#[derive(Clone, Copy, Debug)]
struct BranchPoint<'a> {
    /// The host timestamp the operator selected. `None` branches at the end
    /// of the history.
    at_ms: Option<u64>,
    history: crate::protocol::BranchHistory,
    /// The operator message the cutoff lands on, when it lands on one; see
    /// [`crate::journal::operator_message_at`].
    selected_operator_message: Option<&'a str>,
}

/// Build the provider-native half of a branch from the same branch point used
/// for its Styra journal.
fn branch_native_session(
    source: &str,
    from: genta::session::SessionFormat,
    to: genta::session::SessionFormat,
    new_id: &str,
    cwd: &Path,
    point: BranchPoint<'_>,
) -> Result<String> {
    let BranchPoint {
        at_ms,
        history,
        selected_operator_message,
    } = point;
    if history == crate::protocol::BranchHistory::SelectedOnly && at_ms.is_none() {
        anyhow::bail!("branching only the selected entry requires its timestamp");
    }
    let keep_messages = at_ms
        .map(|cutoff| messages_up_to(source, from, cutoff, selected_operator_message))
        .transpose()?;
    let mut portable = genta::session::parse(source, from)?;
    match (history, keep_messages) {
        (crate::protocol::BranchHistory::ThroughSelected, Some(keep)) => {
            portable.messages.truncate(keep);
        }
        (crate::protocol::BranchHistory::SelectedOnly, Some(keep)) => {
            let selected = keep
                .checked_sub(1)
                .and_then(|index| portable.messages.get(index))
                .cloned()
                .context("the selected entry does not correspond to a provider message")?;
            portable.messages = vec![selected];
        }
        (crate::protocol::BranchHistory::ThroughSelected, None) => {}
        (crate::protocol::BranchHistory::SelectedOnly, None) => unreachable!("validated above"),
    }
    portable.id = new_id.to_owned();
    portable.cwd = cwd.to_string_lossy().into_owned();
    Ok(genta::session::serialize(&portable, to))
}

/// Parse a provider's RFC 3339 message timestamp (e.g.
/// `"2026-08-21T10:00:00.000Z"`) into milliseconds since the epoch, so it can
/// be compared against a [`RawLine::at_ms`] cutoff. `None` on anything that
/// does not parse — callers treat that as "keep it" rather than as an error,
/// since one malformed timestamp should not fail an entire branch.
fn parse_rfc3339_ms(timestamp: &str) -> Option<u64> {
    let parsed =
        time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
            .ok()?;
    u64::try_from(parsed.unix_timestamp_nanos() / 1_000_000).ok()
}

/// Where a freshly converted transcript must be written for its destination
/// provider's own resume to find it: Claude scopes sessions under a
/// per-project directory keyed by the encoded working directory; Codex scans
/// its whole sessions tree by id, so a dedicated subdirectory keeps
/// Genta-imported rollouts apart from Codex's own date-nested ones.
fn native_session_destination(
    provider: crate::agent::Provider,
    session_id: &str,
    cwd: &Path,
) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .context("HOME is required to place a converted provider session")?;
    let home = PathBuf::from(home);
    match provider {
        crate::agent::Provider::Claude => Ok(home
            .join(".claude/projects")
            .join(claude_project_directory_name(cwd))
            .join(format!("{session_id}.jsonl"))),
        crate::agent::Provider::Codex => Ok(home
            .join(".codex/sessions/genta-imported")
            .join(format!("rollout-{session_id}.jsonl"))),
        crate::agent::Provider::CodexExec => {
            anyhow::bail!("provider codex-exec cannot store a resumable session")
        }
    }
}

/// Claude Code's own encoding of a project's working directory into its
/// session storage directory name: every character that is not plain ASCII
/// alphanumeric becomes `-`.
fn claude_project_directory_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

/// Resolve and merge the named Driva templates against a `driva.toml` in the
/// operator's workspace, if any (falling back to Driva's built-ins), in the
/// order given: later names take precedence on conflicting settings, mirroring
/// `driva run --template` layering.
fn resolve_templates(workspace: &Path, names: &[String]) -> Result<Option<ResolvedTemplate>> {
    if names.is_empty() {
        return Ok(None);
    }
    let driva_config = workspace_driva_config(workspace)?;
    let mut merged: Option<driva::TemplateConfig> = None;
    for name in names {
        let later = driva_config
            .template(name)
            .with_context(|| format!("unknown driva template {name:?}"))?;
        match &mut merged {
            Some(current) => current.overlay(later),
            None => merged = Some(later),
        }
    }
    ResolvedTemplate::resolve(merged.expect("non-empty names produces a merged template")).map(Some)
}

/// Styra Workspaces are durable host directories, so keep their canonical path
/// meaningful inside the sandbox.
fn workspace_layout(workspace: &Path) -> SandboxLayout {
    SandboxLayout::same_path(workspace)
}

/// Where the directory an interaction works in lands inside its sandbox.
///
/// A Workspace keeps its host path, which is what makes absolute-path tooling
/// and provider session state stable for that project. A worktree cannot: it
/// lives under the store, at a path that names one interaction and means
/// nothing to the agent working there, so it takes the fixed layout instead.
fn launch_layout(
    worktrees: Option<&crate::worktree::Worktrees>,
    workspace: &Path,
) -> SandboxLayout {
    match worktrees {
        Some(_) => SandboxLayout::default(),
        None => workspace_layout(workspace),
    }
}

/// What a checked-out worktree needs beside itself to be a working repository.
fn worktree_mounts(worktrees: Option<&crate::worktree::Worktrees>) -> Vec<MountSpec> {
    worktrees
        .map(|worktrees| vec![worktrees.metadata_mount()])
        .unwrap_or_default()
}

/// The Driva configuration a launch in this Workspace resolves against: the
/// Workspace's own `driva.toml` when it has one, otherwise Driva's built-ins.
fn workspace_driva_config(workspace: &Path) -> Result<driva::Config> {
    let candidate = workspace.join("driva.toml");
    if candidate.exists() {
        driva::Config::load(&candidate)
    } else {
        Ok(driva::Config::default())
    }
}

/// Turn the operator's mount requests into concrete bind mounts.
///
/// The source is canonicalized here rather than at launch, so a path that does
/// not exist (or a typo) is reported while the operator is still editing the
/// policy, instead of surfacing later as a bwrap failure with no context. A
/// destination is optional and defaults to the canonical source, matching
/// Driva's rule for a bind mount that names no destination.
fn resolve_launch_mounts(mounts: &[LaunchMount]) -> Result<Vec<MountSpec>> {
    mounts
        .iter()
        .map(|mount| {
            let source = driva::canonicalize_mount(&mount.source).with_context(|| {
                format!("invalid extra mount source {}", mount.source.display())
            })?;
            let destination = mount.destination.clone().unwrap_or_else(|| source.clone());
            if !destination.is_absolute() {
                anyhow::bail!(
                    "extra mount destination {} must be an absolute path inside the sandbox",
                    destination.display()
                );
            }
            Ok(MountSpec {
                source,
                destination,
                writable: mount.writable,
            })
        })
        .collect()
}

/// The base system a launch in this Workspace runs on.
///
/// The Workspace's `driva.toml` enables static Driva capabilities. A selected
/// template adds the capabilities its command requires.
fn launch_base(workspace: &Path, template: Option<&ResolvedTemplate>) -> Result<driva::BaseConfig> {
    let mut base = workspace_driva_config(workspace)?.base();
    for name in template.iter().flat_map(|value| value.capabilities.iter()) {
        base.include(*name);
    }
    Ok(base)
}

/// The host executables one launch has to run, as read-only mounts.
///
/// Styra launches into Driva's private root, which carries the host system
/// runtime and nothing else — no host home, and no `/` passthrough. An agent
/// installed outside that runtime, and the `tmux` behind the session shell,
/// therefore have to be named as mounts like anything else the sandbox holds.
/// Deriving them here, from the profile that is about to be launched, keeps
/// each grant tied to the command it exists for.
fn tooling_mounts(
    profile: &genta::agent::Profile,
    tmux: &Path,
    base: &driva::BaseConfig,
) -> Result<Vec<MountSpec>> {
    let agent = profile
        .command
        .first()
        .context("the agent profile names no executable")?;
    crate::tooling::executable_mounts(&[PathBuf::from(agent), tmux.to_path_buf()], base)
}

/// Reject a policy that binds two things at the same place inside the sandbox.
///
/// Driva itself refuses this when it validates the request, but that is at
/// spawn time. Checking the captured policy means an extra mount that lands on
/// top of the workspace (or on a template's grant) is reported while it is
/// still being chosen, rather than as a failed launch afterwards.
fn ensure_distinct_destinations(options: &DrivaOptions) -> Result<()> {
    let mut seen = HashSet::new();
    for attributed in &options.mounts {
        let destination = match &attributed.mount {
            driva::Mount::Bind { destination, .. }
            | driva::Mount::Overlay { destination, .. }
            | driva::Mount::Temporary { destination } => destination,
        };
        if !seen.insert(destination.clone()) {
            anyhow::bail!(
                "conflicting mount destination: {} is already bound",
                destination.display()
            );
        }
    }
    Ok(())
}

/// Serve socket connections until the listener fails or the process exits.
pub fn serve(listener: UnixListener, state: ServerState) -> Result<()> {
    for connection in listener.incoming() {
        let stream = connection.context("accepting a Styra client")?;
        let state = state.clone();
        std::thread::Builder::new()
            .name("styra-client".into())
            .spawn(move || {
                if let Err(error) = serve_connection(stream, &state) {
                    eprintln!("styra-server client error: {error:#}");
                }
            })
            .context("starting a Styra client thread")?;
    }
    Ok(())
}

fn serve_connection(mut stream: UnixStream, state: &ServerState) -> Result<()> {
    let wire = match crate::transport::read_message_limited(
        &mut BufReader::new(&stream),
        MAX_REQUEST_BYTES,
    )
    .and_then(|request| state.handle(request))
    {
        Ok(response) => WireResponse::Ok { response },
        Err(error) => WireResponse::Error {
            error: format!("{error:#}"),
        },
    };
    crate::transport::write_message(&mut stream, &wire).context("writing the Styra response")?;
    // The ack is on its way to the client; only now is it safe to exit.
    state.shutdown_if_requested();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;
    use crate::git::Git as _;
    use crate::protocol::{AttributedMount, MountOrigin};
    use driva::{Mount, MountAccess};

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("styra-server-{tag}-{}.sock", std::process::id(),))
    }

    /// A turn is dated once, when it starts. Restating what the interaction is
    /// already doing — a second operator message inside one running turn —
    /// must not restart the clock, or the figure a client shows would fall
    /// back to zero without the work having begun again.
    #[test]
    fn the_activity_clock_restarts_only_when_the_activity_actually_changes() {
        let activity = CurrentActivity::new();
        activity.set(InteractionActivity::Running, None);
        let started = activity.get().since_ms;

        std::thread::sleep(Duration::from_millis(2));
        activity.set(InteractionActivity::Running, None);
        let restated = activity.get();
        assert_eq!(restated.activity, InteractionActivity::Running);
        assert_eq!(restated.since_ms, started);

        std::thread::sleep(Duration::from_millis(2));
        activity.set(
            InteractionActivity::Pending,
            Some(InteractionActivityReason::TurnCompleted),
        );
        let idle = activity.get();
        assert_eq!(idle.activity, InteractionActivity::Pending);
        assert!(idle.since_ms > started, "a real change dates itself");
    }

    /// A background set reported empty answers for the state it was asked
    /// about. A turn that has started since owns the interaction, and the late
    /// answer must not put it back to waiting for input.
    #[test]
    fn an_empty_background_set_does_not_displace_a_turn_that_has_since_started() {
        let activity = CurrentActivity::new();
        activity.set(InteractionActivity::Running, None);

        assert!(!activity.replace_if(
            InteractionActivity::Background,
            InteractionActivity::Pending,
            Some(InteractionActivityReason::BackgroundFinished),
        ));
        assert_eq!(activity.activity(), InteractionActivity::Running);
        assert_eq!(
            activity.get().reason,
            None,
            "a refused transition states nothing about why"
        );

        activity.set(InteractionActivity::Background, None);
        assert!(activity.replace_if(
            InteractionActivity::Background,
            InteractionActivity::Pending,
            Some(InteractionActivityReason::BackgroundFinished),
        ));
        assert_eq!(activity.activity(), InteractionActivity::Pending);
        assert_eq!(
            activity.get().reason,
            Some(InteractionActivityReason::BackgroundFinished)
        );
    }

    /// The agent reports one ending for all of these, so the answer comes from
    /// what the server noticed on the way to it — and the more specific
    /// noticing wins: a turn a window refused never ran to be interrupted.
    #[test]
    fn a_turns_ending_is_read_from_what_the_server_noticed_during_it() {
        let refusal = InteractionActivityReason::RateLimited {
            window: "five_hour".into(),
            resets_at_ms: None,
        };

        assert_eq!(
            turn_end_reason(None, false, None),
            InteractionActivityReason::TurnCompleted
        );
        assert_eq!(
            turn_end_reason(None, true, None),
            InteractionActivityReason::Interrupted
        );
        assert_eq!(
            turn_end_reason(None, false, Some("context window exceeded".into())),
            InteractionActivityReason::Failed {
                message: "context window exceeded".into()
            }
        );
        assert_eq!(
            turn_end_reason(Some(refusal.clone()), true, Some("refused".into())),
            refusal
        );
    }

    /// Claude's refusal does not take its agent away: the CLI reports the
    /// rate limit, the turn ends, and the session sits there idle and still
    /// taking messages — "idle · after an error" once the operator's next ask
    /// fails on the same spent window.
    ///
    /// The quota log hands the window's reset over on time, which is why the
    /// view at the bottom of the screen turns over to `-`. The interaction it
    /// refused must come back with it; being alive is not being unrefused,
    /// and an operator who asked to wait the window out gets nothing at all
    /// if a live-but-refused session is passed over.
    #[test]
    fn a_refused_interaction_still_taking_messages_comes_back_with_its_window() {
        const CLAUDE_REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":
            {"status":"rejected","resetsAt":1788290400,"rateLimitType":"five_hour",
             "overageStatus":"rejected","overageDisabledReason":"out_of_credits",
             "isUsingOverage":false}}"#;

        // The refusal, as the wire stated it and the interaction recorded it.
        let log = crate::quota::QuotaLog::new();
        let refusal = log
            .observe("s-1", crate::agent::Provider::Claude, 1, CLAUDE_REJECTED)
            .rejected
            .expect("the line refused the work");

        // The agent survived it, so the turn merely ended. The operator asked
        // again, the same window failed that turn with an error, and the
        // interaction is left idle — accepting messages nothing will run.
        let activity = CurrentActivity::new();
        activity.set(
            InteractionActivity::Pending,
            Some(turn_end_reason(
                None,
                false,
                Some("Claude usage limit reached".into()),
            )),
        );
        assert_eq!(
            activity.get().reason,
            Some(InteractionActivityReason::Failed {
                message: "Claude usage limit reached".into()
            }),
            "the state the operator is looking at: idle, after an error"
        );

        // The window comes back, and says so: this is the reset the quota
        // view draws as turned over.
        // Well past the grace the log holds a reported reset for.
        let due = log.resets(refusal.resets_at_ms.expect("a reported reset") + 10 * 60 * 1_000);
        assert_eq!(due.len(), 1, "the refused window turned over");
        assert!(reset_releases(&refusal, &due[0]));

        assert_eq!(
            awaiting_window(true, activity.activity(), Some(&refusal)),
            Some(refusal.clone()),
            "a session told to wait the window out is asked again when it resets, \
             whether or not its agent outlived the refusal"
        );
    }

    /// The other side of asking a live session again: an interaction with a
    /// turn under way is being served, whatever refused it earlier, and a
    /// reset arriving mid-turn must not ask the same question beside it.
    #[test]
    fn a_turn_under_way_is_not_waiting_for_a_window() {
        let refusal = crate::protocol::QuotaEvent {
            at_ms: 1,
            session_id: "s-1".into(),
            provider: crate::agent::Provider::Claude,
            window: "five_hour".into(),
            status: crate::protocol::QuotaStatus::Exhausted,
            utilization: None,
            resets_at_ms: Some(1_788_290_400_000),
            detail: None,
        };

        for busy in [
            InteractionActivity::Running,
            InteractionActivity::Background,
        ] {
            assert_eq!(awaiting_window(true, busy, Some(&refusal)), None);
        }
        for waiting in [InteractionActivity::Pending, InteractionActivity::Stopped] {
            assert_eq!(
                awaiting_window(true, waiting, Some(&refusal)),
                Some(refusal.clone())
            );
            // Without the operator's standing answer there is nobody to ask
            // again for, whatever the window does.
            assert_eq!(awaiting_window(false, waiting, Some(&refusal)), None);
        }
    }

    /// A refusal is only interesting until the provider contradicts it, and it
    /// is contradicted silently — by serving. Left standing, it would have the
    /// next reset send a turn this session has long since had an answer to.
    #[test]
    fn a_provider_serving_again_retires_the_refusal_on_record() {
        const CLAUDE_ALLOWED: &str = r#"{"type":"rate_limit_event","rate_limit_info":
            {"status":"allowed","resetsAt":1788290400,"rateLimitType":"five_hour"}}"#;

        let log = crate::quota::QuotaLog::new();
        let observed = log.observe("s-1", crate::agent::Provider::Claude, 2, CLAUDE_ALLOWED);
        assert!(observed.rejected.is_none());
        assert!(
            observed.serving,
            "a reading exists because the provider answered a turn"
        );
    }

    /// Stopping an interaction is not the agent doing something else, so it
    /// says why without claiming the turn moved — and without restarting the
    /// clock that says how long it has been where it is.
    #[test]
    fn a_stop_states_its_reason_without_moving_the_activity() {
        let activity = CurrentActivity::new();
        activity.set(InteractionActivity::Running, None);
        let started = activity.get().since_ms;

        std::thread::sleep(Duration::from_millis(2));
        activity.note_reason(InteractionActivityReason::Paused);

        let state = activity.get();
        assert_eq!(state.activity, InteractionActivity::Running);
        assert_eq!(state.reason, Some(InteractionActivityReason::Paused));
        assert_eq!(state.since_ms, started);
    }

    /// An agent that a window refused, or that the operator stopped, exits as
    /// a consequence a moment later. The exit is not the news — the window is,
    /// and a reset comes back to it — so the reason on record survives the
    /// ending that carries it out.
    #[test]
    fn an_ending_does_not_displace_the_reason_that_caused_it() {
        let refusal = InteractionActivityReason::RateLimited {
            window: "five_hour".into(),
            resets_at_ms: None,
        };
        let activity = CurrentActivity::new();
        activity.set(InteractionActivity::Running, None);
        activity.note_reason(refusal.clone());

        activity.stopped(InteractionActivityReason::Exited { exit_code: Some(1) });

        let state = activity.get();
        assert_eq!(state.activity, InteractionActivity::Stopped);
        assert_eq!(state.reason, Some(refusal));
    }

    /// With nothing better on record, the ending speaks for itself — and a
    /// turn that merely finished is not an explanation for the interaction
    /// going away.
    #[test]
    fn an_ending_explains_itself_when_nothing_else_does() {
        let activity = CurrentActivity::new();
        activity.set(
            InteractionActivity::Pending,
            Some(InteractionActivityReason::TurnCompleted),
        );

        activity.stopped(InteractionActivityReason::Exited { exit_code: Some(0) });

        assert_eq!(
            activity.get().reason,
            Some(InteractionActivityReason::Exited { exit_code: Some(0) })
        );
    }

    /// Stopping is a transition like any other, so it is dated — how long an
    /// interaction has been stopped is as much a question as how long a turn
    /// has been running — and it is the one state that takes no messages.
    #[test]
    fn stopping_dates_itself_and_closes_the_interaction_to_messages() {
        let activity = CurrentActivity::new();
        activity.set(InteractionActivity::Running, None);
        let running_since = activity.get().since_ms;
        assert!(activity.activity().accepting());

        std::thread::sleep(Duration::from_millis(2));
        activity.stopped(InteractionActivityReason::Paused);

        let state = activity.get();
        assert!(state.since_ms > running_since);
        assert!(!state.activity.accepting());
    }

    /// The notification exists to send an operator somewhere they are not. An
    /// interaction that finishes its turn on a client's screen was watched
    /// finishing it, so there is nothing to send anyone to.
    #[test]
    fn going_idle_on_a_clients_screen_is_not_a_notification() {
        let idle = IdleNotice::new(false);

        idle.note_watched();
        idle.became_idle();
        assert!(!idle.unseen());

        // The same interaction, once no client is drawing it any more: this is
        // the case the notification is for.
        *idle.watched.lock().unwrap() = Some(Instant::now() - WATCHED_FOR);
        idle.became_idle();
        assert!(idle.unseen());
    }

    /// A client arriving at an interaction that went idle unwatched is the
    /// operator answering the notification, whether it arrived through a load
    /// or by the interaction becoming the client's current screen.
    #[test]
    fn a_watching_client_acknowledges_a_notification_raised_before_it() {
        let idle = IdleNotice::new(false);
        idle.became_idle();
        assert!(idle.unseen());

        idle.note_watched();

        assert!(!idle.unseen());
    }

    /// Read when the agent stops, and not before: a checkout the operator has
    /// since committed still reads as it did at that moment, and a workspace
    /// the agent dirtied mid-turn does not report until the turn is over.
    #[test]
    fn a_working_tree_is_read_when_the_interaction_stops() {
        let host = temp_path("working-tree-host");
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let git = Arc::new(crate::git::FakeGit::new());
        git.init(&host);
        let tree = WorkingTree::new(git.clone(), host.clone());

        assert!(!tree.uncommitted(), "nothing has been read yet");

        git.set_uncommitted_changes(&host, true);
        assert!(!tree.uncommitted(), "the agent is still working");

        tree.reread();
        assert!(tree.uncommitted());

        git.set_uncommitted_changes(&host, false);
        assert!(tree.uncommitted(), "still what the turn left behind");
        tree.reread();
        assert!(!tree.uncommitted());
    }

    /// An operator arriving at an interaction that left work uncommitted is
    /// usually arriving to commit it. The claim is checked again just after
    /// they get there, so it stops being made once it stops being true — and
    /// no check is scheduled where there is no claim to correct.
    #[test]
    fn focusing_an_interaction_rereads_work_it_left_behind() {
        let host = temp_path("working-tree-focus-host");
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let git = Arc::new(crate::git::FakeGit::new());
        git.init(&host);
        let tree = Arc::new(WorkingTree::new(git.clone(), host.clone()));

        // Nothing left behind, nothing to go and look at again.
        tree.recheck_after(Duration::from_millis(0));
        git.set_uncommitted_changes(&host, true);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!tree.uncommitted(), "no reading was scheduled");

        tree.reread();
        assert!(tree.uncommitted(), "the turn left work behind");

        // The operator arrives and commits it.
        git.set_uncommitted_changes(&host, false);
        tree.recheck_after(Duration::from_millis(0));
        for _ in 0..200 {
            if !tree.uncommitted() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!tree.uncommitted(), "the checkout was read again");
    }

    /// A workspace outside a repository has no answer to give, and the
    /// question failing is not something to report at an operator.
    #[test]
    fn a_workspace_outside_a_repository_reports_nothing() {
        let host = temp_path("working-tree-bare-host");
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let tree = WorkingTree::new(crate::git::FakeGit::shared(), host);

        tree.reread();

        assert!(!tree.uncommitted());
        assert_eq!(tree.checkout_state(), None);
    }

    /// Read at the same moment and on the same terms as the uncommitted work
    /// beside it: nothing until the interaction stops, and then where the
    /// agent actually was.
    #[test]
    fn a_working_tree_records_the_branch_it_stopped_on() {
        let host = temp_path("working-tree-branch-host");
        std::fs::remove_dir_all(&host).ok();
        let git = Arc::new(crate::git::FakeGit::new());
        let repository = git.init(&host);
        let tree = WorkingTree::new(git.clone(), host.clone());

        assert_eq!(tree.checkout_state(), None, "nothing has been read yet");

        tree.reread();

        let state = tree.checkout_state().expect("the workspace is a checkout");
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert_eq!(state.worktree, repository.root);
        // A main checkout is its own repository, which is what makes the
        // linked-worktree question answerable without a second field.
        assert_eq!(state.repository, repository.root);
        assert!(!state.linked());
    }

    /// An interaction given its own linked worktree reports that worktree and
    /// its branch, and names the repository the branch will turn up in — which
    /// the worktree path, parked wherever Styra keeps them, does not say.
    #[test]
    fn a_linked_worktree_names_the_repository_it_branches_from() {
        let host = temp_path("working-tree-linked-host");
        std::fs::remove_dir_all(&host).ok();
        let git = Arc::new(crate::git::FakeGit::new());
        let repository = git.init(&host.join("repository"));
        let worktree = host.join("worktrees").join("styra-session");
        git.create_worktree(&repository.root, "styra/session", &worktree)
            .unwrap();
        let tree = WorkingTree::new(git.clone(), worktree.clone());

        tree.reread();

        let state = tree.checkout_state().expect("the worktree is a checkout");
        assert_eq!(state.branch.as_deref(), Some("styra/session"));
        assert_eq!(state.worktree, worktree.canonicalize().unwrap());
        assert_eq!(state.repository, repository.root);
        assert!(state.linked());
    }

    /// The reading survives the repository becoming unreadable. Where the work
    /// was is still the best answer available, and an operator shown nothing
    /// would read it as "not a checkout" rather than "could not look".
    #[test]
    fn a_checkout_that_goes_away_leaves_the_last_reading_standing() {
        let host = temp_path("working-tree-vanished-host");
        std::fs::remove_dir_all(&host).ok();
        let git = Arc::new(crate::git::FakeGit::new());
        git.init(&host);
        let tree = WorkingTree::new(git.clone(), host.clone());
        tree.reread();
        assert!(tree.checkout_state().is_some());

        std::fs::remove_dir_all(&host).unwrap();
        tree.reread();

        assert_eq!(
            tree.checkout_state()
                .and_then(|state| state.branch)
                .as_deref(),
            Some("main")
        );
    }

    #[test]
    fn workspace_worktrees_are_not_prepared_until_enabled() {
        let store = temp_path("optional-worktrees-store");
        let host = temp_path("optional-worktrees-host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let git = Arc::new(crate::git::FakeGit::new());
        git.init(&host);
        let state = ServerState::with_git(git, store.clone());
        let workspace = crate::workspace::create(&store, &host, None).unwrap();
        let worktrees_path = crate::workspace::worktrees_dir(&store, &workspace.id);

        assert!(state
            .workspace_worktrees(&workspace, false)
            .unwrap()
            .is_none());
        assert!(!worktrees_path.exists());

        assert!(state
            .workspace_worktrees(&workspace, true)
            .unwrap()
            .is_some());
        assert!(worktrees_path.is_dir());

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// A Session on a repository, with no interaction: enough durable state
    /// for the checkout questions, which are decided before an agent starts.
    fn stored_session(
        tag: &str,
    ) -> (
        PathBuf,
        PathBuf,
        ServerState,
        crate::protocol::WorkspaceSummary,
        String,
        PathBuf,
    ) {
        let store = temp_path(&format!("{tag}-store"));
        let host = temp_path(&format!("{tag}-host"));
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let git = Arc::new(crate::git::FakeGit::new());
        git.init(&host);
        let state = ServerState::with_git(git, store.clone());
        let workspace = crate::workspace::create(&store, &host, None).unwrap();
        let selection = Selection::new(crate::agent::Provider::Codex);
        let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
        let (journal, id) =
            Journal::create_in_workspace(&store, &workspace.id, &profile, &selection, None)
                .unwrap();
        let session_path = journal.path().parent().unwrap().to_path_buf();
        drop(journal);
        (store, host, state, workspace, id, session_path)
    }

    /// Completion belongs to the stored Session, so the stored-sessions
    /// picker can mark one this server never ran — there is no live
    /// interaction behind such a row, and demanding one made the whole view's
    /// `C` fail.
    #[test]
    fn a_stored_session_is_completed_without_an_interaction_behind_it() {
        let (store, host, state, _workspace, id, session_path) = stored_session("complete-stored");

        assert!(!journal::read_session_completed(&session_path).unwrap());

        state
            .handle(Request::SetSessionCompleted {
                id: id.clone(),
                completed: true,
            })
            .expect("completing a stored Session needs no live interaction");
        assert!(journal::read_session_completed(&session_path).unwrap());

        // And back: revealing a completed row is only useful if it can be
        // unmarked from the same place.
        state
            .handle(Request::SetSessionCompleted {
                id,
                completed: false,
            })
            .unwrap();
        assert!(!journal::read_session_completed(&session_path).unwrap());

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// Where a Session was working is said on the wire and written down
    /// nowhere else, so reopening one that moved — a Codex told to work
    /// elsewhere, a Claude Code that checked out a worktree of its own — means
    /// reading it back out of the journal. Without that it comes back naming
    /// the directory it was launched in and keeps naming it until the agent
    /// mentions a directory again, which for a stopped Session is never.
    #[test]
    fn a_reopened_session_stands_where_it_was_working() {
        let (store, host, state, _workspace, id, session_path) = stored_session("replayed-cwd");
        let worktree = host.join(".worktrees/audio");
        {
            let mut journal = Journal::open(&session_path).unwrap();
            journal
                .record_agent_line(
                    &serde_json::json!({ "result": { "cwd": worktree } }).to_string(),
                )
                .unwrap();
        }

        let Response::StoredSession(stored) = state
            .handle(Request::StoredSession {
                id: id.clone(),
                raw: false,
            })
            .unwrap()
        else {
            panic!("expected a stored session");
        };
        assert_eq!(stored.working_directory, Some(worktree));

        // The replayed stream ends standing there too, which is what a client
        // attaching to a restored row applies.
        let updates = replayed_session_updates(
            &session_path,
            crate::event::Protocol::CodexAppServer,
            WorkspaceMount {
                host: &host,
                sandbox: &host,
            },
        )
        .unwrap();
        assert!(matches!(
            updates.last().map(|update| &update.update),
            Some(InteractionUpdate::WorkingDirectoryChanged(directory))
                if directory == &host.join(".worktrees/audio")
        ));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// A Session that never left its Workspace directory reports no move: the
    /// client already stands at the root of the mount when it opens one, and
    /// an update restating that is noise in every Session that behaved.
    #[test]
    fn a_session_that_stayed_put_reports_no_directory() {
        let (store, host, state, _workspace, id, session_path) = stored_session("unmoved-cwd");
        {
            let mut journal = Journal::open(&session_path).unwrap();
            journal
                .record_agent_line(&serde_json::json!({ "result": { "cwd": host } }).to_string())
                .unwrap();
            // A directory outside the Workspace mount is not the client's to be
            // shown either, for the reason the live reader will not show one.
            journal
                .record_agent_line(&serde_json::json!({ "result": { "cwd": "/etc" } }).to_string())
                .unwrap();
        }

        let Response::StoredSession(stored) = state
            .handle(Request::StoredSession { id, raw: false })
            .unwrap()
        else {
            panic!("expected a stored session");
        };
        assert_eq!(stored.working_directory, None);
        assert!(!replayed_session_updates(
            &session_path,
            crate::event::Protocol::CodexAppServer,
            WorkspaceMount {
                host: &host,
                sandbox: &host,
            },
        )
        .unwrap()
        .iter()
        .any(|update| matches!(update.update, InteractionUpdate::WorkingDirectoryChanged(_))));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// The Session states which checkout it works in, so a resume asks the
    /// Session rather than the shape of a directory name. Without it a
    /// checkout named after its first prompt is not recognised, and the
    /// Session comes back mounted on the operator's own directory with its
    /// branch and its uncommitted work left behind.
    #[test]
    fn a_session_records_the_checkout_it_was_given() {
        let (store, host, state, workspace, id, session_path) = stored_session("records-checkout");

        // Before anything is checked out there is nothing to report, and
        // asking must not be what brings the parent directory into being.
        assert_eq!(
            state
                .session_checkout(&session_path, &workspace.id, &id)
                .unwrap(),
            None
        );
        assert!(!crate::workspace::worktrees_dir(&store, &workspace.id).exists());

        let worktrees = state
            .workspace_worktrees(&workspace, true)
            .unwrap()
            .unwrap();
        let made = crate::worktree::Checkout::at(
            worktrees
                .checkout(&id, Some("teach-the-picker-to-filter"))
                .unwrap(),
        );
        journal::store_session_checkout(&session_path, &made).unwrap();

        // Both halves are the Session's own record now, branch included —
        // nothing has to reconstruct either from a path.
        let stored = journal::read_session_checkout(&session_path).unwrap();
        assert_eq!(stored, Some(made.clone()));
        assert_eq!(
            made.path.file_name().unwrap(),
            format!("teach-the-picker-to-filter-{id}").as_str()
        );
        assert_eq!(
            made.branch,
            format!("styra/teach-the-picker-to-filter-{id}")
        );
        assert_eq!(
            state
                .session_checkout(&session_path, &workspace.id, &id)
                .unwrap(),
            Some(made.clone())
        );

        // And the record, not the name, is what answers: a checkout renamed to
        // something the id-suffix scan cannot find is still reported as this
        // Session's.
        let renamed = made.path.with_file_name("renamed-by-the-operator");
        std::fs::rename(&made.path, &renamed).unwrap();
        assert_eq!(
            crate::worktree::existing_checkout(
                &crate::workspace::worktrees_dir(&store, &workspace.id),
                &id
            ),
            None
        );
        assert_eq!(
            state
                .session_checkout(&session_path, &workspace.id, &id)
                .unwrap(),
            Some(made)
        );

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// A new Session reached with `n` shares the source Session's checkout,
    /// including its branch and uncommitted file tree, instead of falling
    /// back to the Workspace's original host directory.
    #[test]
    fn a_new_session_can_plan_in_an_existing_sessions_checkout() {
        let (store, host, state, workspace, id, session_path) = stored_session("inherits-checkout");
        let worktrees = state
            .workspace_worktrees(&workspace, true)
            .unwrap()
            .unwrap();
        let checkout = crate::worktree::Checkout::at(
            worktrees
                .checkout(&id, Some("shared-investigation"))
                .unwrap(),
        );
        journal::store_session_checkout(&session_path, &checkout).unwrap();

        let plan = state
            .plan_session(crate::protocol::PlanSession {
                workspace_id: workspace.id,
                selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
                launch: LaunchPolicy::default(),
                create_worktree: false,
                checkout_from: Some(id),
            })
            .unwrap();

        let sandbox = SandboxLayout::default().workspace;
        assert_eq!(plan.working_directory, sandbox);
        assert!(plan.mounts.iter().any(|attributed| matches!(
            &attributed.mount,
            Mount::Bind { source, destination, access: MountAccess::ReadWrite }
                if source == &checkout.path && destination == &sandbox
        )));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    #[test]
    fn a_checkout_cannot_be_inherited_across_workspaces() {
        let (store, host, state, _workspace, id, _session_path) =
            stored_session("rejects-foreign-checkout");
        let other_host = temp_path("rejects-foreign-checkout-other-host");
        std::fs::remove_dir_all(&other_host).ok();
        std::fs::create_dir_all(&other_host).unwrap();
        let other = crate::workspace::create(&store, &other_host, None).unwrap();

        let error = state.inherited_checkout(&other.id, Some(&id)).unwrap_err();
        assert!(
            error.to_string().contains("belongs to Workspace"),
            "{error:#}"
        );

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
        std::fs::remove_dir_all(other_host).ok();
    }

    /// A Session launched before the record existed has a checkout and no
    /// mention of it. It is found the old way — by the id its directory ends
    /// with — and written down on the way past, so the scan answers for that
    /// Session once and never again.
    #[test]
    fn a_session_predating_the_record_is_given_one() {
        let (store, host, state, workspace, id, session_path) = stored_session("backfill-checkout");
        let worktrees = state
            .workspace_worktrees(&workspace, true)
            .unwrap()
            .unwrap();
        let path = worktrees.checkout(&id, Some("fix-the-flaky-test")).unwrap();
        assert_eq!(journal::read_session_checkout(&session_path).unwrap(), None);

        let found = state
            .session_checkout(&session_path, &workspace.id, &id)
            .unwrap();

        assert_eq!(found, Some(crate::worktree::Checkout::at(path.clone())));
        assert_eq!(
            journal::read_session_checkout(&session_path).unwrap(),
            Some(crate::worktree::Checkout::at(path))
        );
        // A different Session in the same Workspace still has none of its own,
        // and is not handed its neighbour's.
        let other = crate::workspace::sessions_dir(&store, &workspace.id).join("1757000000000-9-9");
        assert_eq!(
            state
                .session_checkout(&other, &workspace.id, "1757000000000-9-9")
                .ok()
                .flatten(),
            None
        );

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// A source with no live interaction still learns where its history was
    /// continued: the marker is appended to its stored journal, so its next
    /// replay shows the link.
    #[test]
    fn a_stored_source_records_the_branch_taken_from_it() {
        let store = temp_path("source-branch-marker-store");
        let host = temp_path("source-branch-marker-host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));
        let workspace = crate::workspace::create(&store, &host, None).unwrap();
        let selection = Selection::new(crate::agent::Provider::Codex);
        let profile = crate::agent::resolve_profile(&selection, &workspace_layout(&host)).unwrap();
        let (mut journal, source_id) = Journal::create_in_workspace(
            &store,
            &workspace.id,
            &profile,
            &selection,
            Some("review".into()),
        )
        .unwrap();
        journal.record_user_message("original question").unwrap();
        let source_path = journal.path().parent().unwrap().to_path_buf();
        drop(journal);

        state
            .record_source_branch(&source_id, &source_path, "styra-branch", Some("review"))
            .unwrap();

        assert_eq!(
            journal::replay(&source_path, profile.protocol).unwrap(),
            vec![
                crate::event::AgentEvent::UserMessage {
                    text: "original question".into()
                },
                crate::event::AgentEvent::Branched {
                    direction: crate::event::BranchDirection::To,
                    session: "styra-branch".into(),
                    name: Some("review".into()),
                },
            ]
        );

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// A Workspace opened on a linked worktree names the same directory twice:
    /// as the Workspace and as the checkout root its repository mounts carry.
    /// Planning there has to succeed — the repeat asks for nothing the
    /// Workspace mount does not already provide.
    #[test]
    fn a_workspace_on_a_linked_worktree_plans_without_a_mount_conflict() {
        let store = temp_path("worktree-workspace-store");
        let host = temp_path("worktree-workspace-host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let git = Arc::new(crate::git::FakeGit::new());
        git.init(&host);
        let worktree = host.join(".worktrees/sandbox-base");
        git.create_worktree(&host, "sandbox-base", &worktree)
            .unwrap();

        let state = ServerState::with_git(git.clone(), store.clone());
        // Exactly what a client launching from inside the worktree records:
        // the nearest enclosing checkout is the worktree itself.
        let workspace = crate::workspace::create_with_repository(
            git.as_ref(),
            &store,
            &worktree,
            None,
            Some(&worktree),
        )
        .unwrap();

        let plan = state
            .plan_session(crate::protocol::PlanSession {
                workspace_id: workspace.id,
                selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
                launch: LaunchPolicy::default(),
                create_worktree: false,
                checkout_from: None,
            })
            .unwrap();
        let canonical = worktree.canonicalize().unwrap();
        let at_worktree: Vec<_> = plan
            .mounts
            .iter()
            .filter(|attributed| attributed.mount.destination() == canonical)
            .collect();
        assert!(matches!(
            at_worktree.as_slice(),
            [AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    access: MountAccess::ReadWrite,
                    ..
                },
            }]
        ));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// With worktrees enabled the interaction does not get the operator's
    /// checkout at all: its sandbox workspace is a checkout of its own, and the
    /// only other thing Styra adds is the Git metadata that makes that checkout
    /// a repository.
    #[test]
    fn an_enabled_workspace_launches_in_its_own_checkout() {
        let store = temp_path("worktree-launch-store");
        let host = temp_path("worktree-launch-host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();
        let git = Arc::new(crate::git::FakeGit::new());
        git.init(&host);

        let state = ServerState::with_git(git, store.clone());
        let workspace = crate::workspace::create(&store, &host, None).unwrap();

        let plan = state
            .plan_session(crate::protocol::PlanSession {
                workspace_id: workspace.id.clone(),
                selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
                launch: LaunchPolicy::default(),
                create_worktree: true,
                checkout_from: None,
            })
            .unwrap();

        let sandbox = SandboxLayout::default().workspace;
        assert_eq!(plan.working_directory, sandbox);
        let at_workspace: Vec<_> = plan
            .mounts
            .iter()
            .filter(|attributed| attributed.mount.destination() == sandbox)
            .collect();
        assert!(matches!(
            at_workspace.as_slice(),
            [AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source,
                    access: MountAccess::ReadWrite,
                    ..
                },
            }] if *source == crate::workspace::worktrees_dir(&store, &workspace.id)
                .join(PENDING_SESSION_ID)
        ));
        let canonical = host.canonicalize().unwrap();
        assert!(plan.mounts.iter().any(|attributed| matches!(
            &attributed.mount,
            Mount::Bind { source, destination, access: MountAccess::ReadWrite }
                if *source == canonical.join(".git") && destination == source
        )));
        // The directory the operator works in is left alone, which is the
        // whole point of checking out somewhere else.
        assert!(!plan.mounts.iter().any(|attributed| matches!(
            &attributed.mount,
            Mount::Bind { source, access: MountAccess::ReadWrite, .. } if *source == canonical
        )));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// The workspace mount is the one grant no policy row can name, so the
    /// policy answer for it has to reach the plan — and the plan is what the
    /// operator is shown before their first message.
    #[test]
    fn a_read_only_workspace_policy_plans_the_workspace_mount_read_only() {
        let store = temp_path("read-only-workspace-store");
        let host = temp_path("read-only-workspace-host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&host).ok();
        std::fs::create_dir_all(&host).unwrap();

        let state = ServerState::new(store.clone(), store.with_extension("sock"));
        let workspace = crate::workspace::create(&store, &host, None).unwrap();
        let canonical = host.canonicalize().unwrap();
        let access_at_workspace = |launch: LaunchPolicy| {
            let plan = state
                .plan_session(crate::protocol::PlanSession {
                    workspace_id: workspace.id.clone(),
                    selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
                    launch,
                    create_worktree: false,
                    checkout_from: None,
                })
                .unwrap();
            plan.mounts
                .iter()
                .find_map(|attributed| match &attributed.mount {
                    Mount::Bind {
                        destination,
                        access,
                        ..
                    } if *destination == canonical => Some(*access),
                    _ => None,
                })
                .expect("the workspace is always mounted")
        };

        // Writable is what a launch does when no layer says otherwise.
        assert_eq!(
            access_at_workspace(LaunchPolicy::default()),
            MountAccess::ReadWrite
        );
        assert_eq!(
            access_at_workspace(LaunchPolicy {
                writable_workspace: Some(false),
                ..LaunchPolicy::default()
            }),
            MountAccess::ReadOnly
        );

        // And the Workspace's standing answer applies to a launch that asks
        // for nothing, because the two are merged before the spec is built.
        crate::workspace::change_launch(
            &store,
            &workspace.id,
            crate::protocol::WorkspaceLaunchChange::SetWritableWorkspace(Some(false)),
        )
        .unwrap();
        assert_eq!(
            access_at_workspace(LaunchPolicy::default()),
            MountAccess::ReadOnly
        );

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// Which interactions a reset releases: the providers are separate
    /// subscriptions and each reports several windows, so waking the wrong
    /// sessions would send held-back work into a limit that is still refusing
    /// it.
    #[test]
    fn a_reset_releases_only_the_work_its_own_window_refused() {
        let refusal = |provider, window: &str| crate::protocol::QuotaEvent {
            at_ms: 1_000,
            session_id: "s-1".into(),
            provider,
            window: window.into(),
            status: crate::protocol::QuotaStatus::Exhausted,
            utilization: None,
            resets_at_ms: Some(2_000),
            detail: None,
        };
        let reset = crate::quota::WindowReset {
            provider: crate::agent::Provider::Claude,
            window: "five_hour".into(),
            at_ms: 2_000,
        };

        assert!(reset_releases(
            &refusal(crate::agent::Provider::Claude, "five_hour"),
            &reset
        ));
        // The same account's other window is still full.
        assert!(!reset_releases(
            &refusal(crate::agent::Provider::Claude, "seven_day"),
            &reset
        ));
        // And the other subscription's window of the same name is not this
        // one at all.
        assert!(!reset_releases(
            &refusal(crate::agent::Provider::Codex, "five_hour"),
            &reset
        ));
    }

    #[test]
    fn socket_health_reports_the_service() {
        let socket = temp_path("health");
        std::fs::remove_file(&socket).ok();
        let listener = UnixListener::bind(&socket).unwrap();
        let store = socket.with_extension("store");
        let state = ServerState::new(store.clone(), socket.clone());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, &state).unwrap();
        });

        let health = Client::new(&socket).health().unwrap();
        assert_eq!(health.service, "styra");

        server.join().unwrap();
        std::fs::remove_file(socket).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// The standalone transport answers the same requests as the socket one,
    /// with no listener, no daemon and no socket file anywhere.
    #[test]
    fn an_in_process_client_serves_requests_without_a_socket() {
        let root = std::env::temp_dir().join(format!("styra-in-process-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let store = root.join("store");
        let host = root.join("work");
        std::fs::create_dir_all(&host).unwrap();

        let lock_path = root.join("test.lock");
        let lock = std::fs::File::create(lock_path).unwrap();
        let client = crate::daemon::in_process_client(ServerState::in_process(store.clone(), lock));
        assert!(client.socket_path().is_none());
        assert_eq!(client.health().unwrap().service, "styra");

        let workspace = client
            .create_workspace(&CreateWorkspace {
                host_path: host.clone(),
                name: Some("standalone".into()),
                git_repository: None,
            })
            .unwrap();
        assert_eq!(workspace.host_path, host);
        assert_eq!(
            client
                .list_workspaces()
                .unwrap()
                .iter()
                .map(|workspace| workspace.id.clone())
                .collect::<Vec<_>>(),
            vec![workspace.id]
        );
        // An error comes back as an error, not as a wire-encoded string.
        assert!(client.workspace("styra-nothing").is_err());

        let clone = client.clone();
        client.shutdown().unwrap();
        assert!(clone
            .health()
            .unwrap_err()
            .to_string()
            .contains("shut down"));
        assert!(client
            .shutdown()
            .unwrap_err()
            .to_string()
            .contains("already shut down"));

        std::fs::remove_dir_all(root).ok();
    }

    /// An operator's mount request is resolved against the host now, so a path
    /// that is not there is reported while the policy is being chosen.
    #[test]
    fn extra_mounts_resolve_their_source_and_default_the_destination_to_it() {
        let root =
            std::env::temp_dir().join(format!("styra-server-extra-mounts-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("data")).unwrap();

        let resolved = resolve_launch_mounts(&[
            LaunchMount {
                source: root.join("data"),
                destination: None,
                writable: true,
            },
            LaunchMount {
                source: root.join("data"),
                destination: Some(PathBuf::from("/mnt/data")),
                writable: false,
            },
        ])
        .unwrap();

        assert_eq!(
            resolved[0].source,
            root.join("data").canonicalize().unwrap()
        );
        assert_eq!(resolved[0].destination, resolved[0].source);
        assert!(resolved[0].writable);
        assert_eq!(resolved[1].destination, PathBuf::from("/mnt/data"));
        assert!(!resolved[1].writable);

        let missing = resolve_launch_mounts(&[LaunchMount {
            source: root.join("absent"),
            destination: None,
            writable: false,
        }])
        .unwrap_err();
        assert!(
            missing.to_string().contains("invalid extra mount source"),
            "{missing}"
        );

        let relative = resolve_launch_mounts(&[LaunchMount {
            source: root.join("data"),
            destination: Some(PathBuf::from("mnt/data")),
            writable: false,
        }])
        .unwrap_err();
        assert!(relative.to_string().contains("absolute"), "{relative}");

        std::fs::remove_dir_all(root).ok();
    }

    /// Two mounts landing on the same place inside the sandbox is a policy
    /// Driva refuses, so it is caught while planning rather than at spawn.
    #[test]
    fn a_policy_binding_two_things_at_one_destination_is_rejected() {
        let options = DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            base: Vec::new(),
            mounts: vec![
                AttributedMount {
                    origin: MountOrigin::Workspace,
                    mount: driva::Mount::Bind {
                        source: PathBuf::from("/srv/one"),
                        destination: PathBuf::from("/tmp/styra/workspace"),
                        access: driva::MountAccess::ReadWrite,
                    },
                },
                AttributedMount {
                    origin: MountOrigin::Operator,
                    mount: driva::Mount::Bind {
                        source: PathBuf::from("/srv/two"),
                        destination: PathBuf::from("/tmp/styra/workspace"),
                        access: driva::MountAccess::ReadOnly,
                    },
                },
            ],
            ..Default::default()
        };
        let error = ensure_distinct_destinations(&options).unwrap_err();
        assert!(
            error.to_string().contains("conflicting mount destination"),
            "{error}"
        );

        let mut fine = options;
        fine.mounts.pop();
        assert!(ensure_distinct_destinations(&fine).is_ok());
    }

    #[test]
    fn stored_ids_are_resolved_from_the_store_listing() {
        let store =
            std::env::temp_dir().join(format!("styra-server-id-test-{}", std::process::id()));
        std::fs::remove_dir_all(&store).ok();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));
        let error = state.stored_summary("../../etc").unwrap_err();
        assert!(error.to_string().contains("was not found"));
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn broker_is_staged_in_the_sandbox_control_mount() {
        let root =
            std::env::temp_dir().join(format!("styra-server-broker-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let socket = root.join("styra.sock");
        let state = ServerState::new(root.join("store"), socket);

        let broker = state
            .prepare_broker("session-1", PathBuf::from("/usr/bin/tmux"))
            .unwrap();

        assert_eq!(
            broker.executable,
            PathBuf::from("/tmp/styra/control/styra-broker")
        );
        let staged = broker.control.source.join("styra-broker");
        let metadata = std::fs::metadata(&staged).unwrap();
        assert!(metadata.len() > 0);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn describing_a_broker_names_the_same_mount_without_staging_anything() {
        let root =
            std::env::temp_dir().join(format!("styra-server-plan-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let state = ServerState::new(root.join("store"), root.join("styra.sock"));

        let planned = state.describe_broker(PENDING_SESSION_ID, PathBuf::from("/usr/bin/tmux"));
        let real = state.describe_broker("session-1", PathBuf::from("/usr/bin/tmux"));

        assert_eq!(planned.control.destination, real.control.destination);
        assert_eq!(planned.executable, real.executable);
        assert_eq!(
            planned.control.source,
            root.join("sandboxes").join(PENDING_SESSION_ID)
        );
        assert!(!planned.control.source.exists());
        assert!(!root.join("sandboxes").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_legacy_session_without_a_provider_id_is_viewable_but_not_resumable() {
        let store =
            std::env::temp_dir().join(format!("styra-server-legacy-test-{}", std::process::id()));
        let host = store.with_extension("host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));
        let workspace = crate::workspace::create(&store, &host, Some("legacy".into())).unwrap();
        let session_dir =
            crate::workspace::sessions_dir(&store, &workspace.id).join("0000000000001-1-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("journal.jsonl"),
            concat!(
                "{\"source\":\"user\",\"at_ms\":1,\"text\":\"old question\"}\n",
                "{\"source\":\"agent\",\"at_ms\":2,\"raw\":\"{\\\"method\\\":\\\"item/completed\\\",\\\"params\\\":{\\\"item\\\":{\\\"type\\\":\\\"agentMessage\\\",\\\"id\\\":\\\"m\\\",\\\"text\\\":\\\"old answer\\\"}}}\"}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            session_dir.join("session.json"),
            serde_json::json!({
                "workspace_id": workspace.id,
                "selection": {
                    "provider": "codex",
                    "model": "gpt-5.6-sol",
                    "effort": "high"
                },
                "protocol": "codex-app-server"
            })
            .to_string(),
        )
        .unwrap();

        assert!(matches!(
            state
                .handle(Request::StoredSession {
                    id: "0000000000001-1-1".into(),
                    raw: true,
                })
                .unwrap(),
            Response::StoredSession(_)
        ));
        // The same session asked for without raw lines: the decoded events are
        // still there, the verbatim wire log is not shipped at all.
        let Response::StoredSession(events_only) = state
            .handle(Request::StoredSession {
                id: "0000000000001-1-1".into(),
                raw: false,
            })
            .unwrap()
        else {
            panic!("expected a stored session");
        };
        assert_eq!(events_only.events.len(), 2);
        assert!(events_only.raw.is_empty());
        let replayed = replayed_session_updates(
            &session_dir,
            crate::event::Protocol::CodexAppServer,
            WorkspaceMount {
                host: &host,
                sandbox: &host,
            },
        )
        .unwrap();
        assert_eq!(
            replayed
                .iter()
                .map(|update| update.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(matches!(
            &replayed[0].update,
            InteractionUpdate::Event(
                crate::event::AgentEvent::UserMessage { text }
            ) if text == "old question"
        ));
        assert!(matches!(
            &replayed[1].update,
            InteractionUpdate::Event(
                crate::event::AgentEvent::AgentMessage { text }
            ) if text == "old answer"
        ));
        assert!(replayed[2..]
            .iter()
            .all(|update| matches!(update.update, InteractionUpdate::Raw(_))));
        let error = state
            .resume_session(ResumeSession {
                id: "0000000000001-1-1".into(),
                launch: LaunchPolicy::default(),
                selection: None,
            })
            .unwrap_err();
        assert!(error.to_string().contains("can be viewed but not resumed"));

        // A resume may name the model and effort to revive on, but not another
        // agent: the stored transcript is the agent's own, so it is refused
        // before anything is launched rather than resumed under the stored
        // agent as though the choice had been honoured.
        let error = state
            .resume_session(ResumeSession {
                id: "0000000000001-1-1".into(),
                launch: LaunchPolicy::default(),
                selection: Some(Selection::parse("claude:claude-opus-5/high").unwrap()),
            })
            .unwrap_err();
        assert!(
            error.to_string().contains("cannot be resumed as claude"),
            "{error:#}"
        );
        assert!(error.to_string().contains("convert"), "{error:#}");
        // The same resume naming this agent's own catalog gets as far as the
        // native transcript this legacy journal does not have.
        let error = state
            .resume_session(ResumeSession {
                id: "0000000000001-1-1".into(),
                launch: LaunchPolicy::default(),
                selection: Some(Selection::parse("codex:gpt-5.6-luna/low").unwrap()),
            })
            .unwrap_err();
        assert!(
            error.to_string().contains("can be viewed but not resumed"),
            "{error:#}"
        );

        // Conversion needs the provider's native transcript just as resume
        // does. A legacy Styra-only journal must fail without making a
        // partially-created sibling session appear in the picker.
        let error = state
            .convert_session_provider("0000000000001-1-1")
            .unwrap_err();
        assert!(
            error.to_string().contains("converting session"),
            "{error:#}"
        );
        assert!(
            format!("{error:#}").contains("no stored provider session id"),
            "{error:#}"
        );
        assert_eq!(
            crate::workspace::get(&store, &workspace.id)
                .unwrap()
                .session_count,
            1
        );

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    #[test]
    fn native_session_lookup_detects_removal() {
        let root = std::env::temp_dir().join(format!("styra-native-test-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/rollout-provider-7.jsonl"), "").unwrap();
        assert!(find_session_file(&root, "provider-7").is_some());
        assert!(find_session_file(&root, "provider-gone").is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn conversion_always_flips_between_styras_two_interactive_providers() {
        assert_eq!(
            other_interactive_provider(crate::agent::Provider::Codex).unwrap(),
            crate::agent::Provider::Claude
        );
        assert_eq!(
            other_interactive_provider(crate::agent::Provider::Claude).unwrap(),
            crate::agent::Provider::Codex
        );
        assert!(other_interactive_provider(crate::agent::Provider::CodexExec).is_err());
    }

    #[test]
    fn native_session_format_matches_each_providers_own_transcript() {
        assert_eq!(
            native_session_format(crate::agent::Provider::Codex),
            genta::session::SessionFormat::Codex
        );
        assert_eq!(
            native_session_format(crate::agent::Provider::CodexExec),
            genta::session::SessionFormat::Codex
        );
        assert_eq!(
            native_session_format(crate::agent::Provider::Claude),
            genta::session::SessionFormat::Claude
        );
    }

    #[test]
    fn claude_project_directory_names_replace_every_non_alphanumeric_character() {
        assert_eq!(
            claude_project_directory_name(Path::new("/home/op/.dotfiles/project")),
            "-home-op--dotfiles-project"
        );
    }

    #[test]
    fn a_converted_destination_is_scoped_by_provider() {
        let claude = native_session_destination(
            crate::agent::Provider::Claude,
            "new-id",
            Path::new("/home/op/project"),
        )
        .unwrap();
        assert!(claude.ends_with("-home-op-project/new-id.jsonl"));

        let codex =
            native_session_destination(crate::agent::Provider::Codex, "new-id", Path::new("/x"))
                .unwrap();
        assert!(codex.ends_with("genta-imported/rollout-new-id.jsonl"));

        assert!(native_session_destination(
            crate::agent::Provider::CodexExec,
            "new-id",
            Path::new("/x")
        )
        .is_err());
    }

    /// What a resume is *changing* is measured against the end of the
    /// Session's history, not against the selection it was created with — and
    /// the history of a converted Session includes the old agent's own model
    /// reports, which say nothing about this one.
    #[test]
    fn the_replayed_selection_is_the_end_of_the_history_and_ignores_a_foreign_agents_reports() {
        let stored = Selection::parse("claude:claude-opus-5/high").unwrap();
        let events = |events: Vec<crate::event::AgentEvent>| {
            events
                .into_iter()
                .enumerate()
                .map(|(index, event)| SequencedUpdate {
                    sequence: index as u64 + 1,
                    update: InteractionUpdate::Event(event),
                })
                .collect::<Vec<_>>()
        };

        // Nothing reported: the Session is still on what it was created with.
        assert_eq!(replayed_selection(&[], &stored), stored);

        // Its own agent's reports carry, last one winning.
        let own = events(vec![
            crate::event::AgentEvent::ThreadStarted {
                thread_id: "t".into(),
                model: Some("claude-sonnet-5".into()),
                effort: Some("max".into()),
            },
            crate::event::AgentEvent::ModelChanged {
                model: Some("claude-opus-4-8".into()),
                effort: None,
            },
        ]);
        assert_eq!(
            replayed_selection(&own, &stored),
            Selection::parse("claude:claude-opus-4-8/max").unwrap()
        );

        // The copied history of a conversion does not: its model is a model
        // this agent cannot be running, and its effort describes that same
        // foreign thread.
        let converted = events(vec![crate::event::AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: Some("gpt-5.6-terra".into()),
            effort: Some("medium".into()),
        }]);
        assert_eq!(replayed_selection(&converted, &stored), stored);
    }

    #[test]
    fn rfc3339_timestamps_parse_to_milliseconds_since_the_epoch() {
        assert_eq!(
            parse_rfc3339_ms("2026-08-21T10:00:00.000Z"),
            Some(1787306400000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-08-21T10:00:00.500Z"),
            Some(1787306400500)
        );
        assert_eq!(parse_rfc3339_ms("not a timestamp"), None);
    }

    #[test]
    fn messages_up_to_keeps_the_leading_run_at_or_before_the_cutoff() {
        let claude = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"first"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:02.000Z","message":{"role":"user","content":"third"}}"#,
            "\n",
        );
        let cutoff = parse_rfc3339_ms("2026-08-21T10:00:01.000Z").unwrap();
        assert_eq!(
            messages_up_to(claude, genta::session::SessionFormat::Claude, cutoff, None).unwrap(),
            2
        );
        assert_eq!(
            messages_up_to(claude, genta::session::SessionFormat::Claude, 0, None).unwrap(),
            0
        );
        assert_eq!(
            messages_up_to(
                claude,
                genta::session::SessionFormat::Claude,
                u64::MAX,
                None
            )
            .unwrap(),
            3
        );
    }

    /// The host writes an operator message to the agent before the provider
    /// stamps its own copy, so a cutoff resolved from that message lands just
    /// before it in the native transcript. Naming the selected text keeps it:
    /// branching through one's own turn must not branch through the previous
    /// one.
    #[test]
    fn a_cutoff_on_an_operator_message_keeps_the_message_it_names() {
        let claude = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"first"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:02.500Z","message":{"role":"user","content":"try again"}}"#,
            "\n",
        );
        // The host sent "try again" half a second before Claude stamped it.
        let cutoff = parse_rfc3339_ms("2026-08-21T10:00:02.000Z").unwrap();

        assert_eq!(
            messages_up_to(claude, genta::session::SessionFormat::Claude, cutoff, None).unwrap(),
            2
        );
        assert_eq!(
            messages_up_to(
                claude,
                genta::session::SessionFormat::Claude,
                cutoff,
                Some("try again")
            )
            .unwrap(),
            3
        );
        // A different message at the cutoff is not the one named, so nothing
        // beyond the timestamps is taken.
        assert_eq!(
            messages_up_to(
                claude,
                genta::session::SessionFormat::Claude,
                cutoff,
                Some("something else")
            )
            .unwrap(),
            2
        );
    }

    /// The same skew decides which single message `selected_only` takes: named
    /// or not, it must be the operator's own turn rather than the reply before
    /// it.
    #[test]
    fn a_selected_only_branch_on_an_operator_message_takes_that_message() {
        let claude = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"first"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:02.500Z","message":{"role":"user","content":"try again"}}"#,
            "\n",
        );
        let cutoff = parse_rfc3339_ms("2026-08-21T10:00:02.000Z").unwrap();
        let branched = branch_native_session(
            claude,
            genta::session::SessionFormat::Claude,
            genta::session::SessionFormat::Claude,
            "new-id",
            Path::new("/repo"),
            BranchPoint {
                at_ms: Some(cutoff),
                history: crate::protocol::BranchHistory::SelectedOnly,
                selected_operator_message: Some("try again"),
            },
        )
        .unwrap();
        let parsed =
            genta::session::parse(&branched, genta::session::SessionFormat::Claude).unwrap();

        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].text, "try again");
    }

    #[test]
    fn a_selected_only_native_branch_contains_just_the_selected_message() {
        let claude = concat!(
            r#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"first"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"selected"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","isSidechain":false,"cwd":"/repo","sessionId":"id","timestamp":"2026-08-21T10:00:02.000Z","message":{"role":"user","content":"later"}}"#,
            "\n",
        );
        let cutoff = parse_rfc3339_ms("2026-08-21T10:00:01.000Z").unwrap();
        let branched = branch_native_session(
            claude,
            genta::session::SessionFormat::Claude,
            genta::session::SessionFormat::Codex,
            "new-id",
            Path::new("/new/repo"),
            BranchPoint {
                at_ms: Some(cutoff),
                history: crate::protocol::BranchHistory::SelectedOnly,
                selected_operator_message: None,
            },
        )
        .unwrap();
        let parsed =
            genta::session::parse(&branched, genta::session::SessionFormat::Codex).unwrap();

        assert_eq!(parsed.id, "new-id");
        assert_eq!(parsed.cwd, "/new/repo");
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].text, "selected");
    }

    /// The standing policy is stored with the Workspace, not with the client
    /// that set it, so every client launching there reads the same one back.
    #[test]
    fn a_workspace_launch_policy_is_stored_and_reported_back() {
        let store = std::env::temp_dir().join(format!(
            "styra-server-workspace-launch-{}",
            std::process::id()
        ));
        let host = store.with_extension("host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));

        let created = match state
            .handle(Request::CreateWorkspace(CreateWorkspace {
                host_path: host.clone(),
                name: None,
                git_repository: None,
            }))
            .unwrap()
        {
            Response::WorkspaceCreated(workspace) => workspace,
            other => panic!("unexpected response: {other:?}"),
        };
        assert!(created.launch.is_empty());

        let launch = LaunchPolicy {
            network: Some(true),
            writable_workspace: None,
            templates: vec!["rust".into()],
            mounts: vec![LaunchMount {
                source: PathBuf::from("/srv/corpus"),
                destination: None,
                writable: false,
            }],
            ignore_workspace: false,
        };
        let updated = match state
            .handle(Request::ChangeWorkspaceLaunch {
                workspace_id: created.id.clone(),
                change: crate::protocol::WorkspaceLaunchChange::Replace(launch.clone()),
            })
            .unwrap()
        {
            Response::WorkspaceLaunchUpdated(workspace) => workspace,
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(updated, launch);

        match state
            .handle(Request::Workspace {
                id: created.id.clone(),
            })
            .unwrap()
        {
            Response::Workspace(workspace) => assert_eq!(workspace.launch, launch),
            other => panic!("unexpected response: {other:?}"),
        }

        // A policy cannot be stored for a Workspace that is not there.
        assert!(state
            .handle(Request::ChangeWorkspaceLaunch {
                workspace_id: "no-such-workspace".into(),
                change: crate::protocol::WorkspaceLaunchChange::Replace(launch),
            })
            .is_err());

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }

    #[test]
    fn workspace_api_creates_lists_and_scopes_sessions() {
        let store = std::env::temp_dir().join(format!(
            "styra-server-workspace-test-{}",
            std::process::id()
        ));
        let host = store.with_extension("host");
        std::fs::remove_dir_all(&store).ok();
        std::fs::create_dir_all(&host).unwrap();
        let state = ServerState::new(store.clone(), store.with_extension("sock"));

        let created = match state
            .handle(Request::CreateWorkspace(CreateWorkspace {
                host_path: host.clone(),
                name: Some("api test".into()),
                git_repository: None,
            }))
            .unwrap()
        {
            Response::WorkspaceCreated(workspace) => workspace,
            other => panic!("unexpected response: {other:?}"),
        };
        assert_eq!(created.name.as_deref(), Some("api test"));
        assert!(matches!(
            state.handle(Request::ListWorkspaces).unwrap(),
            Response::Workspaces(workspaces) if workspaces == vec![created.clone()]
        ));
        assert!(matches!(
            state
                .handle(Request::ListSessions {
                    workspace_id: created.id,
                })
                .unwrap(),
            Response::StoredSessions(sessions) if sessions.is_empty()
        ));

        std::fs::remove_dir_all(store).ok();
        std::fs::remove_dir_all(host).ok();
    }
}
