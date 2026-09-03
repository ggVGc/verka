//! Messages on their way out: the shape the next one asks its reply to take,
//! and the ones written while the agent was busy.
//!
//! Held apart from [`App`](crate::app::App) because the two enforce one rule
//! between them — a contract belongs to the question it was chosen for. It is
//! taken as the message leaves ([`Outbox::take_contract`]) rather than read,
//! so it cannot silently type the following message too, and a message that
//! waits in the queue carries its own contract with it rather than leaving it
//! behind to be applied to whatever is sent next.
//!
//! The buffer the message is written in is [`Composer`](crate::composer),
//! which does not depend on there being a session at all.

use styra_server::{Contract, QueuedMessage};

/// The pending contract and a read-only projection of the server-owned queue.
#[derive(Default)]
pub struct Outbox {
    /// The shape the next message asks its reply to come back in, or `None`
    /// for an ordinary turn.
    contract: Option<Contract>,
    /// Messages waiting on the server, retained here only for presentation.
    queued: Vec<QueuedMessage>,
}

impl Outbox {
    /// The shape the message being written asks for, for the message box to
    /// show while it is being written.
    pub fn contract(&self) -> Option<Contract> {
        self.contract
    }

    /// Step to the next return contract for the message being typed.
    pub fn cycle_contract(&mut self) {
        self.contract = crate::session::next_contract(self.contract);
    }

    /// Take the contract the message about to be sent asks for, clearing it.
    ///
    /// Taken rather than read: a contract belongs to the question it was
    /// chosen for, and leaving it set would silently type the next message
    /// too.
    pub fn take_contract(&mut self) -> Option<Contract> {
        self.contract.take()
    }

    /// Adopt a contract chosen on a previous screen, which is still the shape
    /// the message being carried over asks for.
    pub fn set_contract(&mut self, contract: Option<Contract>) {
        self.contract = contract;
    }

    pub fn queued(&self) -> impl Iterator<Item = &QueuedMessage> {
        self.queued.iter()
    }

    pub fn queued_count(&self) -> usize {
        self.queued.len()
    }

    /// Replace the display projection with the server's authoritative state.
    pub fn replace_queued(&mut self, queued: Vec<QueuedMessage>) {
        self.queued = queued;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape was chosen for one question. Reading it instead of taking it
    /// would silently apply it to the next message as well.
    #[test]
    fn a_contract_does_not_outlive_the_message_it_was_chosen_for() {
        let mut outbox = Outbox::default();
        outbox.cycle_contract();
        assert!(outbox.contract().is_some());

        let taken = outbox.take_contract();

        assert!(taken.is_some());
        assert!(outbox.contract().is_none(), "the next message is untyped");
    }

    /// Cycling walks the server's own list in order and ends back at an
    /// untyped turn, so an operator who opened a contract can always get out
    /// of one without leaving the message box.
    #[test]
    fn cycling_contracts_walks_the_list_and_returns_to_an_untyped_turn() {
        let mut outbox = Outbox::default();
        assert_eq!(outbox.contract(), None);

        for expected in crate::session::CONTRACTS {
            outbox.cycle_contract();
            assert_eq!(outbox.contract(), Some(expected));
        }
        outbox.cycle_contract();

        assert_eq!(outbox.contract(), None);
    }

    #[test]
    fn the_server_queue_projection_is_replaced_as_a_whole() {
        let mut outbox = Outbox::default();
        outbox.replace_queued(vec![QueuedMessage::new("one"), QueuedMessage::new("two")]);
        assert_eq!(outbox.queued_count(), 2);

        outbox.replace_queued(Vec::new());
        assert_eq!(outbox.queued_count(), 0);
    }
}
