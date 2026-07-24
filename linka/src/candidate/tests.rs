use super::*;
use crate::ops::{self, NewNode};
use crate::vcs::FakeVcs;
use std::fs;
use std::path::PathBuf;

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn successful_output() -> (TempDir, Store, FakeVcs, NodeId, String) {
    let root = std::env::temp_dir().join(format!("linka-candidate-test-{}", ulid::Ulid::new()));
    let store = Store::init(root.join(".linka")).unwrap();
    let mut vcs = FakeVcs {
        root: Some("base".into()),
        next_id: "output".into(),
        ..Default::default()
    };
    vcs.commits
        .borrow_mut()
        .extend(["base".into(), "output".into()]);
    let node: NodeId = ops::add(
        &store,
        &vcs,
        NewNode {
            description: "candidate work".into(),
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap()
    .parse()
    .unwrap();
    ops::complete(
        &store,
        &vcs,
        node.as_str(),
        &["out.txt".into()],
        &[],
        None,
        "produced",
        Author::Machine,
    )
    .unwrap();
    vcs.refs
        .get_mut()
        .insert("refs/heads/candidates/a".into(), "output".into());
    vcs.refs
        .get_mut()
        .insert("refs/heads/main".into(), "base".into());
    vcs.drift_for.insert("output".into(), "A out.txt".into());
    (TempDir(root), store, vcs, node, "output".into())
}

fn register(store: &Store, vcs: &FakeVcs, node: &NodeId) -> CandidateRecord {
    CandidateStore::new(store)
        .register(
            vcs,
            NewCandidate {
                node: node.clone(),
                branch: "candidates/a".into(),
                target: "main".into(),
                external: Some(ExternalIdentity {
                    namespace: "test-runner".into(),
                    id: "run-1".into(),
                }),
            },
        )
        .unwrap()
}

fn conclude(
    store: &Store,
    vcs: &FakeVcs,
    candidate: &CandidateRecord,
    outcome: crate::VerificationOutcome,
) -> NodeId {
    let verification: NodeId = ops::add_verification(
        store,
        vcs,
        &candidate.id,
        NewNode {
            description: "Verify the candidate".into(),
            author: Author::Human,
            assignee: Some(Author::Human),
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap()
    .parse()
    .unwrap();
    let snapshot = ops::snapshot_work(store, vcs, verification.as_str(), &[]).unwrap();
    ops::submit_verification(
        store,
        vcs,
        crate::VerificationSubmission {
            snapshot,
            outcome,
            notes: format!("candidate is {}", outcome.as_str()),
            author: Author::Human,
            producer: None,
        },
    )
    .unwrap();
    verification
}

#[test]
fn candidate_acceptance_and_publication_are_first_class_node_state() {
    let (_temp, store, vcs, node, output) = successful_output();
    assert_eq!(
        ops::node_state(&store, &vcs, node.as_str())
            .unwrap()
            .currency,
        crate::Currency::Stale,
        "without a candidate this is a direct output drift"
    );

    let candidate = register(&store, &vcs, &node);
    let state = ops::node_state(&store, &vcs, node.as_str()).unwrap();
    assert_eq!(state.currency, crate::Currency::Current);
    assert_eq!(state.integration, IntegrationStatus::Pending);
    assert!(!state.is_ready());
    assert!(!state.is_complete());

    let candidates = CandidateStore::new(&store);
    let _verification = conclude(
        &store,
        &vcs,
        &candidate,
        crate::VerificationOutcome::Accepted,
    );
    assert_eq!(
        ops::node_state(&store, &vcs, node.as_str())
            .unwrap()
            .integration,
        IntegrationStatus::Accepted
    );
    candidates.publish(&vcs, &candidate.id).unwrap();
    assert_eq!(vcs.refs.borrow().get("refs/heads/main"), Some(&output));
    let state = ops::node_state(&store, &vcs, node.as_str()).unwrap();
    assert_eq!(state.integration, IntegrationStatus::Published);
    assert!(state.is_complete());

    assert_eq!(register(&store, &vcs, &node).id, candidate.id);
    candidates.publish(&vcs, &candidate.id).unwrap();
}

#[test]
fn rejection_returns_the_source_node_to_ready_without_losing_the_candidate() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    let _verification = conclude(
        &store,
        &vcs,
        &candidate,
        crate::VerificationOutcome::Rejected,
    );
    let state = ops::node_state(&store, &vcs, node.as_str()).unwrap();
    assert_eq!(state.integration, IntegrationStatus::Rejected);
    assert!(state.is_ready());
    assert_eq!(
        CandidateStore::new(&store).for_node(&node).unwrap().len(),
        1
    );
}

#[test]
fn rejected_verification_atomically_rejects_the_exact_candidate() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    let rejected = conclude(
        &store,
        &vcs,
        &candidate,
        crate::VerificationOutcome::Rejected,
    );
    let stored = CandidateStore::new(&store).load(&candidate.id).unwrap();
    assert_eq!(
        stored.integration(&vcs).unwrap(),
        IntegrationStatus::Rejected
    );
    assert!(matches!(
        stored.state,
        CandidateState::Rejected {
            verification: Some(ref id),
            ..
        } if id == &rejected
    ));
}

#[test]
fn abandoned_verification_is_terminal_but_cannot_decide_the_candidate() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    let abandoned = conclude(
        &store,
        &vcs,
        &candidate,
        crate::VerificationOutcome::Abandoned,
    );
    let state = ops::node_state(&store, &vcs, abandoned.as_str()).unwrap();
    assert_eq!(state.outcome, crate::RecordedOutcome::Abandoned);
    assert!(state.is_complete());

    let candidates = CandidateStore::new(&store);
    let accept_error = candidates
        .accept(
            &vcs,
            &candidate.id,
            &abandoned,
            Author::Human,
            String::new(),
        )
        .unwrap_err();
    assert!(
        accept_error.to_string().contains("abandoned, not accepted"),
        "{accept_error:#}"
    );
    let reject_error = candidates
        .reject(
            &vcs,
            &candidate.id,
            &abandoned,
            Author::Human,
            "abandoned".into(),
        )
        .unwrap_err();
    assert!(
        reject_error.to_string().contains("abandoned, not rejected"),
        "{reject_error:#}"
    );
    assert_eq!(
        candidates
            .load(&candidate.id)
            .unwrap()
            .integration(&vcs)
            .unwrap(),
        IntegrationStatus::Pending
    );
}

#[test]
fn a_moved_source_cannot_accept_an_obsolete_candidate() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    let verification: NodeId = ops::add_verification(
        &store,
        &vcs,
        &candidate.id,
        NewNode {
            description: "Verify the candidate".into(),
            author: Author::Human,
            assignee: Some(Author::Human),
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap()
    .parse()
    .unwrap();
    let snapshot = ops::snapshot_work(&store, &vcs, verification.as_str(), &[]).unwrap();
    ops::edit(&store, &vcs, node.as_str(), "candidate work changed".into()).unwrap();
    let error = ops::submit_verification(
        &store,
        &vcs,
        crate::VerificationSubmission {
            snapshot,
            outcome: crate::VerificationOutcome::Accepted,
            notes: String::new(),
            author: Author::Human,
            producer: None,
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ops::SubmissionError::Conflict(ref conflicts)
            if conflicts.contains(&crate::SubmissionConflict::LineageChanged)
    ));
    assert_eq!(
        CandidateStore::new(&store)
            .load(&candidate.id)
            .unwrap()
            .integration(&vcs)
            .unwrap(),
        IntegrationStatus::Pending
    );
}

#[test]
fn candidate_branch_is_informational_after_registration() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    vcs.refs.borrow_mut().remove("refs/heads/candidates/a");
    let verification = conclude(
        &store,
        &vcs,
        &candidate,
        crate::VerificationOutcome::Accepted,
    );

    CandidateStore::new(&store)
        .accept(
            &vcs,
            &candidate.id,
            &verification,
            Author::Human,
            String::new(),
        )
        .unwrap();
}

#[test]
fn an_exact_result_has_only_one_candidate() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    let error = CandidateStore::new(&store)
        .register(
            &vcs,
            NewCandidate {
                node,
                branch: "candidates/other".into(),
                target: "main".into(),
                external: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains(&candidate.id.0), "{error:#}");
}

#[test]
fn publication_is_derived_and_target_corruption_is_detected() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    let candidates = CandidateStore::new(&store);
    let verification = conclude(
        &store,
        &vcs,
        &candidate,
        crate::VerificationOutcome::Accepted,
    );
    candidates
        .accept(
            &vcs,
            &candidate.id,
            &verification,
            Author::Human,
            String::new(),
        )
        .unwrap();
    assert_eq!(
        candidates
            .load(&candidate.id)
            .unwrap()
            .integration(&vcs)
            .unwrap(),
        IntegrationStatus::Accepted
    );
    vcs.refs
        .borrow_mut()
        .insert("refs/heads/main".into(), "unrelated".into());
    let error = candidates
        .load(&candidate.id)
        .unwrap()
        .integration(&vcs)
        .unwrap_err();
    assert!(
        error.to_string().contains("without containing"),
        "{error:#}"
    );
}

#[test]
fn verification_has_a_review_outcome_and_candidate_lineage() {
    let (_temp, store, vcs, source, output) = successful_output();
    let candidate = register(&store, &vcs, &source);

    let verification = ops::add_verification(
        &store,
        &vcs,
        &candidate.id,
        NewNode {
            description: "Verify the candidate".into(),
            author: Author::Human,
            assignee: Some(Author::Machine),
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap();

    let (meta, _) = store.read_node(&verification).unwrap();
    assert_eq!(meta.verifies.as_ref(), Some(&candidate.id));
    assert_eq!(meta.derived_from, vec![source.clone()]);
    assert_eq!(
        ops::verifications_for(&store, &candidate.id).unwrap(),
        vec![verification.clone()]
    );
    assert!(ops::node_state(&store, &vcs, &verification)
        .unwrap()
        .is_ready());

    let snapshot = ops::snapshot_work(&store, &vcs, &verification, &[]).unwrap();
    ops::submit_verification(
        &store,
        &vcs,
        crate::VerificationSubmission {
            snapshot,
            outcome: crate::VerificationOutcome::Accepted,
            notes: "candidate is valid".into(),
            author: Author::Machine,
            producer: None,
        },
    )
    .unwrap();
    let (result, _) = store.read_result(&verification).unwrap().unwrap();
    assert_eq!(
        result.outcome,
        crate::ResultOutcome::Verification(crate::VerificationOutcome::Accepted)
    );
    assert_eq!(result.consumed.len(), 1);
    assert_eq!(result.consumed[0].id, source);
    assert_eq!(
        result.consumed[0].output.as_ref().map(|item| &item.id),
        Some(&output)
    );
    assert!(ops::check(&store).unwrap().is_empty());
}

#[test]
fn completed_verification_becomes_stale_when_its_source_is_reworked() {
    let (_temp, store, mut vcs, source, _) = successful_output();
    let candidate = register(&store, &vcs, &source);
    let verification = ops::add_verification(
        &store,
        &vcs,
        &candidate.id,
        NewNode {
            description: "Verify the candidate".into(),
            author: Author::Human,
            assignee: Some(Author::Machine),
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap();
    let snapshot = ops::snapshot_work(&store, &vcs, &verification, &[]).unwrap();
    ops::submit_verification(
        &store,
        &vcs,
        crate::VerificationSubmission {
            snapshot,
            outcome: crate::VerificationOutcome::Rejected,
            notes: "candidate requires rework".into(),
            author: Author::Machine,
            producer: None,
        },
    )
    .unwrap();
    assert!(ops::staleness(&store, &vcs, &verification)
        .unwrap()
        .is_empty());

    ops::edit(
        &store,
        &vcs,
        source.as_str(),
        "candidate work revised".into(),
    )
    .unwrap();
    vcs.next_id = "replacement-output".into();
    ops::complete(
        &store,
        &vcs,
        source.as_str(),
        &["out.txt".into()],
        &[],
        None,
        "reworked",
        Author::Machine,
    )
    .unwrap();

    let reasons = ops::staleness(&store, &vcs, &verification).unwrap();
    assert!(
        reasons.contains(&crate::StalenessReason::ConsumedOutputChanged {
            id: source.to_string()
        })
    );
    let state = ops::node_state(&store, &vcs, &verification).unwrap();
    assert_eq!(state.currency, crate::Currency::Stale);
    assert!(!state.is_complete());
}

#[test]
fn check_detects_verification_without_candidate_source_lineage() {
    let (_temp, store, vcs, source, _) = successful_output();
    let candidate = register(&store, &vcs, &source);
    let verification = ops::add_verification(
        &store,
        &vcs,
        &candidate.id,
        NewNode {
            description: "Verify the candidate".into(),
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap();
    let (mut meta, description) = store.read_node(&verification).unwrap();
    meta.derived_from.clear();
    store
        .write_node(&verification, &meta, &description)
        .unwrap();

    let problems = ops::check(&store).unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("does not derive from its source node")),
        "{problems:?}"
    );
}
