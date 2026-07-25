//! End-to-end coordination of a Linka candidate and a Git-only Nota review.

mod common;

use common::*;
use linka::{Author, CandidateStore, NewCandidate, VerificationOutcome};
use orka::review::{AbandonOutcome, FinishOutcome, Reviews};
use orka::review_worktree::{GitReviewWorktrees, ReviewCleanupOutcome};
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
                VerificationOutcome::Accepted,
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
    assert_eq!(
        result.outcome,
        linka::ResultOutcome::Verification(VerificationOutcome::Accepted)
    );
    assert!(result.output.is_none());
    assert!(notes.contains("Review outcome: accepted"));
    let producer = result.producer.unwrap();
    assert_eq!(producer.namespace, "orka.nota");
    assert_eq!(producer.data["candidate"], candidate.id.0);
    assert_eq!(producer.data["head"], note.commit);
    assert_eq!(producer.data["outcome"], "accepted");

    let vcs = linka::GitVcs::for_store(&store);
    assert_eq!(
        CandidateStore::new(&store)
            .load(&candidate.id)
            .unwrap()
            .integration(&vcs)
            .unwrap(),
        linka::IntegrationStatus::Published
    );
    assert!(
        linka::ops::node_state(&store, &vcs, started.record.verification.as_str())
            .unwrap()
            .is_complete()
    );
    assert_eq!(
        reviews
            .finish(
                &started.record.verification,
                VerificationOutcome::Accepted,
                None,
                Author::Human,
            )
            .unwrap(),
        FinishOutcome::AlreadySubmitted
    );
}

#[test]
fn rejected_review_rejects_the_exact_candidate_and_reopens_its_source() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();

    assert_eq!(
        reviews
            .finish(
                &started.record.verification,
                VerificationOutcome::Rejected,
                Some("The implementation misses the edge case."),
                Author::Human,
            )
            .unwrap(),
        FinishOutcome::Submitted
    );

    let vcs = linka::GitVcs::for_store(&store);
    let candidate = CandidateStore::new(&store).load(&candidate.id).unwrap();
    assert_eq!(
        candidate.integration(&vcs).unwrap(),
        linka::IntegrationStatus::Rejected
    );
    assert!(
        linka::ops::node_state(&store, &vcs, candidate.node.as_str())
            .unwrap()
            .is_ready()
    );
}

#[test]
fn verification_submission_atomically_decides_the_candidate() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    let review = nota::load_review_ref(&root.join("project"), &started.record.branch).unwrap();
    let producer = linka::ProducerEvidence {
        namespace: "orka.nota".into(),
        data: serde_json::json!({
            "candidate": candidate.id.0,
            "verification": started.record.verification.as_str(),
            "branch": review.branch,
            "marker": review.marker,
            "head": review.marker,
            "outcome": "accepted",
        }),
    };
    let vcs = linka::GitVcs::for_store(&store);
    linka::ops::submit_verification(
        &store,
        &vcs,
        linka::VerificationSubmission {
            snapshot: started.record.snapshot.clone(),
            outcome: VerificationOutcome::Accepted,
            notes: "Review outcome: accepted".into(),
            author: Author::Human,
            producer: Some(producer),
        },
    )
    .unwrap();
    assert_ne!(
        CandidateStore::new(&store)
            .load(&candidate.id)
            .unwrap()
            .integration(&vcs)
            .unwrap(),
        linka::IntegrationStatus::Pending
    );

    assert_eq!(
        reviews
            .finish(
                &started.record.verification,
                VerificationOutcome::Accepted,
                None,
                Author::Human,
            )
            .unwrap(),
        FinishOutcome::AlreadySubmitted
    );
    assert_ne!(
        CandidateStore::new(&store)
            .load(&candidate.id)
            .unwrap()
            .integration(&vcs)
            .unwrap(),
        linka::IntegrationStatus::Pending
    );
}

#[test]
fn finishing_a_review_rejects_an_invalid_git_suggestion() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();
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
    std::fs::create_dir_all(review_tree.join(".nota")).unwrap();
    std::fs::write(review_tree.join(".nota/metadata"), "invalid\n").unwrap();
    git(&review_tree, &["add", "--force", ".nota/metadata"]);
    git(
        &review_tree,
        &["commit", "--quiet", "-m", "invalid suggestion"],
    );

    let error = reviews
        .finish(
            &started.record.verification,
            VerificationOutcome::Rejected,
            None,
            Author::Human,
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("may not contain Nota files"));
    assert!(store
        .read_result(started.record.verification.as_str())
        .unwrap()
        .is_none());
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
fn managed_review_worktrees_are_reused_inspected_and_safely_cleaned() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    let worktrees =
        GitReviewWorktrees::new(root.join("project"), root.join(".orka/review-worktrees"));

    let prepared = worktrees.prepare(&started.record).unwrap();
    assert_eq!(
        prepared.path,
        root.join(".orka/review-worktrees")
            .join(started.record.verification.as_str())
    );
    assert_eq!(
        git(&prepared.path, &["branch", "--show-current"]),
        started.record.branch
    );
    assert_eq!(worktrees.prepare(&started.record).unwrap(), prepared);

    let listed = worktrees.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].verification, started.record.verification);
    assert_eq!(listed[0].path, prepared.path);
    assert!(!listed[0].dirty);

    std::fs::write(prepared.path.join("scratch.txt"), "not committed\n").unwrap();
    assert!(worktrees.list().unwrap()[0].dirty);
    assert_eq!(
        worktrees.cleanup(&started.record).unwrap(),
        ReviewCleanupOutcome::RetainedDirty
    );
    assert!(prepared.path.exists());

    std::fs::remove_file(prepared.path.join("scratch.txt")).unwrap();
    assert_eq!(
        worktrees.cleanup(&started.record).unwrap(),
        ReviewCleanupOutcome::Removed
    );
    assert!(!prepared.path.exists());
    assert!(!git(
        &root.join("project"),
        &["branch", "--list", &started.record.branch]
    )
    .is_empty());
    assert_eq!(
        worktrees.cleanup(&started.record).unwrap(),
        ReviewCleanupOutcome::AlreadyAbsent
    );

    git(
        &root.join("project"),
        &[
            "worktree",
            "add",
            "-b",
            "unrelated/review",
            prepared.path.to_str().unwrap(),
            "HEAD",
        ],
    );
    let error = worktrees.prepare(&started.record).unwrap_err();
    assert!(format!("{error:#}").contains("expected `nota/"));
    assert!(
        prepared.path.exists(),
        "a mismatched tree must not be removed"
    );
}

#[test]
fn cli_enter_prepares_the_managed_tree_and_prints_its_path() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let output = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "start",
            &candidate.id.0,
            "--enter",
        ])
        // A deliberately invalid shell proves that `--enter` does not launch
        // a process; it only prepares the tree and reports its path.
        .env("SHELL", root.join("does-not-exist"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let record = reviews.list().unwrap().pop().unwrap();
    let expected_path = root
        .join(".orka/review-worktrees")
        .join(record.verification.as_str());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .last()
            .unwrap(),
        expected_path.to_str().unwrap()
    );
    assert!(expected_path.is_dir());

    let enter_output = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "enter",
            record.verification.as_str(),
        ])
        .env("SHELL", root.join("does-not-exist"))
        .output()
        .unwrap();
    assert!(enter_output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&enter_output.stdout).trim(),
        expected_path.to_str().unwrap()
    );

    let path_output = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "worktree",
            record.verification.as_str(),
            "--print-path",
        ])
        .output()
        .unwrap();
    assert!(path_output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&path_output.stdout).trim(),
        expected_path.to_str().unwrap()
    );
}

#[test]
fn audit_reports_stale_and_unregistered_managed_review_worktrees() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    let worktrees =
        GitReviewWorktrees::new(store.project_root(), root.join(".orka/review-worktrees"));
    let prepared = worktrees.prepare(&started.record).unwrap();
    assert!(worktrees.audit(&reviews).unwrap().is_empty());

    std::fs::remove_dir_all(&prepared.path).unwrap();
    let problems = worktrees.audit(&reviews).unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("stale Git worktree registration")),
        "{problems:?}"
    );

    git(&store.project_root(), &["worktree", "prune"]);
    std::fs::create_dir_all(&prepared.path).unwrap();
    let problems = worktrees.audit(&reviews).unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("is not registered with Git")),
        "{problems:?}"
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
    assert_eq!(
        result.outcome,
        linka::ResultOutcome::Verification(VerificationOutcome::Abandoned)
    );
    assert_eq!(notes, "review is no longer needed");
    let producer = result.producer.unwrap();
    assert_eq!(producer.namespace, "orka.nota");
    assert_eq!(producer.data["status"], "abandoned");
    assert_eq!(producer.data["candidate"], candidate.id.0);
    assert_eq!(
        CandidateStore::new(&store)
            .load(&candidate.id)
            .unwrap()
            .integration(&linka::GitVcs::for_store(&store))
            .unwrap(),
        linka::IntegrationStatus::Pending
    );
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
fn audit_reports_a_verification_left_without_its_review_binding() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();
    assert!(reviews.audit().unwrap().is_empty());

    std::fs::remove_file(
        root.join(".orka")
            .join("reviews")
            .join(started.record.verification.as_str())
            .join("review.toml"),
    )
    .unwrap();

    let problems = reviews.audit().unwrap();
    let all = problems.join("\n");
    assert!(all.contains("review binding record is missing"), "{all}");
    assert!(
        all.contains("Orka verification has no durable review binding"),
        "{all}"
    );
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
fn cli_lists_active_reviews_and_abandons_one() {
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

    let abandoned = Command::new(binary)
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "abandon",
            started.record.verification.as_str(),
            "--notes",
            "abandoned from the CLI",
        ])
        .output()
        .unwrap();
    assert!(
        abandoned.status.success(),
        "{}",
        String::from_utf8_lossy(&abandoned.stderr)
    );
    assert!(String::from_utf8_lossy(&abandoned.stdout).contains("abandoned"));
    assert!(reviews.list().unwrap().is_empty());
}

#[test]
fn cli_finish_applies_an_accepted_review_to_the_candidate() {
    let (_temp, root) = workbench();
    let candidate = candidate(&root);
    let store = store_at(&root);
    let reviews = Reviews::new(&store, root.join(".orka"));
    let started = reviews.start(&candidate.id, Author::Human).unwrap();

    let finished = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "finish",
            started.record.verification.as_str(),
            "--outcome",
            "accepted",
            "--summary",
            "Reviewed through the CLI.",
        ])
        .output()
        .unwrap();
    assert!(
        finished.status.success(),
        "{}",
        String::from_utf8_lossy(&finished.stderr)
    );
    assert!(String::from_utf8_lossy(&finished.stdout).contains("completed"));

    assert_ne!(
        CandidateStore::new(&store)
            .load(&candidate.id)
            .unwrap()
            .integration(&linka::GitVcs::for_store(&store))
            .unwrap(),
        linka::IntegrationStatus::Pending
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
            VerificationOutcome::Accepted,
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

    let cli = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "finish",
            started.record.verification.as_str(),
            "--outcome",
            "accepted",
        ])
        .output()
        .unwrap();
    assert!(!cli.status.success());
    assert!(String::from_utf8_lossy(&cli.stderr).contains("stale"));

    let abandon = Command::new(env!("CARGO_BIN_EXE_orka"))
        .args([
            "--workbench",
            root.to_str().unwrap(),
            "review",
            "abandon",
            started.record.verification.as_str(),
        ])
        .output()
        .unwrap();
    assert!(!abandon.status.success());
    assert!(String::from_utf8_lossy(&abandon.stderr).contains("stale"));
}
