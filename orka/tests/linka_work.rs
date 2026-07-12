//! Orka's concrete Linka integration against a real workbench: two git
//! repositories, a real store, and the real `git` CLI. Exercises selection,
//! snapshot/prompt preparation, and version-checked submission of success and
//! failure — including every structured conflict and producer evidence.

mod common;

use common::*;
use linka::{Author, SubmissionConflict};
use orka::linka_work::{LinkaWork, Settled};
use orka::workspace::{GitWorkspaces, WorkspaceManager};
use std::path::Path;

/// Prepare a real execution worktree at the snapshot's input commit, so submit
/// can capture from and validate against it exactly as the engine does.
fn worktree(root: &Path, attempt: &str, input_commit: &str) -> std::path::PathBuf {
    let workspaces = GitWorkspaces::new(root.join("project"), root.join(".orka/worktrees"));
    workspaces.prepare(attempt, input_commit).unwrap().path
}

fn producer(attempt: &str) -> linka::ProducerEvidence {
    linka::ProducerEvidence {
        namespace: "orka".into(),
        data: serde_json::json!({ "attempt": attempt, "exit_code": 0 }),
    }
}

#[test]
fn selection_is_machine_only_and_in_linka_order() {
    let (_t, root) = workbench();
    let a = add_node(&root, "First machine task", vec![]);
    let human = add_human_node(&root, "A human's task");
    let b = add_node(&root, "Second machine task", vec![]);

    let store = store_at(&root);
    let linka = LinkaWork::new(&store);
    let ready: Vec<String> = linka
        .ready_for_machine()
        .unwrap()
        .into_iter()
        .map(|w| w.node.to_string())
        .collect();
    assert!(ready.contains(&a));
    assert!(ready.contains(&b));
    assert!(
        !ready.contains(&human),
        "human-assigned work is not machine-ready"
    );
    assert_eq!(
        linka.ready_for_machine().unwrap()[0].title,
        "First machine task"
    );
}

#[test]
fn prepare_input_carries_the_snapshot_and_related_prose() {
    let (_t, root) = workbench();
    let project = root.join("project");

    let dep = add_node(&root, "The dependency\n\nBuild the base.", vec![]);
    std::fs::write(project.join("base.txt"), "base\n").unwrap();
    complete_node(&root, &dep, &["base.txt".into()], "base built");
    let src = add_node(&root, "Original spec", vec![]);
    complete_node(&root, &src, &[], "spec answered");
    let work = add_related(
        &root,
        "The work\n\nUse the base.",
        vec![dep.clone()],
        vec![src.clone()],
    );

    let store = store_at(&root);
    let linka = LinkaWork::new(&store);
    let input = linka.prepare_input(&work.parse().unwrap()).unwrap();

    // The snapshot is Linka's authoritative freeze.
    assert_eq!(input.snapshot.node.as_str(), work);
    assert_eq!(
        input.snapshot.project.revision,
        git(&project, &["rev-parse", "HEAD"])
    );
    assert_eq!(input.snapshot.dependencies.len(), 1);
    assert_eq!(input.snapshot.lineage.len(), 1);
    assert_eq!(input.input_commit(), git(&project, &["rev-parse", "HEAD"]));

    // The prose is what the agent is told.
    assert_eq!(input.description, "The work\n\nUse the base.");
    assert_eq!(input.dependency_context.len(), 1);
    assert_eq!(input.dependency_context[0].title, "The dependency");
    assert_eq!(input.dependency_context[0].result_notes, "base built");
    assert_eq!(input.lineage_context.len(), 1);
    assert_eq!(input.lineage_context[0].title, "Original spec");

    // A blocked node refuses to snapshot.
    let blocked = add_node(&root, "Blocked", vec![work.clone()]);
    assert!(linka.prepare_input(&blocked.parse().unwrap()).is_err());
}

#[test]
fn submit_success_with_outputs_captures_and_records_with_producer_evidence() {
    let (_t, root) = workbench();
    let work = add_node(&root, "Write the file", vec![]);
    let store = store_at(&root);
    let linka = LinkaWork::new(&store);
    let input = linka.prepare_input(&work.parse().unwrap()).unwrap();

    let ws = worktree(&root, "attempt-1", input.input_commit());
    std::fs::write(ws.join("out.txt"), "produced\n").unwrap();

    let settled = linka
        .submit_success(
            &input,
            &ws,
            &["out.txt".parse().unwrap()],
            Some("write the file".into()),
            "wrote it".into(),
            producer("attempt-1"),
        )
        .unwrap();
    let Settled::Accepted { output_commit } = settled else {
        panic!("expected acceptance, got {settled:?}");
    };
    let commit = output_commit.expect("an output commit");

    // The result is recorded, pinned to the output, and carries Orka's evidence.
    assert_eq!(
        linka::ops::output_of(&store, &work).unwrap().as_deref(),
        Some(commit.as_str())
    );
    let (result, _) = store.read_result(&work).unwrap().unwrap();
    assert_eq!(result.author, Author::Machine);
    let evidence = result.producer.expect("producer evidence");
    assert_eq!(evidence.namespace, "orka");
    assert_eq!(evidence.data["attempt"], "attempt-1");
}

#[test]
fn submit_success_supports_graph_only_work() {
    let (_t, root) = workbench();
    let project = root.join("project");
    let work = add_node(&root, "Answer a question", vec![]);
    let store = store_at(&root);
    let linka = LinkaWork::new(&store);
    let input = linka.prepare_input(&work.parse().unwrap()).unwrap();
    let head = git(&project, &["rev-parse", "HEAD"]);
    let ws = worktree(&root, "attempt-1", input.input_commit());

    let settled = linka
        .submit_success(
            &input,
            &ws,
            &[],
            None,
            "answered".into(),
            producer("attempt-1"),
        )
        .unwrap();
    assert!(matches!(
        settled,
        Settled::Accepted {
            output_commit: None
        }
    ));
    // No project commit was made.
    assert_eq!(git(&project, &["rev-parse", "HEAD"]), head);
    let vcs = linka::GitVcs::for_store(&store);
    assert!(linka::ops::node_state(&store, &vcs, &work)
        .unwrap()
        .is_complete());
}

#[test]
fn submit_failure_records_evidence_against_the_frozen_snapshot() {
    let (_t, root) = workbench();
    let work = add_node(&root, "Doomed work", vec![]);
    let store = store_at(&root);
    let linka = LinkaWork::new(&store);
    let input = linka.prepare_input(&work.parse().unwrap()).unwrap();
    let ws = worktree(&root, "attempt-1", input.input_commit());

    let settled = linka
        .submit_failure(
            &input,
            &ws,
            "the approach does not work".into(),
            producer("attempt-1"),
        )
        .unwrap();
    assert!(matches!(
        settled,
        Settled::Accepted {
            output_commit: None
        }
    ));

    let vcs = linka::GitVcs::for_store(&store);
    let state = linka::ops::node_state(&store, &vcs, &work).unwrap();
    assert!(!state.is_complete());
    assert!(state.is_ready(), "a failed node can be retried");
}

#[test]
fn a_definition_change_after_snapshot_is_a_conflict_that_records_nothing() {
    let (_t, root) = workbench();
    let work = add_node(&root, "Moving target", vec![]);
    let store = store_at(&root);
    let linka = LinkaWork::new(&store);
    let input = linka.prepare_input(&work.parse().unwrap()).unwrap();
    let ws = worktree(&root, "attempt-1", input.input_commit());

    // The definition moves between snapshot and submit.
    edit_node(&root, &work, "Moving target, redefined");

    std::fs::write(ws.join("out.txt"), "produced\n").unwrap();
    let settled = linka
        .submit_success(
            &input,
            &ws,
            &["out.txt".parse().unwrap()],
            None,
            "did it".into(),
            producer("attempt-1"),
        )
        .unwrap();
    match settled {
        Settled::Conflict(conflicts) => {
            assert!(
                conflicts.contains(&SubmissionConflict::DefinitionChanged),
                "{conflicts:?}"
            );
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert!(
        store.read_result(&work).unwrap().is_none(),
        "nothing recorded on a conflict"
    );
}

#[test]
fn a_dependency_change_after_snapshot_is_a_conflict() {
    let (_t, root) = workbench();
    let project = root.join("project");
    let dep = add_node(&root, "Dependency", vec![]);
    std::fs::write(project.join("base.txt"), "base\n").unwrap();
    complete_node(&root, &dep, &["base.txt".into()], "built");
    let work = add_node(&root, "Consumer", vec![dep.clone()]);

    let store = store_at(&root);
    let linka = LinkaWork::new(&store);
    let input = linka.prepare_input(&work.parse().unwrap()).unwrap();
    let ws = worktree(&root, "attempt-1", input.input_commit());

    // The dependency's definition moves, so the consumer's pin goes stale.
    edit_node(&root, &dep, "Dependency, redefined");

    let settled = linka
        .submit_failure(&input, &ws, "n/a".into(), producer("attempt-1"))
        .unwrap();
    assert!(matches!(settled, Settled::Conflict(_)), "{settled:?}");
    assert!(store.read_result(&work).unwrap().is_none());
}
