//! End-to-end coordination of a Linka candidate and a Git-only Nota review.

mod common;

use common::*;
use linka::{Author, CandidateStore, NewCandidate};
use orka::review::{FinishOutcome, ReviewVerdict, Reviews};

fn candidate(root: &std::path::Path) -> linka::CandidateRecord {
    let project = root.join("project");
    let node = add_node(root, "Implement the reviewed change", vec![]);
    std::fs::write(project.join("answer.txt"), "answer\n").unwrap();
    complete_node(root, &node, &["answer.txt".into()], "implemented");
    let store = store_at(root);
    let vcs = linka::GitVcs::for_store(&store);
    let output = linka::ops::output_of(&store, &node).unwrap().unwrap();
    git(&project, &["branch", "candidate/review", &output]);
    let target = git(&project, &["branch", "--show-current"]);
    CandidateStore::new(&store)
        .register(
            &vcs,
            NewCandidate {
                node: node.parse().unwrap(),
                branch: "candidate/review".into(),
                target,
                external: None,
            },
        )
        .unwrap()
}

#[test]
fn nota_review_completes_a_linka_verification_without_nota_knowing_linka() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));

    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    assert_eq!(started.record.candidate, candidate.id);
    assert_eq!(started.record.subject, candidate.artifact.id);
    assert_eq!(started.review.subject, candidate.artifact.id);
    assert_eq!(started.review.branch, started.record.branch);
    assert_eq!(
        git(
            &root.join("project"),
            &["rev-parse", &format!("{}^", started.review.marker)]
        ),
        candidate.artifact.id
    );

    let (meta, _) = store
        .read_node(started.record.verification.as_str())
        .unwrap();
    assert_eq!(meta.verifies.as_ref(), Some(&candidate.id));
    assert_eq!(meta.derived_from, vec![candidate.node.clone()]);

    let review_tree = root.join("review-worktree");
    git(
        &root.join("project"),
        &[
            "worktree",
            "add",
            review_tree.to_str().unwrap(),
            &started.review.branch,
        ],
    );
    let note = nota::add_note(&review_tree, "The candidate looks correct.").unwrap();

    assert_eq!(
        reviews
            .finish(
                &started.record.verification,
                ReviewVerdict::Approved,
                None,
                Author::Human,
            )
            .unwrap(),
        FinishOutcome::Submitted
    );
    let (result, notes) = store
        .read_result(started.record.verification.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome, linka::Outcome::Done);
    assert!(result.output.is_none());
    assert!(notes.contains("Review verdict: approved"));
    let producer = result.producer.unwrap();
    assert_eq!(producer.namespace, "orka.nota");
    assert_eq!(producer.data["candidate"], candidate.id.0);
    assert_eq!(producer.data["head"], note.commit);

    let vcs = linka::GitVcs::for_store(&store);
    assert!(
        linka::ops::node_state(&store, &vcs, started.record.verification.as_str())
            .unwrap()
            .is_complete()
    );
    assert_eq!(
        reviews
            .finish(
                &started.record.verification,
                ReviewVerdict::Approved,
                None,
                Author::Human,
            )
            .unwrap(),
        FinishOutcome::AlreadySubmitted
    );
}

#[test]
fn a_source_change_during_review_is_a_submission_conflict() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();

    let vcs = linka::GitVcs::for_store(&store);
    CandidateStore::new(&store)
        .reject(&vcs, &candidate.id, Author::Human, "requires rework".into())
        .unwrap();
    linka::ops::edit(
        &store,
        &vcs,
        candidate.node.as_str(),
        "Implement the reviewed change differently".into(),
    )
    .unwrap();

    let outcome = reviews
        .finish(
            &started.record.verification,
            ReviewVerdict::Commented,
            None,
            Author::Human,
        )
        .unwrap();
    assert!(matches!(
        outcome,
        FinishOutcome::Conflict(ref conflicts)
            if conflicts.contains(&linka::SubmissionConflict::LineageChanged)
    ));
    assert!(store
        .read_result(started.record.verification.as_str())
        .unwrap()
        .is_none());
}
