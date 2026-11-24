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
