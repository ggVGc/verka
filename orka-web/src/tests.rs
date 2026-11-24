use super::*;
use linka::{Blocker, BlockerReason, Currency, IntegrationStatus, NodeState, RecordedOutcome};

fn state(integration: IntegrationStatus, blockers: Vec<Blocker>) -> NodeState {
    NodeState {
        outcome: RecordedOutcome::Succeeded,
        currency: Currency::Current,
        integration,
        staleness: Vec::new(),
        blockers,
    }
}

#[test]
fn integration_wait_is_not_presented_as_blocked() {
    let awaiting = state(IntegrationStatus::Pending, Vec::new());
    assert_eq!(workability(&awaiting), "awaiting_integration");

    let mut blocked = state(
        IntegrationStatus::NotRequired,
        vec![Blocker {
            id: "node-dependency".parse().unwrap(),
            reason: BlockerReason::Open,
        }],
    );
    blocked.outcome = RecordedOutcome::Open;
    assert_eq!(workability(&blocked), "blocked");

    let complete = state(IntegrationStatus::Published, Vec::new());
    assert_eq!(workability(&complete), "complete");
}
