//! The log of the Session on the other side of a `branch` marker, read while
//! the preview rests on that marker.
//!
//! A marker says only where the other side is; what happened there is what
//! the operator is looking at it to find out. The log is fetched from the
//! server, so — as in the session picker — the load waits for the cursor to
//! settle, and walking past a run of markers costs no loads at all.

use std::time::{Duration, Instant};

use styra_protocol::event::{AgentEvent, BranchDirection, Protocol};
use styra_protocol::{Contract, InteractionUpdate};
use styra_server::Client;

/// How long the preview must rest on a marker before its Session is loaded.
const SETTLE: Duration = Duration::from_millis(120);

pub enum Contents {
    Loading,
    Ready {
        entries: Vec<Entry>,
        protocol: Protocol,
    },
    Failed(String),
}

/// One event of the linked Session's log, as its own event list holds it: an
/// operator message without the server's contract framing, and the answer
/// shape it asked for.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub event: AgentEvent,
    pub contract: Option<Contract>,
}

pub struct BranchLog {
    session_id: String,
    since: Instant,
    contents: Contents,
    /// Where a live Interaction's updates continue from, so a branch that is
    /// still working keeps appearing; `None` for a stored Session.
    live_cursor: Option<u64>,
}

impl BranchLog {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            since: Instant::now(),
            contents: Contents::Loading,
            live_cursor: None,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn contents(&self) -> &Contents {
        &self.contents
    }

    /// Fetch the log once the cursor has settled, then follow a live one.
    /// Whether anything on screen changed.
    fn load(&mut self, client: &Client, live: Option<Protocol>) -> bool {
        match (&mut self.contents, self.live_cursor) {
            (Contents::Loading, _) if self.since.elapsed() < SETTLE => false,
            (Contents::Loading, _) => {
                self.contents = match live {
                    Some(protocol) => match client.updates_without_raw(&self.session_id, 0) {
                        Ok(batch) => {
                            self.live_cursor = Some(batch.next);
                            Contents::Ready {
                                entries: entries(batch.updates.into_iter().map(|s| s.update)),
                                protocol,
                            }
                        }
                        Err(error) => Contents::Failed(format!("{error:#}")),
                    },
                    // The preview shows decoded events only, so the raw wire
                    // lines are left on the server.
                    None => match client.stored_session_events(&self.session_id) {
                        Ok(session) => Contents::Ready {
                            entries: entries(
                                session.events.into_iter().map(InteractionUpdate::Event),
                            ),
                            protocol: session.summary.selection.provider.protocol(),
                        },
                        Err(error) => Contents::Failed(format!("{error:#}")),
                    },
                };
                true
            }
            (Contents::Ready { entries: shown, .. }, Some(cursor)) => {
                // A failed poll leaves what was read on screen; the next
                // round asks again.
                let Ok(batch) = client.updates_without_raw(&self.session_id, cursor) else {
                    return false;
                };
                self.live_cursor = Some(batch.next);
                let more = entries(batch.updates.into_iter().map(|s| s.update));
                let changed = !more.is_empty();
                shown.extend(more);
                changed
            }
            _ => false,
        }
    }
}

fn entries(updates: impl Iterator<Item = InteractionUpdate>) -> Vec<Entry> {
    updates
        .filter_map(|update| match update {
            InteractionUpdate::Event(AgentEvent::Unknown { .. }) => None,
            InteractionUpdate::Event(event) => {
                let (event, contract) = crate::ingest::unframed(event);
                Some(Entry { event, contract })
            }
            _ => None,
        })
        .collect()
}

/// The part of the linked Session's log a marker leads to. A branch's own
/// history begins after its last `branched from` marker (see
/// [`crate::timeline::Timeline::is_visible`]); what comes before was copied
/// from the source this screen is showing, so following a `branched to`
/// marker shows only what the branch did. The source's log is shown whole.
pub fn shown_from(direction: BranchDirection, entries: &[Entry]) -> &[Entry] {
    if direction != BranchDirection::To {
        return entries;
    }
    let start = entries
        .iter()
        .rposition(|entry| {
            matches!(
                entry.event,
                AgentEvent::Branched {
                    direction: BranchDirection::From,
                    ..
                }
            )
        })
        .map_or(0, |marker| marker + 1);
    &entries[start..]
}

/// Keep `app.branch_log` on the Session the preview's marker names: start a
/// fresh load when it moves to another, and drop it once the preview leaves
/// markers. A log that has just loaded is shown from its end. Whether
/// anything on screen changed.
pub fn sync(app: &mut crate::app::App, client: &Client) -> bool {
    let Some(target) = app
        .preview_branch_target()
        .map(|(target, _)| target.to_owned())
    else {
        app.branch_log = None;
        return false;
    };
    if app.branch_log.as_ref().map(BranchLog::session_id) != Some(target.as_str()) {
        app.branch_log = Some(BranchLog::new(target));
        return true;
    }
    let live = app
        .interactions
        .current(&target)
        .filter(|interaction| !interaction.restored)
        .map(|interaction| interaction.selection.provider.protocol());
    let Some(log) = app.branch_log.as_mut() else {
        return false;
    };
    let was_loading = matches!(log.contents, Contents::Loading);
    let changed = log.load(client, live);
    // What the operator opens a marker to see is how the other side ended
    // up, so a freshly loaded log is shown from its bottom.
    if was_loading && !matches!(log.contents, Contents::Loading) {
        app.preview.scroll.scroll_to_end();
    }
    changed
}

#[cfg(test)]
impl BranchLog {
    /// A log as it stands once loaded, without a server to load it from.
    pub(crate) fn loaded(session_id: &str, events: Vec<AgentEvent>, protocol: Protocol) -> Self {
        Self {
            contents: Contents::Ready {
                entries: entries(events.into_iter().map(InteractionUpdate::Event)),
                protocol,
            },
            ..Self::new(session_id.into())
        }
    }
}

#[cfg(test)]
mod tests;
