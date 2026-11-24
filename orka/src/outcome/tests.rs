use super::*;

#[test]
fn declarations_round_trip_and_absence_is_an_answer() {
    let dir = std::env::temp_dir().join(format!("orka-outcome-test-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(read_declared(&dir).unwrap(), None);

    std::fs::write(
        dir.join(OUTCOME_FILE),
        "outcome = \"succeeded\"\nnotes = \"did it\"\n",
    )
    .unwrap();
    let declared = read_declared(&dir).unwrap().unwrap();
    assert_eq!(declared.outcome, DeclaredKind::Succeeded);
    assert_eq!(declared.notes, "did it");

    std::fs::write(
        dir.join(OUTCOME_FILE),
        "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\n",
    )
    .unwrap();
    assert!(read_declared(&dir).is_err(), "unknown fields are errors");

    std::fs::write(dir.join(OUTCOME_FILE), "outcome = \"maybe\"").unwrap();
    assert!(read_declared(&dir).is_err(), "garbage is an error");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_failure_matrix_decides_every_combination() {
    let succeeded = DeclaredOutcome {
        outcome: DeclaredKind::Succeeded,
        message: None,
        notes: "n".into(),
    };
    // Declared success plus exit zero: submit.
    assert_eq!(
        decide(Some(succeeded.clone()), 0),
        Decision::Submit {
            outcome: AgentOutcome::Succeeded {
                message: None,
                notes: "n".into(),
            },
            backend_failed: false,
        }
    );
    // Declared success plus nonzero exit cannot complete the node.
    assert!(matches!(
        decide(Some(succeeded), 1),
        Decision::Interrupted { reason }
            if reason == "command exited 1 after declaring success"
    ));
    // Declared failure is failure evidence.
    assert!(matches!(
        decide(
            Some(DeclaredOutcome {
                outcome: DeclaredKind::Failed,
                message: None,
                notes: "why".into(),
            }),
            0
        ),
        Decision::Submit {
            outcome: AgentOutcome::Failed { .. },
            ..
        }
    ));
    // No declaration: exit zero violates the contract; nonzero interrupts.
    assert!(matches!(
        decide(None, 0),
        Decision::ContractViolation { .. }
    ));
    assert!(matches!(decide(None, 137), Decision::Interrupted { .. }));
}
