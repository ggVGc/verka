//! Fetching Interaction snapshots off the event-loop thread, and deciding
//! which of the answers that come back still matter.
//!
//! Moving through the Interaction navigator asks the server for a snapshot of
//! each Interaction the selection lands on. Those fetches are slower than the
//! selection moves, so answers arrive for Interactions the operator has
//! already left, and a preview can arrive after the full load that superseded
//! it. Three pieces of state decide whether an answer is still wanted — which
//! Interaction the view is on, which request is outstanding, and how many
//! times the view has moved — and they only mean anything together.
//!
//! They were three `&mut` locals threaded through five call sites in
//! [`crate::event_loop`], with the "abandon what is in flight and settle here"
//! sequence written out by hand twice, in different orders. Here the rule is
//! [`Loads::accepts`] and the sequence is [`Loads::settle_on`].

use std::sync::mpsc::{self, Receiver, Sender};

use styra_server::{Client, InteractionSnapshot, InteractionSnapshotScope};

/// A request to the loader thread.
#[derive(Clone, Debug)]
struct LoadRequest {
    request_id: String,
    generation: u64,
    id: String,
    scope: InteractionSnapshotScope,
}

/// A snapshot the loader thread finished fetching, wanted or not.
pub struct LoadEvent {
    request_id: String,
    generation: u64,
    pub id: String,
    pub result: anyhow::Result<InteractionSnapshot>,
}

#[derive(Clone, Debug)]
enum Command {
    Load(LoadRequest),
    Cancel(String),
}

/// What the view is waiting for, and what it is waiting on behalf of.
pub struct Loads {
    requests: Sender<Command>,
    /// The Interaction the view is currently on. An answer about any other one
    /// is stale by definition.
    active_id: String,
    /// Bumped every time the view moves. An answer carrying an older
    /// generation was asked for on behalf of a view that no longer exists —
    /// which is how a preview that arrives after the full load that replaced
    /// it is rejected, even though both name the same Interaction.
    generation: u64,
    /// The request the view is waiting on, if any.
    pending: Option<String>,
}

impl Loads {
    /// Start the loader thread and the state that tracks it.
    ///
    /// A coordinator thread keeps navigator movement non-blocking. Fetches use
    /// their own threads so the coordinator stays able to tell the server to
    /// cancel an outstanding request as soon as the selection moves elsewhere.
    pub fn start(client: Client, active_id: String) -> (Self, Receiver<LoadEvent>) {
        let (requests, commands) = mpsc::channel::<Command>();
        let (events_tx, events) = mpsc::channel();
        std::thread::Builder::new()
            .name("styra-interaction-preview".into())
            .spawn(move || {
                while let Ok(command) = commands.recv() {
                    match command {
                        Command::Cancel(request_id) => {
                            let _ = client.cancel_interaction_snapshot(&request_id);
                        }
                        Command::Load(request) => {
                            let fetch_client = client.clone();
                            let fetch_events = events_tx.clone();
                            let _ = std::thread::Builder::new()
                                .name("styra-interaction-fetch".into())
                                .spawn(move || {
                                    let result = fetch_client.interaction_snapshot_requested(
                                        &request.request_id,
                                        &request.id,
                                        request.scope,
                                    );
                                    let _ = fetch_events.send(LoadEvent {
                                        request_id: request.request_id,
                                        generation: request.generation,
                                        id: request.id,
                                        result,
                                    });
                                });
                        }
                    }
                }
            })
            .expect("starting interaction preview worker");
        (
            Self {
                requests,
                active_id,
                generation: 0,
                pending: None,
            },
            events,
        )
    }

    /// The Interaction the view is on.
    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    /// Whether the view has been left pointing at an Interaction other than
    /// the one on screen, which is what the navigator's own movement does.
    pub fn is_on(&self, session_id: &str) -> bool {
        self.active_id == session_id
    }

    /// Ask for a snapshot of `id`, abandoning whatever was in flight.
    pub fn request(&mut self, id: String, scope: InteractionSnapshotScope) {
        self.settle_on(id.clone());
        let request_id = Client::interaction_snapshot_request_id();
        self.pending = Some(request_id.clone());
        let _ = self.requests.send(Command::Load(LoadRequest {
            request_id,
            generation: self.generation,
            id,
            scope,
        }));
    }

    /// Abandon whatever is in flight and treat `id` as the Interaction the
    /// view is on, without asking for anything.
    ///
    /// The generation moves even though no request follows, so an answer to
    /// the abandoned request is rejected when it arrives rather than being
    /// applied to a view that has moved on.
    pub fn settle_on(&mut self, id: String) {
        self.cancel();
        self.generation = self.generation.wrapping_add(1);
        self.active_id = id;
    }

    /// Tell the server to stop working on the outstanding request, if there
    /// is one. Cheap to call when there is not.
    pub fn cancel(&mut self) {
        if let Some(request_id) = self.pending.take() {
            let _ = self.requests.send(Command::Cancel(request_id));
        }
    }

    /// Whether an answer is still wanted: the same Interaction, the request
    /// actually outstanding, and the generation it was asked for under.
    ///
    /// All three are needed. The id alone lets a superseded preview of the
    /// Interaction still on screen overwrite its full load; the request id
    /// alone would accept an answer after the view moved away and back.
    pub fn accepts(&self, event: &LoadEvent) -> bool {
        event.generation == self.generation
            && event.id == self.active_id
            && self.pending.as_deref() == Some(event.request_id.as_str())
    }

    /// Note that the outstanding request has been answered. Separate from
    /// [`Self::cancel`]: there is nothing left for the server to stop.
    pub fn answered(&mut self) {
        self.pending = None;
    }

    /// Whether a snapshot is the one that was asked for, rather than a
    /// coincidental answer carrying another Interaction or request id.
    pub fn matches(&self, snapshot: &InteractionSnapshot, request_id: &str) -> bool {
        snapshot.request_id == request_id && snapshot.interaction.id == self.active_id
    }

    /// The request id an answer arrived under, for [`Self::matches`].
    pub fn request_id_of(event: &LoadEvent) -> &str {
        &event.request_id
    }

    /// Whether an answer is still awaited.
    #[cfg(test)]
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }
}

/// A tracker in a known state, with no loader thread behind it, for tests
/// about what happens to a screen when an answer is accepted or refused.
#[cfg(test)]
pub fn waiting_for(active_id: &str, request_id: &str, generation: u64) -> Loads {
    let (requests, commands) = mpsc::channel();
    // The tests do not read the commands, but dropping the receiver would make
    // every send fail; leaking it keeps the channel open for the test's life.
    std::mem::forget(commands);
    Loads {
        requests,
        active_id: active_id.to_owned(),
        generation,
        pending: Some(request_id.to_owned()),
    }
}

/// An answer as the loader thread would deliver it.
#[cfg(test)]
pub fn load_event(
    request_id: &str,
    generation: u64,
    id: &str,
    result: anyhow::Result<InteractionSnapshot>,
) -> LoadEvent {
    LoadEvent {
        request_id: request_id.to_owned(),
        generation,
        id: id.to_owned(),
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built without a loader thread: these tests are about which answers are
    /// accepted, which needs no fetching.
    fn loads(active_id: &str) -> (Loads, Receiver<Command>) {
        let (requests, commands) = mpsc::channel();
        (
            Loads {
                requests,
                active_id: active_id.to_owned(),
                generation: 0,
                pending: None,
            },
            commands,
        )
    }

    fn answer(loads: &Loads, id: &str) -> LoadEvent {
        LoadEvent {
            request_id: loads.pending.clone().unwrap_or_default(),
            generation: loads.generation,
            id: id.to_owned(),
            result: Err(anyhow::anyhow!("not used by these tests")),
        }
    }

    #[test]
    fn the_answer_to_the_outstanding_request_is_accepted() {
        let (mut loads, _commands) = loads("one");
        loads.request("one".into(), InteractionSnapshotScope::Full);

        assert!(loads.accepts(&answer(&loads, "one")));
    }

    /// The navigator moves faster than the server answers, so answers arrive
    /// for Interactions the operator has already left.
    #[test]
    fn an_answer_about_an_interaction_the_view_has_left_is_rejected() {
        let (mut loads, _commands) = loads("one");
        loads.request("one".into(), InteractionSnapshotScope::Full);
        let in_flight = answer(&loads, "one");

        loads.request("two".into(), InteractionSnapshotScope::Full);

        assert!(!loads.accepts(&in_flight));
        assert!(loads.accepts(&answer(&loads, "two")));
    }

    /// The case the id alone cannot catch: a cheap preview and a full load of
    /// the *same* Interaction, where the preview lands second and would
    /// otherwise replace the complete view with a bounded one.
    #[test]
    fn a_superseded_preview_of_the_same_interaction_is_rejected() {
        let (mut loads, _commands) = loads("one");
        loads.request("one".into(), InteractionSnapshotScope::Preview { limit: 5 });
        let preview = answer(&loads, "one");

        loads.request("one".into(), InteractionSnapshotScope::Full);

        assert_eq!(preview.id, loads.active_id, "the same interaction");
        assert!(!loads.accepts(&preview), "but asked for by an older view");
    }

    /// Settling moves the generation even though nothing is asked for, so the
    /// answer to the request it abandoned does not arrive later and land.
    #[test]
    fn settling_abandons_what_was_in_flight() {
        let (mut loads, _commands) = loads("one");
        loads.request("one".into(), InteractionSnapshotScope::Full);
        let abandoned = answer(&loads, "one");

        loads.settle_on("one".into());

        assert!(!loads.accepts(&abandoned));
        assert!(loads.pending.is_none());
    }

    #[test]
    fn an_answer_is_only_accepted_once() {
        let (mut loads, _commands) = loads("one");
        loads.request("one".into(), InteractionSnapshotScope::Full);
        let event = answer(&loads, "one");
        assert!(loads.accepts(&event));

        loads.answered();

        assert!(!loads.accepts(&event), "nothing is outstanding any more");
    }

    /// Cancelling reaches the server: an abandoned fetch is work the daemon
    /// should stop doing, not just an answer this client will ignore. The
    /// cancel has to go first, so the daemon is never asked to run both.
    #[test]
    fn moving_on_cancels_the_outstanding_fetch_before_asking_for_the_next() {
        let (mut loads, commands) = loads("one");
        loads.request("one".into(), InteractionSnapshotScope::Full);
        let Command::Load(first) = commands.recv().expect("the first load") else {
            panic!("expected a load");
        };
        let generation = loads.generation;

        loads.request("two".into(), InteractionSnapshotScope::Preview { limit: 5 });

        match commands.recv().expect("the cancel") {
            Command::Cancel(cancelled) => assert_eq!(cancelled, first.request_id),
            Command::Load(_) => panic!("the cancel must come first"),
        }
        let Command::Load(second) = commands.recv().expect("the replacement load") else {
            panic!("the replacement fetch must follow cancellation");
        };
        assert_eq!(second.id, "two");
        assert_eq!(loads.active_id(), "two");
        assert_eq!(loads.pending.as_deref(), Some(second.request_id.as_str()));
        assert_eq!(loads.generation, generation + 1);
    }

    #[test]
    fn cancelling_with_nothing_outstanding_says_nothing() {
        let (mut loads, commands) = loads("one");

        loads.cancel();

        assert!(commands.try_recv().is_err());
    }
}
