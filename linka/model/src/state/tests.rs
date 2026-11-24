use super::*;

#[test]
fn derived_reasons_render_the_same_words_for_every_reader() {
    assert_eq!(
        StalenessReason::ContextChanged {
            path: "src/lib.rs".parse().unwrap(),
        }
        .to_string(),
        "context src/lib.rs: content changed"
    );
    assert_eq!(
        StalenessReason::DefinitionChanged {
            metadata: true,
            description: true,
        }
        .to_string(),
        "definition changed since the work (node.toml, description.md)"
    );
    assert_eq!(
        StalenessReason::OutputDrifted {
            artifact: "artifact-1".into(),
            detail: "first\nsecond".into(),
        }
        .to_string(),
        "output changed since artifact-1:\nfirst\nsecond"
    );
    assert_eq!(
        Blocker {
            id: "node-dependency".parse().unwrap(),
            reason: BlockerReason::Stale,
        }
        .to_string(),
        "node-dependency: not complete (stale)"
    );
    assert_eq!(
        Blocker {
            id: "node-candidate".parse().unwrap(),
            reason: BlockerReason::AwaitingIntegration,
        }
        .to_string(),
        "node-candidate: awaiting candidate integration"
    );
}

#[test]
fn integration_is_done_after_publication_or_rejection() {
    assert!(IntegrationStatus::NotRequired.is_done());
    assert!(!IntegrationStatus::Pending.is_done());
    assert!(!IntegrationStatus::Accepted.is_done());
    assert!(IntegrationStatus::Published.is_done());
    assert!(IntegrationStatus::Rejected.is_done());
}

#[test]
fn state_classification_has_one_frontend_precedence() {
    let state = |outcome, currency, integration, blocked: bool| NodeState {
        outcome,
        currency,
        integration,
        staleness: Vec::new(),
        blockers: blocked
            .then(|| Blocker {
                id: "dependency".parse().unwrap(),
                reason: BlockerReason::Open,
            })
            .into_iter()
            .collect(),
    };
    use IntegrationStatus as I;
    use RecordedOutcome as O;
    assert_eq!(
        state(O::Succeeded, Currency::Current, I::Published, false).classification(),
        StateClass::Complete
    );
    assert_eq!(
        state(O::Succeeded, Currency::Current, I::Pending, false).classification(),
        StateClass::AwaitingIntegration
    );
    assert_eq!(
        state(O::Succeeded, Currency::Stale, I::Rejected, false).classification(),
        StateClass::Ready
    );
    assert_eq!(
        state(O::Failed, Currency::Current, I::NotRequired, true).classification(),
        StateClass::Blocked
    );
    assert_eq!(
        state(O::Accepted, Currency::Current, I::NotRequired, false).classification(),
        StateClass::Accepted
    );
    assert_eq!(
        state(O::Rejected, Currency::Current, I::NotRequired, false).classification(),
        StateClass::Rejected
    );
    assert_eq!(
        state(O::Abandoned, Currency::Current, I::NotRequired, false).classification(),
        StateClass::Abandoned
    );
}
