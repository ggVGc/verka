use super::*;

fn empty_app() -> App {
    App {
        root: PathBuf::new(),
        rows: std::array::from_fn(|_| Vec::new()),
        view: View::Ready,
        selected: 0,
        overlay: None,
        status: String::new(),
        busy: false,
        should_quit: false,
        linka: None,
        linka_active: false,
        active_attempt: None,
        worker: None,
        retained_errors: Vec::new(),
    }
}

#[test]
fn an_unopenable_linka_store_does_not_switch_the_view() {
    let mut app = empty_app();
    app.root = PathBuf::from("/nonexistent-orka-workbench");
    app.enter_linka();
    assert!(!app.linka_active);
    assert!(app.linka.is_none());
    assert!(app.status.starts_with("Linka error"));
}

#[test]
fn view_labels_are_stable_and_complete() {
    assert_eq!(
        View::ALL.map(View::label),
        [
            "Ready",
            "Attempts",
            "Candidates",
            "Reviews",
            "Worktrees",
            "Audit",
            "Errors"
        ]
    );
}

#[test]
fn author_input_is_explicit() {
    assert_eq!(parse_author(" HUMAN ").unwrap(), Author::Human);
    assert!(parse_author("robot").is_err());
}

#[test]
fn attempt_progress_exposes_the_live_attempt_id() {
    let attempt = AttemptId("attempt-live".into());
    let progress = RunProgress::AttemptCreated {
        attempt: attempt.clone(),
    };
    assert_eq!(progress_attempt(&progress), Some(&attempt));

    let selected = RunProgress::Selected {
        node: "node-live".parse().unwrap(),
    };
    assert_eq!(progress_attempt(&selected), None);
}

#[test]
fn popup_errors_are_retained_in_the_errors_view() {
    let mut app = empty_app();

    app.error_overlay(
        "Action failed",
        "could not delete execution worktree\ncaused by: worktree is locked".into(),
    );

    let errors = &app.rows[View::Errors as usize];
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].summary,
        "Action failed: could not delete execution worktree"
    );
    assert_eq!(
        errors[0].detail,
        "Action failed: could not delete execution worktree\ncaused by: worktree is locked"
    );

    app.rebuild_error_rows();
    assert_eq!(app.rows[View::Errors as usize].len(), 1);
}

#[test]
fn completion_status_uses_only_the_first_line() {
    let mut app = empty_app();

    app.finish_action(
        ActionCompletion::normal("Attempt attempt-1 finished for node-1\nexit: 0"),
        true,
    );

    assert_eq!(app.status, "Attempt attempt-1 finished for node-1");
    assert!(app.overlay.is_none());
    assert!(app.rows[View::Errors as usize].is_empty());
}

#[test]
fn completion_requiring_attention_is_visible_and_retained() {
    let mut app = empty_app();
    let message = "Attempt attempt-1 finished for node-1\nexit: 0\nsealed: workspace integrity failure: checkpoint escaped";

    app.finish_action(ActionCompletion::requires_attention(message), true);

    assert_eq!(
        app.status,
        "Attempt requires attention: Attempt attempt-1 finished for node-1"
    );
    let errors = &app.rows[View::Errors as usize];
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].detail,
        format!("Attempt requires attention: {message}")
    );
    assert!(matches!(
        app.overlay,
        Some(Overlay::Text { ref title, ref body, .. })
            if title == "ERROR — Attempt requires attention" && body == message
    ));
}

#[test]
fn unsuccessful_seals_require_attention() {
    assert!(seal_requires_attention(
        &SealedState::WorkspaceIntegrityFailure {
            reason: "checkpoint escaped".into(),
        }
    ));
    assert!(seal_requires_attention(&SealedState::ContractViolation {
        reason: "outcome missing".into(),
    }));
    assert!(!seal_requires_attention(&SealedState::FailureRecorded));
    assert!(!seal_requires_attention(&SealedState::Submitted {
        output_commit: None,
    }));
}

#[test]
fn recovery_problems_require_attention() {
    let completion = recovery_completion(vec![RecoveryReport {
        attempt: AttemptId("attempt-recovery".into()),
        node: "node-recovery".parse().unwrap(),
        action: "unrecoverable without intervention".into(),
        sealed: None,
        requires_attention: true,
    }]);

    assert!(completion.requires_attention);
    assert!(completion.message.contains("unrecoverable"));
}

#[test]
fn stale_review_actions_require_attention() {
    let verification: NodeId = "node-review".parse().unwrap();
    let finish = finish_review_completion(
        &verification,
        FinishOutcome::Conflict(vec![linka::SubmissionConflict::DefinitionChanged]),
    );
    let abandon = abandon_review_completion(
        &verification,
        AbandonOutcome::Conflict(vec![linka::SubmissionConflict::ReadinessChanged]),
    );

    assert!(finish.requires_attention);
    assert!(finish.message.starts_with("Stale node-review"));
    assert!(abandon.requires_attention);
    assert!(abandon.message.starts_with("Stale node-review"));
}
