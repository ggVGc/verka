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

use std::collections::VecDeque;

use styra_server::{Contract, QueuedMessage};

/// The pending contract and the queue of messages waiting to be sent.
#[derive(Default)]
pub struct Outbox {
    /// The shape the next message asks its reply to come back in, or `None`
    /// for an ordinary turn.
    contract: Option<Contract>,
    /// Messages submitted while the current turn was running. They remain
    /// visible until sent or the interaction is stopped.
    queued: VecDeque<QueuedMessage>,
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

    pub fn queue(&mut self, message: QueuedMessage) {
        self.queued.push_back(message);
    }

    /// Take the message that has waited longest, for sending.
    pub fn take_queued(&mut self) -> Option<QueuedMessage> {
        self.queued.pop_front()
    }

    /// Drop everything waiting, returning how many there were, so the caller
    /// can say what it discarded.
    pub fn clear_queued(&mut self) -> usize {
        let count = self.queued.len();
        self.queued.clear();
        count
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
    fn queued_messages_are_sent_in_the_order_they_were_written() {
        let mut outbox = Outbox::default();
        outbox.queue(QueuedMessage::new("first"));
        outbox.queue(QueuedMessage::new("second"));
        assert_eq!(outbox.queued_count(), 2);

        assert_eq!(outbox.take_queued().unwrap().text, "first");
        assert_eq!(outbox.take_queued().unwrap().text, "second");
        assert!(outbox.take_queued().is_none());
    }

    /// A queued message carries the shape it was written under, so a contract
    /// chosen for it is still asked for whenever the agent gets to it.
    #[test]
    fn a_queued_message_keeps_its_own_contract() {
        let mut outbox = Outbox::default();
        outbox.cycle_contract();
        let contract = outbox.take_contract();
        outbox.queue(QueuedMessage::new("typed").asking_for(contract));
        outbox.queue(QueuedMessage::new("untyped"));

        assert_eq!(outbox.take_queued().unwrap().contract, contract);
        assert_eq!(outbox.take_queued().unwrap().contract, None);
    }

    #[test]
    fn clearing_says_how_many_were_discarded() {
        let mut outbox = Outbox::default();
        outbox.queue(QueuedMessage::new("one"));
        outbox.queue(QueuedMessage::new("two"));

        assert_eq!(outbox.clear_queued(), 2);
        assert_eq!(outbox.queued_count(), 0);
        assert_eq!(outbox.clear_queued(), 0);
    }
}
