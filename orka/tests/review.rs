//! End-to-end coordination of a Linka candidate and a Git-only Nota review.

mod common;

use common::*;
use linka::{Author, CandidateStore, NewCandidate};
use orka::review::{AbandonOutcome, FinishOutcome, ReviewVerdict, Reviews};
use std::process::Command;

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
fn starting_a_review_twice_resumes_the_only_review_for_the_candidate() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));

    let first = reviews.start(&candidate.id, Author::Human).unwrap();
    let second = reviews.start(&candidate.id, Author::Machine).unwrap();

    assert_eq!(second.record, first.record);
    assert_eq!(second.review.branch, first.review.branch);
    assert_eq!(second.review.marker, first.review.marker);
    assert_eq!(second.review.subject, first.review.subject);
    assert_eq!(
        linka::ops::verifications_for(&store, &candidate.id).unwrap(),
        vec![first.record.verification.to_string()]
    );
}

#[test]
fn active_reviews_can_be_listed_and_abandoned_without_removing_nota_evidence() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));

    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    assert_eq!(reviews.list().unwrap(), vec![started.record.clone()]);

    assert_eq!(
        reviews
            .abandon(
                &started.record.verification,
                Some("review is no longer needed"),
                Author::Human,
            )
            .unwrap(),
        AbandonOutcome::Abandoned
    );
    assert!(reviews.list().unwrap().is_empty());
    let (result, notes) = store
        .read_result(started.record.verification.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome, linka::Outcome::Failed);
    assert_eq!(notes, "review is no longer needed");
    let producer = result.producer.unwrap();
    assert_eq!(producer.namespace, "orka.nota");
    assert_eq!(producer.data["status"], "abandoned");
    assert_eq!(producer.data["candidate"], candidate.id.0);
    assert!(nota::load_review_ref(&root.join("project"), &started.record.branch).is_ok());

    assert_eq!(
        reviews
            .abandon(&started.record.verification, None, Author::Human)
            .unwrap(),
        AbandonOutcome::AlreadyAbandoned
    );

    let restarted = reviews.start(&candidate.id, Author::Human).unwrap();
    assert_ne!(restarted.record.verification, started.record.verification);
    assert_eq!(reviews.list().unwrap(), vec![restarted.record]);
}

#[test]
fn a_review_can_be_abandoned_when_nota_branch_creation_was_interrupted() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    git(
        &root.join("project"),
        &["branch", "-D", &started.record.branch],
    );

    assert_eq!(
        reviews
            .abandon(&started.record.verification, None, Author::Human)
            .unwrap(),
        AbandonOutcome::Abandoned
    );
    assert!(reviews.list().unwrap().is_empty());
}

#[test]
fn cli_lists_active_reviews_and_accepts_stop_as_an_abandon_alias() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    let binary = env!("CARGO_BIN_EXE_orka");

    let listed = Command::new(binary)
        .args(["--workbench", root.to_str().unwrap(), "review", "list"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains(started.record.verification.as_str()));
    assert!(stdout.contains(candidate.id.0.as_str()));
    assert!(stdout.contains(&started.record.branch));

    let stopped = Command::new(binary)
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "stop",
            started.record.verification.as_str(),
            "--notes",
            "stopped from the CLI",
        ])
        .output()
        .unwrap();
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("abandoned"));
    assert!(reviews.list().unwrap().is_empty());
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
