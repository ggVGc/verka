//! Behaviour of the operations and the state they produce.
//!
//! Because state is a pure function of records, the load-bearing properties
//! are asserted directly rather than inferred from the operations' return
//! values: complete implies not ready, ready implies no blockers, a definition
//! edit makes every covering result stale, a submission against a stale
//! snapshot never writes, and one unreadable record leaves every other node's
//! state untouched.

use super::*;
use crate::check::check;
use crate::graph::Graph;
use crate::model::{
    ArtifactRef, Attachment, Blocker, BlockerReason, Candidate, CandidateDecision, Conclusion,
    Currency, DepKind, ExternalIdentity, IntegrationStatus, NewAttachment, NewCandidate,
    RecordedOutcome, StalenessReason, Submission, SubmissionConflict, Workability,
};
use crate::vcs::FakeVcs;

struct Workbench {
    root: std::path::PathBuf,
    store: Store,
    vcs: FakeVcs,
}

impl Drop for Workbench {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Workbench {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("linka-ops-{}", ulid::Ulid::new()));
        let store = Store::init(root.join(".linka")).unwrap();
        let vcs = FakeVcs {
            next_id: "commit-1".into(),
            ..FakeVcs::default()
        };
        Self { root, store, vcs }
    }

    fn graph(&self) -> Graph<'_> {
        Graph::load(&self.store, &self.vcs).unwrap()
    }

    fn state(&self, id: &NodeId) -> crate::model::NodeState {
        self.graph().state(id).clone()
    }

    fn workability(&self, id: &NodeId) -> Workability {
        self.state(id).workability()
    }

    fn add(&self, description: &str) -> NodeId {
        add(&self.store, &self.vcs, node(description), None).unwrap()
    }

    fn add_after(&self, description: &str, dependency: &NodeId) -> NodeId {
        add(
            &self.store,
            &self.vcs,
            NewNode {
                depends_on: vec![dependency.clone()],
                ..node(description)
            },
            None,
        )
        .unwrap()
    }

    /// Record a plain successful result with no project output.
    fn finish(&self, id: &NodeId) {
        self.conclude(id, Conclusion::Done { output: None }, "done")
            .unwrap();
    }

    fn conclude(
        &self,
        id: &NodeId,
        conclusion: Conclusion,
        notes: &str,
    ) -> std::result::Result<(), SubmissionError> {
        let snapshot = snapshot(&self.store, &self.vcs, id, &[])?;
        submit(
            &self.store,
            &self.vcs,
            Submission {
                snapshot,
                conclusion,
                notes: notes.into(),
                author: Author::Machine,
                producer: None,
                attachments: Vec::new(),
            },
        )
    }

    /// Record a successful result that produced `commit` in the project.
    fn produce(&self, id: &NodeId, commit: &str) {
        self.vcs.commit(commit, Some("base"));
        self.conclude(
            id,
            Conclusion::Done {
                output: Some(artifact(commit)),
            },
            "produced",
        )
        .unwrap();
    }

    fn write_project_file(&self, path: &str, content: &str) {
        let file = self.store.project_root().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
}

fn node(description: &str) -> NewNode {
    NewNode {
        description: description.into(),
        author: Author::Human,
        assignee: None,
        depends_on: Vec::new(),
        derived_from: Vec::new(),
    }
}

fn artifact(commit: &str) -> ArtifactRef {
    ArtifactRef {
        scheme: "git-commit".into(),
        // The workbench is unpaired, so the project identity is empty.
        repository: String::new(),
        id: commit.into(),
    }
}

fn conflicts(error: SubmissionError) -> Vec<crate::model::SubmissionConflict> {
    match error {
        SubmissionError::Conflict(conflicts) => conflicts,
        SubmissionError::Evaluation(error) => panic!("expected a conflict, got {error:#}"),
    }
}

// --- the fact writers ----------------------------------------------------------

#[test]
fn a_node_starts_ready_and_its_dependents_start_blocked() {
    let bench = Workbench::new();
    let first = bench.add("write the parser");
    let second = bench.add_after("use the parser", &first);

    assert_eq!(bench.workability(&first), Workability::Ready);
    assert_eq!(bench.workability(&second), Workability::Blocked);
    assert_eq!(
        bench.state(&second).blockers(),
        [Blocker {
            id: first.clone(),
            reason: BlockerReason::Open
        }]
    );

    bench.finish(&first);
    assert_eq!(bench.workability(&first), Workability::Complete);
    assert_eq!(bench.workability(&second), Workability::Ready);
}

#[test]
fn editing_a_description_to_what_it_already_says_moves_nothing() {
    let bench = Workbench::new();
    let id = bench.add("stable");
    let before = bench.store.node_version(&id).unwrap();
    let commits = *bench.vcs.store_commits.borrow();

    assert_eq!(
        edit(&bench.store, &bench.vcs, &id, "stable".into()).unwrap(),
        EditOutcome::Unchanged
    );
    assert_eq!(bench.store.node_version(&id).unwrap(), before);
    assert_eq!(*bench.vcs.store_commits.borrow(), commits);

    assert_eq!(
        edit(&bench.store, &bench.vcs, &id, "moved".into()).unwrap(),
        EditOutcome::Edited
    );
    assert_ne!(bench.store.node_version(&id).unwrap(), before);
}

#[test]
fn an_edit_makes_every_result_that_covered_the_old_definition_stale() {
    let bench = Workbench::new();
    let first = bench.add("write the parser");
    let second = bench.add_after("use the parser", &first);
    bench.finish(&first);
    bench.finish(&second);
    assert_eq!(bench.workability(&second), Workability::Complete);

    edit(
        &bench.store,
        &bench.vcs,
        &first,
        "write a better parser".into(),
    )
    .unwrap();

    // The edited node's own result no longer covers its definition...
    let first_state = bench.state(&first);
    assert_eq!(first_state.currency(), Some(Currency::Stale));
    assert!(matches!(
        first_state.staleness(),
        [StalenessReason::DefinitionChanged { .. }]
    ));
    assert_eq!(first_state.workability(), Workability::Ready);
    // ...and the dependent's pin of it does not either.
    let second_state = bench.state(&second);
    assert_eq!(
        second_state.staleness(),
        [StalenessReason::ConsumedDefinitionChanged { id: first.clone() }]
    );
    assert_eq!(second_state.workability(), Workability::Blocked);
}

#[test]
fn a_failure_can_be_recorded_for_work_that_is_not_ready() {
    let bench = Workbench::new();
    let first = bench.add("write the parser");
    let second = bench.add_after("use the parser", &first);
    assert_eq!(bench.workability(&second), Workability::Blocked);

    bench
        .conclude(&second, Conclusion::Failed, "the parser is not there yet")
        .unwrap();

    let state = bench.state(&second);
    assert_eq!(state.outcome(), Some(RecordedOutcome::Failed));
    // Failure is evidence, not completion: the node is still work to do.
    assert_eq!(state.workability(), Workability::Blocked);
    bench.finish(&first);
    assert_eq!(bench.workability(&second), Workability::Ready);
}

#[test]
fn success_is_refused_for_work_whose_dependencies_are_not_complete() {
    let bench = Workbench::new();
    let first = bench.add("write the parser");
    let second = bench.add_after("use the parser", &first);

    let error = bench
        .conclude(&second, Conclusion::Done { output: None }, "claimed")
        .unwrap_err();

    assert_eq!(conflicts(error), [SubmissionConflict::ReadinessChanged]);
    assert!(bench.store.read_result(&second).unwrap().is_none());
}

#[test]
fn a_submission_against_a_stale_snapshot_conflicts_and_writes_nothing() {
    let bench = Workbench::new();
    let id = bench.add("write the parser");
    let frozen = snapshot(&bench.store, &bench.vcs, &id, &[]).unwrap();
    edit(
        &bench.store,
        &bench.vcs,
        &id,
        "write a different parser".into(),
    )
    .unwrap();

    let error = submit(
        &bench.store,
        &bench.vcs,
        Submission {
            snapshot: frozen,
            conclusion: Conclusion::Done { output: None },
            notes: "done".into(),
            author: Author::Machine,
            producer: None,
            attachments: Vec::new(),
        },
    )
    .unwrap_err();

    assert_eq!(conflicts(error), [SubmissionConflict::DefinitionChanged]);
    assert!(bench.store.read_result(&id).unwrap().is_none());
}

#[test]
fn changed_context_conflicts_and_contributes_to_staleness() {
    let bench = Workbench::new();
    let id = bench.add("read the config");
    bench.write_project_file("config.toml", "one\n");

    let frozen = snapshot(&bench.store, &bench.vcs, &id, &["config.toml".into()]).unwrap();
    bench.write_project_file("config.toml", "two\n");
    let error = submit(
        &bench.store,
        &bench.vcs,
        Submission {
            snapshot: frozen,
            conclusion: Conclusion::Done { output: None },
            notes: "done".into(),
            author: Author::Machine,
            producer: None,
            attachments: Vec::new(),
        },
    )
    .unwrap_err();
    assert_eq!(
        conflicts(error),
        [SubmissionConflict::ContextChanged {
            path: "config.toml".parse().unwrap()
        }]
    );

    // Pinned and then changed afterwards: staleness rather than a conflict.
    let snapshot = snapshot(&bench.store, &bench.vcs, &id, &["config.toml".into()]).unwrap();
    submit(
        &bench.store,
        &bench.vcs,
        Submission {
            snapshot,
            conclusion: Conclusion::Done { output: None },
            notes: "done".into(),
            author: Author::Machine,
            producer: None,
            attachments: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(bench.workability(&id), Workability::Complete);
    bench.write_project_file("config.toml", "three\n");
    assert_eq!(
        bench.state(&id).staleness(),
        [StalenessReason::ContextChanged {
            path: "config.toml".parse().unwrap()
        }]
    );
}

#[test]
fn observed_context_is_pinned_for_one_result_and_ages_with_it() {
    let bench = Workbench::new();
    let id = bench.add("read something");
    bench.write_project_file("notes.md", "read me\n");
    bench.finish(&id);
    let version = bench.store.result_version(&id).unwrap();

    let added = record_observed_context(
        &bench.store,
        &bench.vcs,
        &id,
        &version,
        &["notes.md".into(), "missing.md".into()],
    )
    .unwrap();
    assert_eq!(added, 1);
    assert_eq!(bench.workability(&id), Workability::Complete);

    bench.write_project_file("notes.md", "changed\n");
    assert_eq!(
        bench.state(&id).staleness(),
        [StalenessReason::ContextChanged {
            path: "notes.md".parse().unwrap()
        }]
    );

    // Recording against a result the node no longer has is refused.
    bench.finish(&id);
    assert!(record_observed_context(
        &bench.store,
        &bench.vcs,
        &id,
        &version,
        &["notes.md".into()]
    )
    .is_err());
}

#[test]
fn attachments_are_idempotent_and_never_reach_graph_state() {
    let bench = Workbench::new();
    let id = bench.add("attach evidence");
    let before = bench.state(&id);
    let new = || NewAttachment {
        namespace: "orka".parse().unwrap(),
        key: "transcript".parse().unwrap(),
        media_type: Some("text/plain".into()),
        data: b"what happened".to_vec(),
    };

    let recorded = attach(&bench.store, &bench.vcs, &id, vec![new()]).unwrap();
    assert_eq!(recorded.len(), 1);
    let commits = *bench.vcs.store_commits.borrow();

    // The same bytes again: no second record, no second commit.
    assert_eq!(
        attach(&bench.store, &bench.vcs, &id, vec![new()]).unwrap(),
        recorded
    );
    assert_eq!(*bench.vcs.store_commits.borrow(), commits);
    assert_eq!(bench.store.list_attachments(&id).unwrap().len(), 1);

    // Different bytes under the same key: refused.
    let error = attach(
        &bench.store,
        &bench.vcs,
        &id,
        vec![NewAttachment {
            data: b"something else".to_vec(),
            ..new()
        }],
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("different content"),
        "{error:#}"
    );
    assert_eq!(bench.state(&id), before);
}

// --- candidates and verification ---------------------------------------------------

/// A node whose successful output is proposed as a candidate, plus the review
/// node for it. The project's `main` sits at `base`, and the artifact is a
/// child of it, so publication is a real fast-forward.
fn candidate_bench() -> (Workbench, NodeId, Candidate, NodeId) {
    let bench = Workbench::new();
    bench.vcs.commit("base", None);
    bench.vcs.set_ref("refs/heads/main", "base");
    let source = bench.add("produce something");
    bench.produce(&source, "commit-1");

    let candidate = register_candidate(
        &bench.store,
        &bench.vcs,
        NewCandidate {
            node: source.clone(),
            branch: "work/produce".into(),
            target: "main".into(),
            external: None,
        },
    )
    .unwrap();
    let review = add(
        &bench.store,
        &bench.vcs,
        node("review the output"),
        Some(candidate.id.clone()),
    )
    .unwrap();
    (bench, source, candidate, review)
}

#[test]
fn a_pending_candidate_holds_its_source_node_out_of_the_ready_set() {
    let (bench, source, candidate, review) = candidate_bench();

    let state = bench.state(&source);
    assert_eq!(state.integration(), Some(IntegrationStatus::Pending));
    assert_eq!(state.workability(), Workability::AwaitingIntegration);
    assert!(!bench.graph().ready(None).contains(&&source));
    // The review derives from the source node, so it pins the exact artifact.
    let meta = bench.store.read_node(&review).unwrap().0;
    assert_eq!(meta.derived_from, vec![source]);
    assert_eq!(meta.verifies, Some(candidate.id));
}

#[test]
fn accepting_a_candidate_awaits_publication_and_then_completes_the_source() {
    let (bench, source, candidate, review) = candidate_bench();

    bench
        .conclude(&review, Conclusion::Accepted, "looks right")
        .unwrap();

    assert_eq!(
        bench.graph().decision(&candidate.id).unwrap(),
        CandidateDecision::Accepted
    );
    let state = bench.state(&source);
    assert_eq!(state.integration(), Some(IntegrationStatus::Accepted));
    assert_eq!(state.workability(), Workability::AwaitingIntegration);
    // The review itself is finished work.
    assert_eq!(bench.workability(&review), Workability::Complete);

    publish(&bench.vcs, &candidate).unwrap();

    assert_eq!(
        bench.vcs.ref_commit("refs/heads/main").unwrap().as_deref(),
        Some("commit-1")
    );
    let state = bench.state(&source);
    assert_eq!(state.integration(), Some(IntegrationStatus::Published));
    assert_eq!(state.workability(), Workability::Complete);
    // Publication is idempotent: it is re-derived from ancestry, not journalled.
    publish(&bench.vcs, &candidate).unwrap();
}

#[test]
fn rejecting_a_candidate_returns_the_source_node_to_ready() {
    let (bench, source, candidate, review) = candidate_bench();

    bench
        .conclude(&review, Conclusion::Rejected, "wrong approach")
        .unwrap();

    assert_eq!(
        bench.graph().decision(&candidate.id).unwrap(),
        CandidateDecision::Rejected
    );
    let state = bench.state(&source);
    assert_eq!(state.integration(), Some(IntegrationStatus::Rejected));
    assert_eq!(state.workability(), Workability::Ready);
    assert!(bench.graph().ready(None).contains(&&source));
}

#[test]
fn an_abandoned_review_decides_nothing() {
    let (bench, source, candidate, review) = candidate_bench();

    bench
        .conclude(&review, Conclusion::Abandoned, "no reviewer available")
        .unwrap();

    assert_eq!(
        bench.graph().decision(&candidate.id).unwrap(),
        CandidateDecision::Pending
    );
    assert_eq!(
        bench.state(&source).integration(),
        Some(IntegrationStatus::Pending)
    );
    assert_eq!(bench.workability(&review), Workability::Complete);
}

#[test]
fn two_current_verifications_that_disagree_are_a_corrupt_graph() {
    let (bench, source, candidate, first) = candidate_bench();
    let second = add(
        &bench.store,
        &bench.vcs,
        node("review it again"),
        Some(candidate.id.clone()),
    )
    .unwrap();

    bench
        .conclude(&first, Conclusion::Accepted, "fine")
        .unwrap();
    bench
        .conclude(&second, Conclusion::Rejected, "not fine")
        .unwrap();

    assert!(bench.graph().decision(&candidate.id).is_err());
    assert!(bench.state(&source).is_error());
    assert!(
        check(&bench.store)
            .unwrap()
            .iter()
            .any(|problem| problem.contains("disagree")),
        "{:?}",
        check(&bench.store).unwrap()
    );
}

#[test]
fn a_verification_node_cannot_record_work_and_a_work_node_cannot_review() {
    let (bench, source, _, review) = candidate_bench();

    let error = bench
        .conclude(&review, Conclusion::Done { output: None }, "done")
        .unwrap_err();
    assert!(format!("{error}").contains("accepted, rejected, or abandoned"));

    let other = bench.add("ordinary work");
    let error = bench
        .conclude(&other, Conclusion::Accepted, "?")
        .unwrap_err();
    assert!(format!("{error}").contains("done or failed"));
    assert!(bench.state(&source).error().is_none());
}

#[test]
fn registering_the_same_candidate_twice_converges() {
    let (bench, source, candidate, _) = candidate_bench();
    let external = ExternalIdentity {
        namespace: "orka".parse().unwrap(),
        id: "attempt-7".into(),
    };

    let same = register_candidate(
        &bench.store,
        &bench.vcs,
        NewCandidate {
            node: source.clone(),
            branch: candidate.branch.clone(),
            target: candidate.target.clone(),
            external: None,
        },
    )
    .unwrap();
    assert_eq!(same, candidate);

    // A producer's own identity is honoured on retry, without a second record.
    let bench2 = Workbench::new();
    bench2.vcs.commit("base", None);
    let source = bench2.add("produce something");
    bench2.produce(&source, "commit-1");
    let register = |external: &ExternalIdentity| {
        register_candidate(
            &bench2.store,
            &bench2.vcs,
            NewCandidate {
                node: source.clone(),
                branch: "work/produce".into(),
                target: "main".into(),
                external: Some(external.clone()),
            },
        )
    };
    let first = register(&external).unwrap();
    assert_eq!(register(&external).unwrap(), first);
    assert_eq!(bench2.graph().candidates().count(), 1);
}

#[test]
fn a_candidate_backed_result_never_drifts() {
    let (bench, source, _, _) = candidate_bench();
    // The working tree has moved on from the artifact...
    let vcs = FakeVcs {
        drift_for: [("commit-1".to_string(), "M src/lib.rs".to_string())]
            .into_iter()
            .collect(),
        ..FakeVcs::default()
    };
    vcs.commit("base", None);
    vcs.commit("commit-1", Some("base"));

    let graph = Graph::load(&bench.store, &vcs).unwrap();
    // ...which says nothing about an immutable artifact commit.
    assert_eq!(graph.state(&source).staleness(), []);
}

#[test]
fn a_direct_result_drifts_with_the_project_working_tree() {
    let bench = Workbench::new();
    let id = bench.add("apply it directly");
    bench.produce(&id, "commit-1");
    assert_eq!(bench.workability(&id), Workability::Complete);

    let vcs = FakeVcs {
        drift_for: [("commit-1".to_string(), "M src/lib.rs".to_string())]
            .into_iter()
            .collect(),
        ..FakeVcs::default()
    };
    let graph = Graph::load(&bench.store, &vcs).unwrap();
    assert!(matches!(
        graph.state(&id).staleness(),
        [StalenessReason::OutputDrifted { .. }]
    ));
    assert_eq!(graph.state(&id).workability(), Workability::Ready);
}

// --- bad records -------------------------------------------------------------------

#[test]
fn an_unreadable_record_is_that_nodes_state_and_nothing_elses() {
    let bench = Workbench::new();
    let broken = bench.add("this one gets hand-edited");
    let fine = bench.add("this one is untouched");
    let dependent = bench.add_after("this one waits", &broken);
    let before = bench.state(&fine);

    std::fs::write(
        bench.store.node_dir(&broken).join("node.toml"),
        "not = = toml",
    )
    .unwrap();

    let graph = bench.graph();
    assert!(graph.state(&broken).is_error());
    assert_eq!(*graph.state(&fine), before);
    assert_eq!(
        graph.state(&dependent).blockers(),
        [Blocker {
            id: broken,
            reason: BlockerReason::Error
        }]
    );
    assert!(!check(&bench.store).unwrap().is_empty());
}

#[test]
fn a_dependency_cycle_makes_every_node_on_it_an_error() {
    let bench = Workbench::new();
    let first = bench.add("first");
    let second = bench.add_after("second", &first);
    let outside = bench.add_after("outside the cycle", &second);
    link(
        &bench.store,
        &bench.vcs,
        &first,
        &second,
        DepKind::DependsOn,
    )
    .unwrap();

    let graph = bench.graph();
    for id in [&first, &second] {
        assert!(
            graph.state(id).error().is_some_and(|e| e.contains("cycle")),
            "{:?}",
            graph.state(id)
        );
    }
    assert_eq!(
        graph.state(&outside).blockers(),
        [Blocker {
            id: second,
            reason: BlockerReason::Error
        }]
    );
    assert_eq!(graph.cycle_reports().len(), 1);
    assert!(check(&bench.store)
        .unwrap()
        .iter()
        .any(|problem| problem.contains("dependency cycle")));
}

#[test]
fn a_result_whose_outcome_does_not_fit_its_node_is_a_corrupt_record() {
    let bench = Workbench::new();
    let id = bench.add("ordinary work");
    bench.finish(&id);
    let result = bench.store.node_dir(&id).join("result.toml");
    let text = std::fs::read_to_string(&result)
        .unwrap()
        .replace("\"done\"", "\"accepted\"");
    std::fs::write(&result, text).unwrap();

    assert!(bench.state(&id).is_error());
    assert!(check(&bench.store)
        .unwrap()
        .iter()
        .any(|problem| problem.contains("verification outcome")));
}

// --- projections --------------------------------------------------------------------

#[test]
fn every_state_agrees_with_the_others() {
    let (bench, source, _, review) = candidate_bench();
    let waiting = bench.add_after("waits on the source", &source);
    let graph = bench.graph();

    for id in graph.ids() {
        let state = graph.state(id);
        // Exactly one workability, and the predicates agree with it.
        let flags = [
            state.is_complete(),
            state.is_ready(),
            state.is_awaiting_integration(),
            state.is_blocked(),
            state.is_error(),
        ];
        assert_eq!(
            flags.iter().filter(|flag| **flag).count(),
            1,
            "{id}: {state:?}"
        );
        assert!(!(state.is_complete() && state.is_ready()));
        if state.is_ready() {
            assert!(state.blockers().is_empty(), "{id}");
        }
        assert_eq!(
            graph.ready(None).contains(&id),
            state.is_ready(),
            "{id} disagrees with the ready listing"
        );
    }
    assert_eq!(graph.blocked().len(), 1);
    assert_eq!(graph.blocked()[0].0, &waiting);
    assert_eq!(graph.stale(), []);
    let mut dependents = graph.dependents(&source);
    dependents.sort();
    let mut expected = vec![&review, &waiting];
    expected.sort();
    assert_eq!(dependents, expected);
    assert_eq!(graph.origin("commit-1"), Some(&source));
    assert!(!graph.settled(&source).is_empty());
}

#[test]
fn ready_work_can_be_restricted_to_who_it_is_for() {
    let bench = Workbench::new();
    let anyone = bench.add("anyone can do this");
    let question = add(
        &bench.store,
        &bench.vcs,
        NewNode {
            assignee: Some(Author::Human),
            ..node("only a person can answer this")
        },
        None,
    )
    .unwrap();

    let graph = bench.graph();
    assert_eq!(graph.ready(Some(Author::Machine)), vec![&anyone]);
    let mut human = graph.ready(Some(Author::Human));
    human.sort();
    let mut both = vec![&anyone, &question];
    both.sort();
    assert_eq!(human, both);
}

#[test]
fn declared_outputs_cover_the_files_beneath_them() {
    let vcs = FakeVcs {
        dirty: vec!["out/nested/file.txt".into(), "elsewhere.txt".into()],
        ..FakeVcs::default()
    };
    let error = require_clean_except(&vcs, &["out".into()]).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("elsewhere.txt"), "{message}");
    assert!(!message.contains("out/nested/file.txt"), "{message}");
    // A prefix that is not a path boundary is not covered.
    assert!(require_clean_except(&vcs, &["out/nested".into(), "else".into()]).is_err());
    assert!(require_clean_except(
        &vcs,
        &["out/nested/file.txt".into(), "elsewhere.txt".into()]
    )
    .is_ok());
}

#[test]
fn a_dirty_store_blocks_every_mutation_until_it_is_resolved() {
    let bench = Workbench::new();
    bench
        .vcs
        .dirty_store
        .borrow_mut()
        .push(".linka/nodes".into());

    let error = add(&bench.store, &bench.vcs, node("must not be created"), None).unwrap_err();

    assert!(
        format!("{error:#}").contains("uncommitted store changes"),
        "{error:#}"
    );
    assert!(bench.store.node_ids().unwrap().is_empty());
    // Read-only inspection still works throughout.
    assert!(bench.graph().ready(None).is_empty());
}

#[test]
fn an_attachment_batch_is_validated_before_a_result_is_written() {
    let bench = Workbench::new();
    let id = bench.add("submit with evidence");
    let attachment = |data: &[u8]| NewAttachment {
        namespace: "orka".parse().unwrap(),
        key: "transcript".parse().unwrap(),
        media_type: None,
        data: data.to_vec(),
    };
    attach(&bench.store, &bench.vcs, &id, vec![attachment(b"first")]).unwrap();

    let snapshot = snapshot(&bench.store, &bench.vcs, &id, &[]).unwrap();
    let error = submit(
        &bench.store,
        &bench.vcs,
        Submission {
            snapshot,
            conclusion: Conclusion::Done { output: None },
            notes: "done".into(),
            author: Author::Machine,
            producer: None,
            attachments: vec![attachment(b"second")],
        },
    )
    .unwrap_err();

    assert!(format!("{error}").contains("different content"), "{error}");
    assert!(bench.store.read_result(&id).unwrap().is_none());
    assert_eq!(
        bench.store.list_attachments(&id).unwrap().len(),
        1,
        "the rejected batch left nothing behind"
    );
}

#[test]
fn a_result_and_its_attachments_are_recorded_in_one_commit() {
    let bench = Workbench::new();
    let id = bench.add("submit with evidence");
    let snapshot = snapshot(&bench.store, &bench.vcs, &id, &[]).unwrap();
    let commits = *bench.vcs.store_commits.borrow();

    submit(
        &bench.store,
        &bench.vcs,
        Submission {
            snapshot,
            conclusion: Conclusion::Done { output: None },
            notes: "done".into(),
            author: Author::Machine,
            producer: Some(crate::model::Namespaced {
                namespace: "orka".parse().unwrap(),
                data: toml::Value::String("attempt-3".into()),
            }),
            attachments: vec![NewAttachment {
                namespace: "orka".parse().unwrap(),
                key: "transcript".parse().unwrap(),
                media_type: None,
                data: b"what happened".to_vec(),
            }],
        },
    )
    .unwrap();

    assert_eq!(*bench.vcs.store_commits.borrow(), commits + 1);
    let attachments: Vec<Attachment> = bench.store.list_attachments(&id).unwrap();
    assert_eq!(attachments.len(), 1);
    let result = bench.store.read_result(&id).unwrap().unwrap().0;
    assert_eq!(
        result.producer.unwrap().data,
        toml::Value::String("attempt-3".into())
    );
    assert_eq!(bench.workability(&id), Workability::Complete);
}
