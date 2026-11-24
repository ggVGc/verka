use super::{state_summary, strip_comment_lines};
use linka::{
    Blocker, BlockerReason, Currency, IntegrationStatus, NodeState, RecordedOutcome,
    StalenessReason,
};

#[test]
fn strip_comment_lines_follows_the_git_template_convention() {
    let text = "\n# Notes for task-1 — title\n# ignored\nDid the work.\n\nMore detail.\n# trailing comment\n";
    assert_eq!(strip_comment_lines(text), "Did the work.\n\nMore detail.");
    assert_eq!(strip_comment_lines("# only comments\n#\n"), "");
    assert_eq!(strip_comment_lines(""), "");
}

#[test]
fn state_summary_distinguishes_prior_evidence_and_blocking() {
    let stale = NodeState {
        outcome: RecordedOutcome::Succeeded,
        currency: Currency::Stale,
        integration: IntegrationStatus::NotRequired,
        staleness: vec![StalenessReason::ContextMissing {
            path: "input".parse().unwrap(),
        }],
        blockers: vec![],
    };
    assert!(state_summary(&stale).starts_with("ready (previous result stale:"));
    let failed = NodeState {
        outcome: RecordedOutcome::Failed,
        currency: Currency::Current,
        integration: IntegrationStatus::NotRequired,
        staleness: vec![],
        blockers: vec![],
    };
    assert_eq!(state_summary(&failed), "ready (previous attempt failed)");
    let blocked = NodeState {
        blockers: vec![Blocker {
            id: "dependency".parse().unwrap(),
            reason: BlockerReason::Stale,
        }],
        ..failed
    };
    assert_eq!(
        state_summary(&blocked),
        "blocked by dependency: not complete (stale)"
    );
}
