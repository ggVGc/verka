use super::*;

fn report(sealed: SealedState) -> RunReport {
    RunReport {
        attempt: AttemptId("attempt-test".into()),
        node: "node-test".parse().unwrap(),
        candidate: None,
        sealed,
        exit_code: 0,
        backend_failed: false,
        cleanup: CleanupOutcome::Removed,
    }
}

#[test]
fn unsuccessful_run_reports_produce_cli_failure_reasons() {
    assert!(run_failure_reason(&report(SealedState::Submitted {
        output_commit: None,
    }))
    .is_none());
    assert!(run_failure_reason(&report(SealedState::FailureRecorded)).is_none());

    let interrupted = report(SealedState::Interrupted {
        reason: "executor stopped".into(),
    });
    assert!(run_failure_reason(&interrupted)
        .unwrap()
        .contains("interrupted"));

    let mut backend_failed = report(SealedState::FailureRecorded);
    backend_failed.backend_failed = true;
    assert!(run_failure_reason(&backend_failed)
        .unwrap()
        .contains("exited nonzero"));

    let mut cleanup_failed = report(SealedState::Submitted {
        output_commit: None,
    });
    cleanup_failed.cleanup = CleanupOutcome::RetainedIntegrityFailure;
    assert!(run_failure_reason(&cleanup_failed)
        .unwrap()
        .contains("cleanup"));
}
