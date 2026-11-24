use super::*;
use styra_protocol::{
    agent::Provider,
    event::{AgentEvent, BranchDirection},
    BranchHistory, SessionOrigin,
};

#[test]
fn provider_switch_resolves_on_both_sides_from_persisted_origin() {
    let mut app = App::new(
        styra_protocol::agent::Selection::new(Provider::Claude),
        "source",
    );
    let origin = SessionOrigin {
        session_id: "source".into(),
        provider: Provider::Claude,
        at_ms: None,
        history: BranchHistory::ThroughSelected,
    };
    app.interactions
        .note_origin("destination", Provider::Codex, Some(&origin));
    let marker = |direction, session: &str| AgentEvent::Branched {
        direction,
        session: session.into(),
        name: None,
    };
    let expected = Some((Provider::Claude, Provider::Codex));
    assert_eq!(
        branch_provider_switch(&app, &marker(BranchDirection::To, "destination")),
        expected
    );
    app.session_id = "destination".into();
    assert_eq!(
        branch_provider_switch(&app, &marker(BranchDirection::From, "source")),
        expected
    );
    assert_eq!(
        branch_provider_switch(&app, &marker(BranchDirection::From, "unrelated")),
        None
    );
    let inherited_origin = SessionOrigin {
        session_id: "destination".into(),
        provider: Provider::Codex,
        at_ms: None,
        history: BranchHistory::ThroughSelected,
    };
    app.interactions
        .note_origin("descendant", Provider::Codex, Some(&inherited_origin));
    app.session_id = "descendant".into();
    assert_eq!(
        branch_provider_switch(&app, &marker(BranchDirection::From, "source")),
        expected
    );
    assert_eq!(
        branch_provider_switch(&app, &marker(BranchDirection::To, "destination")),
        expected
    );
    app.interactions
        .note_origin("ordinary", Provider::Claude, Some(&origin));
    app.session_id = "source".into();
    assert_eq!(
        branch_provider_switch(&app, &marker(BranchDirection::To, "ordinary")),
        None
    );
}
