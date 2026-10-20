//! What a turn changed in the checkout, read from the snapshots the server
//! took as it started and ended, for the side diff panel's turn diff (see
//! [`crate::turn_diff`]).
//!
//! The agent's own file-change events say what it reported writing; the
//! snapshots say what actually changed — a file a shell command rewrote, a
//! formatter's pass, the operator's edit while the turn ran. Where a turn has
//! both snapshots, the turn diff is theirs. A turn still running is shown
//! from the most recent snapshot to the checkout as it is now, read again
//! every [`REPOLL`] while it runs. Where there are no snapshots — outside a
//! Git checkout, or from before they were taken — it is the events'.
//!
//! The diff is the server's to compute, so it is fetched once the cursor has
//! settled on a turn, as the branch log is (see [`crate::branch_log`]).

use std::time::{Duration, Instant};

use styra_protocol::TurnDiff;
use styra_server::Client;

/// How long the selection must rest on a turn before its snapshots are read.
const SETTLE: Duration = Duration::from_millis(120);

/// How often a turn that has not ended yet is asked about again, so its diff
/// follows the checkout while it runs and settles once it ends.
const REPOLL: Duration = Duration::from_secs(1);

pub enum Contents {
    Loading,
    /// The server's answer: `None` for a turn without snapshots.
    Ready(Option<TurnDiff>),
    Failed,
}

pub struct TurnSnapshot {
    session_id: String,
    turn: u64,
    since: Instant,
    contents: Contents,
}

impl TurnSnapshot {
    fn new(session_id: String, turn: u64) -> Self {
        Self {
            session_id,
            turn,
            since: Instant::now(),
            contents: Contents::Loading,
        }
    }

    fn is_for(&self, session_id: &str, turn: u64) -> bool {
        self.session_id == session_id && self.turn == turn
    }

    /// The turn's snapshot diff, once one has been read.
    fn diff(&self) -> Option<&TurnDiff> {
        match &self.contents {
            Contents::Ready(diff) => diff.as_ref(),
            _ => None,
        }
    }

    /// Ask the server once the cursor has settled, and again while the turn
    /// is still running — or, without snapshots yet, may still get them.
    /// Whether anything on screen changed.
    fn load(&mut self, client: &Client, may_still_end: bool) -> Result<bool, String> {
        let due = match &self.contents {
            Contents::Loading => self.since.elapsed() >= SETTLE,
            Contents::Ready(None) => may_still_end && self.since.elapsed() >= REPOLL,
            Contents::Ready(Some(diff)) => !diff.ended && self.since.elapsed() >= REPOLL,
            Contents::Failed => false,
        };
        if !due {
            return Ok(false);
        }
        self.since = Instant::now();
        match client.snapshot_turn_diff(&self.session_id, self.turn) {
            Ok(diff) => {
                let changed = !matches!(&self.contents, Contents::Ready(shown) if *shown == diff);
                self.contents = Contents::Ready(diff);
                Ok(changed)
            }
            Err(error) => {
                self.contents = Contents::Failed;
                Err(format!(
                    "could not read the snapshots of turn {}: {error:#}",
                    self.turn
                ))
            }
        }
    }
}

impl crate::app::App {
    /// The turn whose snapshots are wanted: the selected one, while the side
    /// diff panel shows its turn diff.
    fn snapshot_turn(&self) -> Option<u64> {
        self.turn_diff_shown()
            .then(|| self.selected_turn())
            .flatten()
    }

    /// The selected turn's diff from its snapshots, if it has been read and
    /// there is one.
    pub(crate) fn turn_snapshot_diff(&self) -> Option<&TurnDiff> {
        let turn = self.selected_turn()?;
        self.turn_snapshot
            .as_ref()
            .filter(|snapshot| snapshot.is_for(&self.session_id, turn))?
            .diff()
    }

    /// How many turns the session has had: the operator messages listed.
    fn turns(&self) -> u64 {
        self.timeline
            .entries
            .iter()
            .filter(|entry| is_operator_message(entry.event()))
            .count() as u64
    }
}

fn is_operator_message(event: &styra_protocol::event::AgentEvent) -> bool {
    matches!(event, styra_protocol::event::AgentEvent::UserMessage { .. })
}

/// Keep `app.turn_snapshot` on the turn the turn diff is about: start a
/// fresh read when it moves to another, and drop it once no turn diff is
/// shown. Whether anything on screen changed.
pub fn sync(app: &mut crate::app::App, client: &Client) -> bool {
    let Some(turn) = app.snapshot_turn().filter(|_| !app.session_id.is_empty()) else {
        app.turn_snapshot = None;
        return false;
    };
    if !app
        .turn_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.is_for(&app.session_id, turn))
    {
        app.turn_snapshot = Some(TurnSnapshot::new(app.session_id.clone(), turn));
        return false;
    }
    // Only the latest turn can still end.
    let may_still_end = turn == app.turns();
    let loaded = app
        .turn_snapshot
        .as_mut()
        .expect("checked to be on this turn above")
        .load(client, may_still_end);
    match loaded {
        Ok(changed) => changed,
        Err(message) => {
            app.push_log(styra_protocol::LogEntry::warn(message));
            false
        }
    }
}

#[cfg(test)]
impl TurnSnapshot {
    /// A turn whose snapshots have been read, without a server to read them.
    pub(crate) fn loaded(session_id: &str, turn: u64, diff: Option<&str>) -> Self {
        Self::loaded_as(session_id, turn, diff, true)
    }

    /// As [`Self::loaded`], for a turn that may not have ended.
    pub(crate) fn loaded_as(session_id: &str, turn: u64, diff: Option<&str>, ended: bool) -> Self {
        Self {
            contents: Contents::Ready(diff.map(|diff| TurnDiff {
                diff: diff.to_owned(),
                ended,
            })),
            ..Self::new(session_id.into(), turn)
        }
    }
}
