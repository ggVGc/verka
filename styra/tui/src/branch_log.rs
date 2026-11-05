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
mod tests {
    use super::*;
    use crate::app::App;
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

    /// A screen on `source` whose preview is open on its `branched to`
    /// marker naming `branch-2`.
    fn app_on_marker() -> App {
        let mut app = test_support::app("source");
        app.push_event(said("before the branch"));
        app.push_event(marker(BranchDirection::To, "branch-2"));
        app.select_last();
        app.preview.show();
        app
    }

    /// Give `app` the loaded log of `session`, in its own protocol.
    fn load(app: &mut App, session: &str, events: Vec<AgentEvent>) {
        let protocol = app.selection.provider.protocol();
        app.branch_log = Some(BranchLog::loaded(session, events, protocol));
    }

    /// What the preview draws of the linked log: each entry's event, answer
    /// shape and folding, and the protocol it is drawn in.
    fn shown(app: &App) -> (Vec<(AgentEvent, Option<Contract>, bool)>, Protocol) {
        let view = crate::presentation::preview::view(app, false);
        let Some(styra_ui::preview::BranchLog::Ready { entries, protocol }) = view.branch_log
        else {
            panic!("linked log expected")
        };
        let entries = entries
            .iter()
            .map(|entry| (entry.event.clone(), entry.contract.copied(), entry.expanded))
            .collect();
        (entries, protocol)
    }

    fn target(app: &App) -> Option<&str> {
        app.preview_branch_target().map(|(id, _)| id)
    }

    /// The branch's copy of the source's history is already on this screen;
    /// following the marker to it shows what the branch did after.
    #[test]
    fn a_branch_is_shown_from_where_its_own_history_begins() {
        let events = entries(
            [
                marker(BranchDirection::From, "source"),
                said("inherited"),
                marker(BranchDirection::From, "source"),
                said("its own"),
            ]
            .into_iter()
            .map(InteractionUpdate::Event),
        );
        assert_eq!(
            shown_from(BranchDirection::To, &events),
            &events[3..],
            "after the last `branched from` marker"
        );
        assert_eq!(shown_from(BranchDirection::From, &events), &events[..]);
    }

    /// The log is the linked Session's event list as this one would show it:
    /// from where the branch's own history begins, conversation only, and
    /// every event once all events are shown, folded as a fresh entry there
    /// is. An operator message reads as it was written, with the answer shape
    /// it asked for.
    #[test]
    fn the_preview_on_a_branch_marker_shows_the_linked_log_as_the_list_would() {
        let mut app = app_on_marker();
        let framed = styra_protocol::contract::frame("show me", Contract::Lines);
        let done = AgentEvent::AgentMessage {
            text: "done it the other way".into(),
        };
        load(
            &mut app,
            "branch-2",
            vec![
                marker(BranchDirection::From, "source"),
                said("before the branch"),
                marker(BranchDirection::From, "source"),
                AgentEvent::UserMessage { text: framed },
                AgentEvent::CommandStarted {
                    command: "cargo test".into(),
                },
                done.clone(),
            ],
        );

        app.timeline.all_events = false;
        let (entries, protocol) = shown(&app);
        assert_eq!(protocol, app.selection.provider.protocol());
        assert_eq!(
            entries,
            [
                (said("show me"), Some(Contract::Lines), true),
                (done, None, true),
            ]
        );

        app.timeline.all_events = true;
        let (all, _) = shown(&app);
        assert_eq!(all.len(), 3, "the command too");
        assert!(all.iter().all(|(_, _, expanded)| !expanded));
    }

    /// Until the marker the cursor reached is loaded, the pane says so rather
    /// than showing the log of the one it left.
    #[test]
    fn a_marker_not_yet_loaded_reads_as_loading() {
        let mut app = app_on_marker();
        load(&mut app, "elsewhere", vec![said("another branch entirely")]);
        assert!(matches!(
            crate::presentation::preview::view(&app, false).branch_log,
            Some(styra_ui::preview::BranchLog::Loading)
        ));
    }

    /// The operator opens a marker to see how the other side ended up, so
    /// once its log arrives the preview starts at the bottom rather than
    /// where the loading placeholder left it.
    #[test]
    fn a_freshly_loaded_marker_is_shown_from_its_end() {
        let root = tempfile::tempdir().unwrap();
        let client = Client::new(root.path().join("missing.sock"));
        let mut app = app_on_marker();

        assert!(sync(&mut app, &client), "a load starts");
        assert_eq!(app.preview.scroll.offset, 0, "still loading");

        app.branch_log.as_mut().unwrap().since -= SETTLE;
        assert!(sync(&mut app, &client));
        assert!(!matches!(
            app.branch_log.as_ref().unwrap().contents,
            Contents::Loading
        ));
        assert_eq!(app.preview.scroll.offset, u16::MAX, "at the end");

        app.preview.scroll.page_up();
        sync(&mut app, &client);
        assert_ne!(
            app.preview.scroll.offset,
            u16::MAX,
            "only the first load moves it"
        );
    }

    /// The preview loads whatever row the cursor is on: the marker, or one of
    /// the branches listed beneath it as rows of their own. Off markers, or
    /// with the preview closed, there is nothing to load.
    #[test]
    fn the_preview_loads_the_branch_the_cursor_is_on() {
        use crate::interactions::tests::branched;
        use styra_protocol::InteractionActivity::Running;

        let mut app = app_on_marker();
        app.interactions.refresh(vec![
            branched("branch-2", Running, "source"),
            branched("child-3", Running, "branch-2"),
            branched("grandchild-4", Running, "child-3"),
        ]);

        assert_eq!(target(&app), Some("branch-2"));
        app.select_next_line();
        assert_eq!(target(&app), Some("child-3"));
        app.select_next_line();
        assert_eq!(target(&app), Some("grandchild-4"));
        app.select_prev_line();
        app.select_prev_line();
        assert_eq!(target(&app), Some("branch-2"), "the marker's own row");

        app.select_first();
        assert_eq!(target(&app), None, "off the marker");
        app.select_last();
        app.preview.toggle();
        assert_eq!(target(&app), None, "the preview is closed");
    }
}
