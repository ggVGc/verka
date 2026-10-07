//! The log of the Session on the other side of a `branch` marker, read while
//! the preview rests on that marker.
//!
//! A marker says only where the other side is; what happened there is what
//! the operator is looking at it to find out. The log is fetched from the
//! server, so — as in the session picker — the load waits for the cursor to
//! settle, and walking past a run of markers costs no loads at all.

use std::time::{Duration, Instant};

use styra_protocol::event::{AgentEvent, BranchDirection, Protocol};
use styra_protocol::InteractionUpdate;
use styra_server::Client;

/// How long the preview must rest on a marker before its Session is loaded.
const SETTLE: Duration = Duration::from_millis(120);

pub enum Contents {
    Loading,
    Ready {
        events: Vec<AgentEvent>,
        protocol: Protocol,
    },
    Failed(String),
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
                                events: events(batch.updates.into_iter().map(|s| s.update)),
                                protocol,
                            }
                        }
                        Err(error) => Contents::Failed(format!("{error:#}")),
                    },
                    // The preview shows decoded events only, so the raw wire
                    // lines are left on the server.
                    None => match client.stored_session_events(&self.session_id) {
                        Ok(session) => Contents::Ready {
                            events: events(
                                session.events.into_iter().map(InteractionUpdate::Event),
                            ),
                            protocol: session.summary.selection.provider.protocol(),
                        },
                        Err(error) => Contents::Failed(format!("{error:#}")),
                    },
                };
                true
            }
            (Contents::Ready { events: shown, .. }, Some(cursor)) => {
                // A failed poll leaves what was read on screen; the next
                // round asks again.
                let Ok(batch) = client.updates_without_raw(&self.session_id, cursor) else {
                    return false;
                };
                self.live_cursor = Some(batch.next);
                let more = events(batch.updates.into_iter().map(|s| s.update));
                let changed = !more.is_empty();
                shown.extend(more);
                changed
            }
            _ => false,
        }
    }
}

fn events(updates: impl Iterator<Item = InteractionUpdate>) -> Vec<AgentEvent> {
    updates
        .filter_map(|update| match update {
            InteractionUpdate::Event(AgentEvent::Unknown { .. }) => None,
            InteractionUpdate::Event(event) => Some(event),
            _ => None,
        })
        .collect()
}

/// The part of the linked Session's log a marker leads to. A branch's own
/// history begins after its last `branched from` marker (see
/// [`crate::timeline::Timeline::branch_point`]); what comes before was copied
/// from the source this screen is showing, so following a `branched to`
/// marker shows only what the branch did. The source's log is shown whole.
pub fn shown_from(direction: BranchDirection, events: &[AgentEvent]) -> &[AgentEvent] {
    if direction != BranchDirection::To {
        return events;
    }
    let start = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                AgentEvent::Branched {
                    direction: BranchDirection::From,
                    ..
                }
            )
        })
        .map_or(0, |marker| marker + 1);
    &events[start..]
}

/// Keep `app.branch_log` on the Session the preview's marker names: start a
/// fresh load when it moves to another, and drop it once the preview leaves
/// markers. Whether anything on screen changed.
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
    app.branch_log
        .as_mut()
        .is_some_and(|log| log.load(client, live))
}

#[cfg(test)]
impl BranchLog {
    /// A log as it stands once loaded, without a server to load it from.
    pub(crate) fn loaded(session_id: &str, events: Vec<AgentEvent>, protocol: Protocol) -> Self {
        Self {
            contents: Contents::Ready { events, protocol },
            ..Self::new(session_id.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::test_support;

    fn marker(direction: BranchDirection, session: &str) -> AgentEvent {
        AgentEvent::Branched {
            direction,
            session: session.into(),
            name: None,
        }
    }

    fn said(text: &str) -> AgentEvent {
        AgentEvent::UserMessage { text: text.into() }
    }

    /// The branch's copy of the source's history is already on this screen;
    /// following the marker to it shows what the branch did after.
    #[test]
    fn a_branch_is_shown_from_where_its_own_history_begins() {
        let events = vec![
            marker(BranchDirection::From, "source"),
            said("inherited"),
            marker(BranchDirection::From, "source"),
            said("its own"),
        ];
        assert_eq!(
            shown_from(BranchDirection::To, &events),
            &events[3..],
            "after the last `branched from` marker"
        );
        assert_eq!(shown_from(BranchDirection::From, &events), &events[..]);
    }

    fn app_on_marker() -> crate::app::App {
        let mut app = test_support::app("source");
        app.push_event(said("before the branch"));
        app.push_event(marker(BranchDirection::To, "branch-2"));
        app.select_last();
        app.preview.show();
        app
    }

    #[test]
    fn the_preview_on_a_branch_marker_shows_the_linked_sessions_log() {
        let mut app = app_on_marker();
        let protocol = app.selection.provider.protocol();
        app.branch_log = Some(BranchLog::loaded(
            "branch-2",
            vec![
                marker(BranchDirection::From, "source"),
                said("before the branch"),
                marker(BranchDirection::From, "source"),
                said("try the other approach"),
                AgentEvent::AgentMessage {
                    text: "done it the other way".into(),
                },
            ],
            protocol,
        ));
        let screen = test_support::screen_sized(&app, 140, 30);
        let (preview_x, _) = screen.find("branch log · C: command");
        let (message_x, _) = screen.find("try the other approach");
        assert!(message_x > preview_x, "the branch's log is in the preview");
        assert!(screen.all().contains("done it the other way"));
    }

    /// Until the marker the cursor reached is loaded, the pane says so rather
    /// than showing the log of the one it left.
    #[test]
    fn a_marker_not_yet_loaded_reads_as_loading() {
        let mut app = app_on_marker();
        let protocol = app.selection.provider.protocol();
        app.branch_log = Some(BranchLog::loaded(
            "elsewhere",
            vec![said("another branch entirely")],
            protocol,
        ));
        let screen = test_support::screen_sized(&app, 140, 30).all();
        assert!(screen.contains("loading…"), "{screen}");
        assert!(!screen.contains("another branch entirely"));
    }

    #[test]
    fn the_preview_off_a_marker_has_no_branch_to_load() {
        let mut app = app_on_marker();
        assert!(app.preview_branch_target().is_some());
        app.select_first();
        assert!(app.preview_branch_target().is_none());
        app.select_last();
        app.preview.toggle();
        assert!(
            app.preview_branch_target().is_none(),
            "the preview is closed"
        );
    }
}
