//! Tests for the graph operations, against the in-memory `FakeVcs`.

use super::*;
use crate::vcs::FakeVcs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// A temp directory removed on drop, so tests are self-contained.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh, initialised store under a unique temp directory.
fn temp_store() -> (TempDir, Store) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("linka-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::init(root.join(".linka")).unwrap();
    (TempDir(root), store)
}

fn new_node(description: &str, depends_on: Vec<NodeId>) -> NewNode {
    NewNode {
        description: description.into(),
        author: Author::Human,
        assignee: None,
        depends_on,
        derived_from: vec![],
    }
}

/// A node id for the "this node does not exist" cases.
fn missing() -> NodeId {
    "node-missing".parse().unwrap()
}

fn done(store: &Store, vcs: &dyn Vcs, id: &NodeId) {
    complete(store, vcs, id, &[], &[], None, "done", Author::Human).unwrap();
}

#[test]
fn output_and_dependent_queries_reject_unknown_nodes() {
    let (_t, store) = temp_store();
    assert!(output_of(&store, &missing()).is_err());
    assert!(dependents(&store, &missing()).is_err());
}

#[test]
fn context_observations_are_immutable_and_result_versioned() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "c1".into(),
        ..Default::default()
    };
    let root = store.project_root();
    std::fs::write(root.join("declared.txt"), "d").unwrap();
    std::fs::write(root.join("read.txt"), "r").unwrap();
    std::fs::write(root.join("out.txt"), "o").unwrap();

    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    complete(
        &store,
        &fake,
        &id,
        &["out.txt".into()],
        &["declared.txt".into()],
        None,
        "done",
        Author::Human,
    )
    .unwrap();

    // One genuinely new read gets pinned; a declared pin, a node output, a
    // missing file, and a duplicate do not.
    let reads: Vec<String> = [
        "read.txt",
        "declared.txt",
        "out.txt",
        "missing.txt",
        "read.txt",
    ]
    .map(String::from)
    .to_vec();
    let version = store.result_version(&id).unwrap();
    assert_eq!(
        record_context_observation(&store, &fake, &id, &version, &reads).unwrap(),
        1
    );

    let (result, notes) = store.read_result(&id).unwrap().unwrap();
    assert_eq!(notes, "done", "observation keeps the narrative");
    let observations = store.read_context_observations(&id).unwrap();
    let pin = observations[0]
        .context
        .iter()
        .find(|p| p.path == "read.txt")
        .unwrap();
    assert!(pin.observed);
    let declared = result
        .context
        .iter()
        .find(|p| p.path == "declared.txt")
        .unwrap();
    assert!(!declared.observed);
    assert!(!result
        .context
        .iter()
        .any(|p| p.path == "out.txt" || p.path == "missing.txt"));

    // Re-running with the same reads adds nothing.
    assert_eq!(
        record_context_observation(&store, &fake, &id, &version, &reads).unwrap(),
        0
    );
    assert_eq!(store.result_version(&id).unwrap(), version);
}

#[test]
fn observed_context_is_hashed_from_the_results_frozen_revision() {
    let (_t, store) = temp_store();
    let frozen = crate::store::blob_id(b"frozen");
    let fake = FakeVcs {
        root: Some("input-commit".into()),
        revision_blobs: [(("input-commit".into(), "read.txt".into()), frozen.clone())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    std::fs::write(
        store.project_root().join("read.txt"),
        "new checkout content",
    )
    .unwrap();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    done(&store, &fake, &id);
    let version = store.result_version(&id).unwrap();

    assert_eq!(
        record_context_observation(&store, &fake, &id, &version, &["read.txt".into()],).unwrap(),
        1
    );
    let observations = store.read_context_observations(&id).unwrap();
    assert_eq!(observations[0].context[0].identity, frozen);
}

#[test]
fn context_observation_rejects_a_node_without_a_result() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let nonexistent = ResultVersion {
        metadata: "none".into(),
        notes: None,
    };
    assert!(
        record_context_observation(&store, &fake, &id, &nonexistent, &["x.txt".into()]).is_err()
    );
}

#[test]
fn node_attachments_are_opaque_committed_and_idempotent() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let definition = store.node_version(&id).unwrap();
    let commits = *fake.store_commits.borrow();
    let new = NewNodeAttachment {
        namespace: "example".into(),
        key: "report-1".into(),
        media_type: Some("application/octet-stream".into()),
        data: vec![0, 1, 2, 255],
    };

    let attachment = record_node_attachment(&store, &fake, &id, new.clone()).unwrap();
    assert_eq!(*fake.store_commits.borrow(), commits + 1);
    assert_eq!(store.node_version(&id).unwrap(), definition);
    assert_eq!(
        store
            .read_node_attachment(&id, "example", "report-1")
            .unwrap()
            .unwrap()
            .1,
        new.data
    );

    assert_eq!(
        record_node_attachment(&store, &fake, &id, new.clone()).unwrap(),
        attachment
    );
    assert_eq!(
        *fake.store_commits.borrow(),
        commits + 1,
        "an identical retry must not create another Git mutation"
    );

    let batch = vec![
        NewNodeAttachment {
            namespace: "example".into(),
            key: "report-2".into(),
            media_type: Some("text/plain".into()),
            data: b"two".to_vec(),
        },
        NewNodeAttachment {
            namespace: "other".into(),
            key: "report-3".into(),
            media_type: None,
            data: b"three".to_vec(),
        },
    ];
    assert_eq!(
        record_node_attachments(&store, &fake, &id, batch.clone())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        *fake.store_commits.borrow(),
        commits + 2,
        "the batch is one Git mutation"
    );
    record_node_attachments(&store, &fake, &id, batch).unwrap();
    assert_eq!(*fake.store_commits.borrow(), commits + 2);

    let mut changed = new;
    changed.data.push(3);
    assert!(record_node_attachment(&store, &fake, &id, changed)
        .unwrap_err()
        .to_string()
        .contains("different content"));
}

#[test]
fn result_and_attachment_batch_share_one_store_commit() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();
    let commits = *fake.store_commits.borrow();

    submit_result_with_attachments(
        &store,
        &fake,
        ResultSubmission {
            snapshot,
            outcome: Outcome::Done,
            output: None,
            notes: "finished".into(),
            author: Author::Machine,
            producer: None,
        },
        vec![NewNodeAttachment {
            namespace: "orka".into(),
            key: "attempt-1/evidence".into(),
            media_type: Some("application/toml".into()),
            data: b"exit_code = 0\n".to_vec(),
        }],
    )
    .unwrap();

    assert_eq!(*fake.store_commits.borrow(), commits + 1);
    assert!(store.read_result(&id).unwrap().is_some());
    assert!(store
        .read_node_attachment(&id, "orka", "attempt-1/evidence")
        .unwrap()
        .is_some());
}

#[test]
fn conflicting_result_records_no_attachment_batch() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();
    edit(&store, &fake, &id, "moved".into()).unwrap();

    let result = submit_result_with_attachments(
        &store,
        &fake,
        ResultSubmission {
            snapshot,
            outcome: Outcome::Done,
            output: None,
            notes: "stale".into(),
            author: Author::Machine,
            producer: None,
        },
        vec![NewNodeAttachment {
            namespace: "orka".into(),
            key: "attempt-1/evidence".into(),
            media_type: None,
            data: b"must not persist".to_vec(),
        }],
    );

    assert!(matches!(result, Err(SubmissionError::Conflict(_))));
    assert!(store.read_result(&id).unwrap().is_none());
    assert!(store
        .read_node_attachment(&id, "orka", "attempt-1/evidence")
        .unwrap()
        .is_none());
}

#[test]
fn observed_pins_participate_in_staleness() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let root = store.project_root();
    std::fs::write(root.join("read.txt"), "v1").unwrap();

    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    done(&store, &fake, &id);
    let version = store.result_version(&id).unwrap();
    assert_eq!(
        record_context_observation(&store, &fake, &id, &version, &["read.txt".into()]).unwrap(),
        1
    );
    assert!(staleness(&store, &fake, &id).unwrap().is_empty());

    std::fs::write(root.join("read.txt"), "v2").unwrap();
    let reasons = staleness(&store, &fake, &id).unwrap();
    assert!(
        reasons
            .iter()
            .any(|r| format!("{r:?}").contains("read.txt")),
        "{reasons:?}"
    );
}

#[test]
fn add_validates_dependencies_and_starts_open() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();

    assert!(add(
        &store,
        &fake,
        new_node("a", vec!["node-nope".parse().unwrap()])
    )
    .is_err());

    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    assert!(store.exists(&id));
    assert!(!node_state(&store, &fake, &id).unwrap().is_complete());
    assert!(
        staleness(&store, &fake, &id).unwrap().is_empty(),
        "no result, nothing to invalidate"
    );
}

#[test]
fn complete_records_result_and_output_commit() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "commit-abc".into(),
        ..Default::default()
    };
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();

    let commit = complete(
        &store,
        &fake,
        &id,
        &["src/x.rs".into()],
        &[],
        None,
        "implemented it",
        Author::Human,
    )
    .unwrap();
    assert_eq!(commit.as_deref(), Some("commit-abc"));
    assert!(node_state(&store, &fake, &id).unwrap().is_complete());
    assert_eq!(
        output_of(&store, &id).unwrap().as_deref(),
        Some("commit-abc")
    );

    let (result, notes) = store.read_result(&id).unwrap().unwrap();
    assert_eq!(result.definition, store.node_version(&id).unwrap());
    assert_eq!(notes, "implemented it");

    // The right paths were captured; add + complete each committed the store.
    assert_eq!(
        fake.captured.borrow().as_slice(),
        &[vec!["src/x.rs".to_string()]]
    );
    assert_eq!(*fake.store_commits.borrow(), 2);
}

#[test]
fn complete_without_outputs_makes_no_commit() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("planning", vec![])).unwrap();

    let commit = complete(
        &store,
        &fake,
        &id,
        &[],
        &[],
        None,
        "made sub-tasks",
        Author::Human,
    )
    .unwrap();
    assert_eq!(commit, None);
    assert!(node_state(&store, &fake, &id).unwrap().is_complete());
    assert!(fake.captured.borrow().is_empty(), "nothing captured");
}

#[test]
fn editing_a_done_node_reopens_it() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    done(&store, &fake, &id);
    assert!(node_state(&store, &fake, &id).unwrap().is_complete());

    edit(&store, &fake, &id, "revised".into()).unwrap();
    assert!(!node_state(&store, &fake, &id).unwrap().is_complete());
    let reasons = staleness(&store, &fake, &id).unwrap();
    assert!(matches!(
        reasons.as_slice(),
        [StalenessReason::DefinitionChanged {
            description: true,
            ..
        }]
    ));
    assert!(node_state(&store, &fake, &id).unwrap().is_ready());
}

#[test]
fn editing_with_the_stored_description_is_a_no_op() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    done(&store, &fake, &id);
    let version = store.node_version(&id).unwrap();
    let commits = *fake.store_commits.borrow();
    let (_, description) = store.read_node(&id).unwrap();

    let outcome = edit(&store, &fake, &id, description).unwrap();

    assert_eq!(outcome, EditOutcome::Unchanged);
    assert_eq!(*fake.store_commits.borrow(), commits);
    assert_eq!(store.node_version(&id).unwrap(), version);
    assert!(node_state(&store, &fake, &id).unwrap().is_complete());
}

#[test]
fn editing_node_metadata_reopens_it() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    done(&store, &fake, &id);

    let (mut meta, description) = store.read_node(&id).unwrap();
    meta.assignee = Some(Author::Human);
    store.write_node(&id, &meta, &description).unwrap();

    assert!(!node_state(&store, &fake, &id).unwrap().is_complete());
    let reasons = staleness(&store, &fake, &id).unwrap();
    assert!(matches!(
        reasons.as_slice(),
        [StalenessReason::DefinitionChanged { metadata: true, .. }]
    ));
}

#[test]
fn dependency_definition_move_makes_dependent_stale() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    done(&store, &fake, &a);
    let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
    done(&store, &fake, &b);
    assert!(staleness(&store, &fake, &b).unwrap().is_empty());

    edit(&store, &fake, &a, "revised".into()).unwrap();
    let reasons = staleness(&store, &fake, &b).unwrap();
    assert_eq!(
        reasons,
        vec![StalenessReason::ConsumedDefinitionChanged { id: a }]
    );
}

#[test]
fn dependency_output_change_makes_dependent_stale() {
    let (_t, store) = temp_store();
    let mut fake = FakeVcs {
        next_id: "commit-1".into(),
        ..Default::default()
    };
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    complete(
        &store,
        &fake,
        &a,
        &["src/a.rs".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
    done(&store, &fake, &b);
    assert!(staleness(&store, &fake, &b).unwrap().is_empty());

    // A is re-worked and produces a new output commit -> B is stale.
    fake.next_id = "commit-2".into();
    edit(&store, &fake, &a, "a, revised".into()).unwrap();
    complete(
        &store,
        &fake,
        &a,
        &["src/a.rs".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    let reasons = staleness(&store, &fake, &b).unwrap();
    assert!(reasons.contains(&StalenessReason::ConsumedOutputChanged { id: a }));
    assert!(node_state(&store, &fake, &b).unwrap().is_ready());
}

#[test]
fn dependency_result_notes_change_makes_dependent_stale() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let answer = add(&store, &fake, new_node("answer", vec![])).unwrap();
    respond(&store, &fake, &answer, "use option A", Author::Human).unwrap();
    let consumer = add(&store, &fake, new_node("consumer", vec![answer.clone()])).unwrap();
    done(&store, &fake, &consumer);
    assert!(staleness(&store, &fake, &consumer).unwrap().is_empty());

    edit(&store, &fake, &answer, "answer, revised".into()).unwrap();
    respond(&store, &fake, &answer, "use option B", Author::Human).unwrap();
    let reasons = staleness(&store, &fake, &consumer).unwrap();
    assert!(reasons.contains(&StalenessReason::ConsumedResultChanged { id: answer }));
    assert!(node_state(&store, &fake, &consumer).unwrap().is_ready());
    let state = node_state(&store, &fake, &consumer).unwrap();
    assert_eq!(state.currency, Currency::Stale);
    assert!(
        !state.is_complete(),
        "changed consumed evidence invalidates success"
    );
}

#[test]
fn context_drift_makes_node_stale() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();

    std::fs::write(store.project_root().join("helper.rs"), "v1").unwrap();
    complete(
        &store,
        &fake,
        &id,
        &[],
        &["helper.rs".into()],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    assert!(staleness(&store, &fake, &id).unwrap().is_empty());

    std::fs::write(store.project_root().join("helper.rs"), "v2").unwrap();
    let reasons = staleness(&store, &fake, &id).unwrap();
    assert_eq!(
        reasons,
        vec![StalenessReason::ContextChanged {
            path: "helper.rs".parse().unwrap()
        }]
    );
    assert!(node_state(&store, &fake, &id).unwrap().is_ready());

    std::fs::remove_file(store.project_root().join("helper.rs")).unwrap();
    let reasons = staleness(&store, &fake, &id).unwrap();
    assert_eq!(
        reasons,
        vec![StalenessReason::ContextMissing {
            path: "helper.rs".parse().unwrap()
        }]
    );
}

#[test]
fn own_output_drift_uses_the_vcs() {
    let (_t, store) = temp_store();
    let mut fake = FakeVcs {
        next_id: "commit-x".into(),
        ..Default::default()
    };
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    complete(
        &store,
        &fake,
        &id,
        &["src/x.rs".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    assert!(staleness(&store, &fake, &id).unwrap().is_empty());

    fake.drift_for
        .insert("commit-x".into(), "M\tsrc/x.rs".into());
    let reasons = staleness(&store, &fake, &id).unwrap();
    assert!(matches!(
        reasons.as_slice(),
        [StalenessReason::OutputDrifted { .. }]
    ));
    assert!(node_state(&store, &fake, &id).unwrap().is_ready());
}

#[test]
fn state_errors_are_not_converted_to_graph_facts() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let malformed = add(&store, &fake, new_node("malformed", vec![])).unwrap();
    std::fs::write(store.node_dir(&malformed).join("node.toml"), "not = [toml").unwrap();
    assert!(node_state(&store, &fake, &malformed).is_err());
    assert!(is_ready(&store, &fake, &malformed).is_err());

    let bad_result = add(&store, &fake, new_node("bad result", vec![])).unwrap();
    std::fs::write(
        store.node_dir(&bad_result).join("result.toml"),
        "outcome = ???",
    )
    .unwrap();
    assert!(node_state(&store, &fake, &bad_result).is_err());

    let target = add(&store, &fake, new_node("target", vec![])).unwrap();
    let consumer = add(&store, &fake, new_node("consumer", vec![target.clone()])).unwrap();
    std::fs::remove_dir_all(store.node_dir(&target)).unwrap();
    assert_eq!(
        node_state(&store, &fake, &consumer).unwrap().blockers,
        vec![Blocker {
            id: target.clone(),
            reason: BlockerReason::Missing,
        }]
    );
    std::fs::create_dir_all(store.node_dir(&target)).unwrap();
    std::fs::write(store.node_dir(&target).join("node.toml"), "not = [toml").unwrap();
    std::fs::write(store.node_dir(&target).join("description.md"), "target").unwrap();
    assert!(node_state(&store, &fake, &consumer).is_err());
}

#[test]
fn context_and_artifact_inspection_failures_are_errors() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let context_node = add(&store, &fake, new_node("context", vec![])).unwrap();
    std::fs::write(store.project_root().join("input"), "content").unwrap();
    complete(
        &store,
        &fake,
        &context_node,
        &[],
        &["input".into()],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    std::fs::remove_file(store.project_root().join("input")).unwrap();
    std::fs::create_dir(store.project_root().join("input")).unwrap();
    assert!(node_state(&store, &fake, &context_node).is_err());

    let failing_vcs = FakeVcs {
        next_id: "output".into(),
        drift_error: Some("artifact backend unavailable".into()),
        ..Default::default()
    };
    let output_node = add(&store, &failing_vcs, new_node("output", vec![])).unwrap();
    complete(
        &store,
        &failing_vcs,
        &output_node,
        &["out".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    let error = node_state(&store, &failing_vcs, &output_node).unwrap_err();
    assert!(format!("{error:#}").contains("artifact backend unavailable"));
}

#[cfg(unix)]
#[test]
fn context_symlink_cannot_escape_project_root() {
    use std::os::unix::fs::symlink;
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("context", vec![])).unwrap();
    let outside = store.workbench_root().join("outside-secret");
    std::fs::write(&outside, "secret").unwrap();
    symlink(&outside, store.project_root().join("escape")).unwrap();
    let error = complete(
        &store,
        &fake,
        &id,
        &[],
        &["escape".into()],
        None,
        "",
        Author::Human,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("escapes the project root"));
}

#[test]
fn blockers_follow_dependency_status() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();

    // A not done -> B blocked, not ready.
    let blocked = blockers(&store, &fake, &b).unwrap();
    assert_eq!(
        blocked,
        vec![Blocker {
            id: a.clone(),
            reason: BlockerReason::Open
        }]
    );
    assert!(!is_ready(&store, &fake, &b).unwrap());

    // A done -> B ready.
    done(&store, &fake, &a);
    assert!(blockers(&store, &fake, &b).unwrap().is_empty());
    assert!(is_ready(&store, &fake, &b).unwrap());

    // A edited after done -> reopened -> B blocked again.
    edit(&store, &fake, &a, "revised".into()).unwrap();
    let blocked = blockers(&store, &fake, &b).unwrap();
    assert_eq!(
        blocked,
        vec![Blocker {
            id: a,
            reason: BlockerReason::Stale
        }]
    );
}

#[test]
fn failed_node_is_ready_to_retry_but_blocks_dependents() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();

    fail(&store, &fake, &a, "build broke", Author::Human).unwrap();
    assert_eq!(
        node_state(&store, &fake, &a).unwrap().outcome,
        RecordedOutcome::Failed
    );
    assert!(
        is_ready(&store, &fake, &a).unwrap(),
        "a failed node can be retried"
    );
    assert!(
        !is_ready(&store, &fake, &b).unwrap(),
        "its dependents stay blocked"
    );

    // Retry succeeds: the result is overwritten, B unblocks.
    done(&store, &fake, &a);
    assert!(node_state(&store, &fake, &a).unwrap().is_complete());
    assert!(is_ready(&store, &fake, &b).unwrap());
}

#[test]
fn stale_node_with_incomplete_dependency_is_blocked_and_blocks_dependents() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
    done(&store, &fake, &dependency);
    let stale = add(
        &store,
        &fake,
        new_node("consumer", vec![dependency.clone()]),
    )
    .unwrap();
    done(&store, &fake, &stale);
    let dependent = add(&store, &fake, new_node("dependent", vec![stale.clone()])).unwrap();

    edit(&store, &fake, &dependency, "changed dependency".into()).unwrap();

    let stale_state = node_state(&store, &fake, &stale).unwrap();
    assert_eq!(stale_state.currency, Currency::Stale);
    assert!(stale_state.is_blocked());
    assert_eq!(
        stale_state.blockers,
        vec![Blocker {
            id: dependency,
            reason: BlockerReason::Stale,
        }]
    );
    let dependent_state = node_state(&store, &fake, &dependent).unwrap();
    assert!(dependent_state.is_blocked());
    assert_eq!(
        dependent_state.blockers,
        vec![Blocker {
            id: stale,
            reason: BlockerReason::Stale,
        }]
    );
}

#[test]
fn work_snapshots_freeze_exact_inputs_and_reject_blocked_or_corrupt_nodes() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        root: Some("project-revision".into()),
        revision_blobs: [(
            ("project-revision".into(), "input".into()),
            crate::store::blob_id(b"content"),
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
    done(&store, &fake, &dependency);
    let lineage = add(&store, &fake, new_node("lineage", vec![])).unwrap();
    let mut work = new_node("work", vec![dependency.clone()]);
    work.derived_from = vec![lineage.clone()];
    let work = add(&store, &fake, work).unwrap();
    std::fs::write(store.project_root().join("input"), "content").unwrap();

    let snapshot = snapshot_work(&store, &fake, &work, &["input".into()]).unwrap();
    assert_eq!(snapshot.node, work);
    assert_eq!(snapshot.definition, store.node_version(&work).unwrap());
    assert_eq!(snapshot.dependencies[0].id, dependency);
    assert_eq!(
        snapshot.dependencies[0].outcome,
        Some(ResultOutcome::Work(Outcome::Done))
    );
    assert_eq!(snapshot.lineage[0].id, lineage);
    assert_eq!(snapshot.context[0].path.as_str(), "input");
    assert_eq!(snapshot.project.revision, "project-revision");
    assert_eq!(snapshot.project.tree, "tree-project-revision");

    done(&store, &fake, &work);
    edit(&store, &fake, &work, "changed work".into()).unwrap();
    assert!(
        snapshot_work(&store, &fake, &work, &[]).is_ok(),
        "stale ready work can be snapshotted"
    );

    let blocked = add(&store, &fake, new_node("blocked", vec![lineage.clone()])).unwrap();
    assert!(snapshot_work(&store, &fake, &blocked, &[]).is_err());
    std::fs::write(store.node_dir(&lineage).join("node.toml"), "bad = [toml").unwrap();
    assert!(snapshot_work(&store, &fake, &lineage, &[]).is_err());
}

#[test]
fn submissions_revalidate_snapshots_and_preserve_previous_results_on_conflict() {
    let (_t, store) = temp_store();
    let mut fake = FakeVcs {
        root: Some("revision".into()),
        revision_blobs: [(
            ("revision".into(), "input".into()),
            crate::store::blob_id(b"one"),
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let node = add(&store, &fake, new_node("work", vec![])).unwrap();
    std::fs::write(store.project_root().join("input"), "one").unwrap();
    let snapshot = snapshot_work(&store, &fake, &node, &["input".into()]).unwrap();
    let submission = ResultSubmission {
        snapshot,
        outcome: Outcome::Done,
        output: None,
        notes: "finished".into(),
        author: Author::Human,
        producer: None,
    };
    let foreign = ResultSubmission {
        output: Some(ArtifactRef {
            scheme: "git-commit".into(),
            repository: "foreign-repository".into(),
            id: "output".parse().unwrap(),
        }),
        ..submission.clone()
    };
    assert!(matches!(
        submit_result(&store, &fake, foreign),
        Err(SubmissionError::Evaluation(_))
    ));
    assert!(store.read_result(&node).unwrap().is_none());

    edit(&store, &fake, &node, "changed".into()).unwrap();
    assert!(matches!(
        submit_result(&store, &fake, submission.clone()),
        Err(SubmissionError::Conflict(ref conflicts))
            if conflicts.contains(&SubmissionConflict::DefinitionChanged)
    ));
    assert!(store.read_result(&node).unwrap().is_none());

    let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
    done(&store, &fake, &dependency);
    let consumer = add(
        &store,
        &fake,
        new_node("consumer", vec![dependency.clone()]),
    )
    .unwrap();
    let dependency_snapshot = snapshot_work(&store, &fake, &consumer, &[]).unwrap();
    edit(&store, &fake, &dependency, "dependency, revised".into()).unwrap();
    respond(&store, &fake, &dependency, "new evidence", Author::Human).unwrap();
    let dependency_submission = ResultSubmission {
        snapshot: dependency_snapshot,
        ..submission.clone()
    };
    assert!(matches!(
        submit_result(&store, &fake, dependency_submission),
        Err(SubmissionError::Conflict(ref conflicts))
            if conflicts.contains(&SubmissionConflict::DependenciesChanged)
    ));
    assert!(store.read_result(&consumer).unwrap().is_none());

    let fresh = snapshot_work(&store, &fake, &node, &["input".into()]).unwrap();
    std::fs::write(store.project_root().join("input"), "two").unwrap();
    fake.revision_blobs.insert(
        ("revision".into(), "input".into()),
        crate::store::blob_id(b"two"),
    );
    let context_submission = ResultSubmission {
        snapshot: fresh,
        ..submission.clone()
    };
    assert!(matches!(
        submit_result(&store, &fake, context_submission),
        Err(SubmissionError::Conflict(ref conflicts))
            if conflicts.iter().any(|conflict| matches!(conflict, SubmissionConflict::ContextChanged { .. }))
    ));

    std::fs::write(store.project_root().join("input"), "one").unwrap();
    fake.revision_blobs.insert(
        ("revision".into(), "input".into()),
        crate::store::blob_id(b"one"),
    );
    let concurrent = snapshot_work(&store, &fake, &node, &["input".into()]).unwrap();
    fail(&store, &fake, &node, "other producer", Author::Machine).unwrap();
    let previous = store.result_version(&node).unwrap();
    let concurrent_submission = ResultSubmission {
        snapshot: concurrent,
        ..submission
    };
    assert!(matches!(
        submit_result(&store, &fake, concurrent_submission),
        Err(SubmissionError::Conflict(ref conflicts))
            if conflicts.contains(&SubmissionConflict::PreviousResultChanged)
    ));
    assert_eq!(store.result_version(&node).unwrap(), previous);
    assert_eq!(
        store.read_result(&node).unwrap().unwrap().1,
        "other producer"
    );
}

#[test]
fn successful_results_require_ready_dependencies_but_lineage_does_not_block() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
    let blocked = add(&store, &fake, new_node("blocked", vec![dependency.clone()])).unwrap();
    assert!(complete(&store, &fake, &blocked, &[], &[], None, "", Author::Human).is_err());
    assert!(respond(&store, &fake, &blocked, "answer", Author::Human).is_err());
    assert!(store.read_result(&blocked).unwrap().is_none());

    fail(&store, &fake, &blocked, "blocked attempt", Author::Machine).unwrap();
    assert_eq!(
        store.read_result(&blocked).unwrap().unwrap().0.outcome,
        ResultOutcome::Work(Outcome::Failed)
    );

    let lineage = add(&store, &fake, new_node("lineage", vec![])).unwrap();
    let mut derived = new_node("derived", vec![]);
    derived.derived_from = vec![lineage];
    let derived = add(&store, &fake, derived).unwrap();
    respond(
        &store,
        &fake,
        &derived,
        "lineage does not block",
        Author::Human,
    )
    .unwrap();
    assert!(node_state(&store, &fake, &derived).unwrap().is_complete());
}

#[test]
fn origin_maps_a_commit_back_to_its_node() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "commit-xyz".into(),
        ..Default::default()
    };
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    add(&store, &fake, new_node("other", vec![])).unwrap();
    complete(
        &store,
        &fake,
        &a,
        &["src/x.rs".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();

    assert_eq!(origin(&store, "commit-xyz").unwrap(), Some(a));
    assert_eq!(origin(&store, "no-such-commit").unwrap(), None);
}

#[test]
fn dependents_scans_both_lists() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
    let mut c = new_node("c", vec![]);
    c.derived_from = vec![a.clone()];
    let c = add(&store, &fake, c).unwrap();
    add(&store, &fake, new_node("unrelated", vec![])).unwrap();

    let mut deps = dependents(&store, &a).unwrap();
    deps.sort();
    let mut expected = vec![b, c];
    expected.sort();
    assert_eq!(deps, expected);
}

#[test]
fn link_rejects_unknown_and_duplicate_targets() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    let b = add(&store, &fake, new_node("b", vec![])).unwrap();

    assert!(link(
        &store,
        &fake,
        &a,
        &"node-nope".parse().unwrap(),
        DepKind::DependsOn
    )
    .is_err());
    link(&store, &fake, &a, &b, DepKind::DependsOn).unwrap();
    assert!(link(&store, &fake, &a, &b, DepKind::DependsOn).is_err());

    let (meta, _) = store.read_node(&a).unwrap();
    assert_eq!(meta.depends_on, vec![b.parse().unwrap()]);
}

#[test]
fn check_reports_sideways_damage() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();

    // A healthy little graph passes.
    let node = add(&store, &fake, new_node("a", vec![])).unwrap();
    let dep = add(&store, &fake, new_node("b", vec![node.clone()])).unwrap();
    assert!(check(&store).unwrap().is_empty());

    // Damage entered "sideways" (direct writes, as a hand edit or merge would):
    // give `node` a self-reference, a duplicate, and a missing target.
    let (mut meta, body) = store.read_node(&node).unwrap();
    meta.depends_on = vec![
        node.parse().unwrap(),
        node.parse().unwrap(),
        "node-gone".parse().unwrap(),
    ];
    store.write_node(&node, &meta, &body).unwrap();

    let problems = check(&store).unwrap();
    let all = problems.join("\n");
    assert!(all.contains("refers to the node itself"), "{all}");
    assert!(all.contains("duplicate depends_on entry"), "{all}");
    assert!(all.contains("missing or unreadable"), "{all}");
    assert!(
        all.contains(&format!("dependency cycle: {node} -> {node}")),
        "{all}"
    );

    // An unparseable file is reported, not a crash.
    std::fs::write(store.node_dir(&dep).join("node.toml"), "not = valid = toml").unwrap();
    let problems = check(&store).unwrap();
    assert!(
        problems.iter().any(|p| p.contains("unreadable definition")),
        "{problems:?}"
    );
}

#[test]
fn check_workbench_reports_uncommitted_store_changes() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    add(&store, &fake, new_node("a", vec![])).unwrap();

    assert!(check_workbench(&store, &fake).unwrap().is_empty());

    fake.dirty_store
        .borrow_mut()
        .push(" M .linka/nodes/node-partial/result.toml".into());
    let problems = check_workbench(&store, &fake).unwrap();
    let all = problems.join("\n");
    assert!(all.contains("store has uncommitted changes"), "{all}");
    assert!(all.contains("result.toml"), "{all}");
}

#[test]
fn check_artifacts_requires_the_exact_output_retention_ref() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "output-commit".into(),
        root: Some("0123456789abcdef0123456789abcdef01234567".into()),
        ..Default::default()
    };
    pair(&store, &fake, None, false).unwrap();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();
    capture_submission(
        &store,
        &fake,
        snapshot,
        &[path("out.txt")],
        None,
        Outcome::Done,
        String::new(),
        Author::Machine,
        None,
    )
    .unwrap();

    assert!(check_artifacts(&store, &fake).unwrap().is_empty());

    let reference = format!("refs/linka/outputs/{id}");
    fake.refs.borrow_mut().remove(&reference);
    let problems = check_artifacts(&store, &fake).unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("output retention ref is missing")),
        "{problems:?}"
    );

    fake.refs
        .borrow_mut()
        .insert(reference, "different-commit".into());
    let problems = check_artifacts(&store, &fake).unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("points to different-commit")),
        "{problems:?}"
    );
}

#[test]
fn check_rejects_semantically_impossible_results() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
    done(&store, &fake, &dependency);
    let consumer = add(&store, &fake, new_node("consumer", vec![dependency])).unwrap();
    done(&store, &fake, &consumer);

    let (mut result, notes) = store.read_result(&consumer).unwrap().unwrap();
    result.consumed[0].outcome = None;
    result.consumed.push(result.consumed[0].clone());
    result.consumed.push(ConsumedNode {
        id: "undeclared".parse().unwrap(),
        definition: result.definition.clone(),
        result: None,
        outcome: None,
        output: Some(ArtifactRef {
            scheme: "unknown".into(),
            repository: String::new(),
            id: "artifact".parse().unwrap(),
        }),
    });
    result.context.push(crate::model::ContextPin {
        path: "input".parse().unwrap(),
        identity: "one".into(),
        observed: false,
    });
    result.context.push(crate::model::ContextPin {
        path: "input".parse().unwrap(),
        identity: "two".into(),
        observed: false,
    });
    store.write_result(&consumer, &result, &notes).unwrap();

    let problems = check(&store).unwrap().join("\n");
    assert!(problems.contains("duplicate consumed-node pin"));
    assert!(problems.contains("no declared edge"));
    assert!(problems.contains("no successful evidence"));
    assert!(problems.contains("duplicate context pin"));
    assert!(problems.contains("unsupported artifact scheme"));
}

#[test]
fn end_to_end_snapshot_conflict_resnapshot_submit_and_dependency_rework() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
    done(&store, &fake, &dependency);
    let consumer = add(
        &store,
        &fake,
        new_node("consumer", vec![dependency.clone()]),
    )
    .unwrap();
    std::fs::write(store.project_root().join("input"), "one").unwrap();
    let stale_snapshot = snapshot_work(&store, &fake, &consumer, &["input".into()]).unwrap();
    std::fs::write(store.project_root().join("input"), "two").unwrap();
    let stale_submission = ResultSubmission {
        snapshot: stale_snapshot,
        outcome: Outcome::Done,
        output: None,
        notes: "old work".into(),
        author: Author::Machine,
        producer: None,
    };
    assert!(matches!(
        submit_result(&store, &fake, stale_submission),
        Err(SubmissionError::Conflict(_))
    ));

    let fresh = snapshot_work(&store, &fake, &consumer, &["input".into()]).unwrap();
    submit_result(
        &store,
        &fake,
        ResultSubmission {
            snapshot: fresh,
            outcome: Outcome::Done,
            output: None,
            notes: "fresh work".into(),
            author: Author::Machine,
            producer: None,
        },
    )
    .unwrap();
    assert!(node_state(&store, &fake, &consumer).unwrap().is_complete());

    edit(&store, &fake, &dependency, "dependency changed".into()).unwrap();
    done(&store, &fake, &dependency);
    let state = node_state(&store, &fake, &consumer).unwrap();
    assert_eq!(state.currency, Currency::Stale);
    assert!(
        state.is_ready(),
        "consumer is selected for rework after dependency refresh"
    );
}

#[test]
fn check_finds_multi_node_cycles() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
    // Close the loop sideways: a -> b (write-time link would allow a -> b;
    // the *cycle* is only visible to check).
    let (mut meta, body) = store.read_node(&a).unwrap();
    meta.depends_on = vec![b.parse().unwrap()];
    store.write_node(&a, &meta, &body).unwrap();

    let problems = check(&store).unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("dependency cycle: "),
        "{problems:?}"
    );
    assert!(problems[0].contains(a.as_str()) && problems[0].contains(b.as_str()));
}

#[test]
fn settled_requires_the_whole_derived_branch_to_be_done_and_fresh() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "commit-1".into(),
        ..Default::default()
    };
    // Root -> sub-task (derived) -> implementation (depends on the sub-task).
    let root = add(&store, &fake, new_node("idea", vec![])).unwrap();
    let mut sub = new_node("sub", vec![]);
    sub.derived_from = vec![root.clone()];
    let sub = add(&store, &fake, sub).unwrap();
    let imp = add(&store, &fake, new_node("impl", vec![sub.clone()])).unwrap();

    // Root done (spawned the sub-task), sub done (spec settled), impl open:
    // root is done, but not settled — the derived branch is unfinished.
    done(&store, &fake, &root);
    done(&store, &fake, &sub);
    let reasons = unsettled(&store, &fake, &root).unwrap();
    assert_eq!(reasons, vec![format!("{imp}: not done (open)")]);

    // Implementation lands: the whole branch is settled.
    complete(
        &store,
        &fake,
        &imp,
        &["src/x.rs".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    assert!(unsettled(&store, &fake, &root).unwrap().is_empty());
    assert!(
        unsettled(&store, &fake, &imp).unwrap().is_empty(),
        "leaves settle too"
    );

    // Editing the sub-task makes both its success and the implementation
    // that consumed it stale.
    edit(&store, &fake, &sub, "revised".into()).unwrap();
    let reasons = unsettled(&store, &fake, &root).unwrap();
    assert!(
        reasons.contains(&format!("{sub}: done but stale")),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&format!("{imp}: done but stale")),
        "{reasons:?}"
    );
}

#[test]
fn link_rejects_self_reference() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    assert!(link(&store, &fake, &a, &a, DepKind::DependsOn).is_err());
}

#[test]
fn assignee_round_trips_through_add() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let mut question = new_node("Question: which auth scheme?", vec![]);
    question.author = Author::Machine;
    question.assignee = Some(Author::Human);
    let q = add(&store, &fake, question).unwrap();

    let (meta, _) = store.read_node(&q).unwrap();
    assert_eq!(meta.assignee, Some(Author::Human));

    // Unassigned nodes stay unassigned (and omit the key on disk).
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    let (meta, _) = store.read_node(&a).unwrap();
    assert_eq!(meta.assignee, None);
    let text = std::fs::read_to_string(store.node_dir(&a).join("node.toml")).unwrap();
    assert!(!text.contains("assignee"), "{text}");
}

#[test]
fn respond_completes_despite_a_dirty_tree_and_pins_dependencies() {
    let (_t, store) = temp_store();
    // Groundwork lands on a clean tree; then the tree goes dirty with
    // whatever prompted the question — the normal state when a question
    // node is answered.
    let mut dirty = FakeVcs::default();
    let dep = add(&store, &dirty, new_node("groundwork", vec![])).unwrap();
    done(&store, &dirty, &dep);
    dirty.dirty.push("PROPOSAL.md".into());
    let mut question = new_node("Question: which concept?", vec![dep.clone()]);
    question.author = Author::Machine;
    question.assignee = Some(Author::Human);
    let q = add(&store, &dirty, question).unwrap();

    assert!(
        respond(&store, &dirty, &q, "  ", Author::Human).is_err(),
        "needs text"
    );
    respond(&store, &dirty, &q, "concept A", Author::Human).unwrap();

    assert!(node_state(&store, &dirty, &q).unwrap().is_complete());
    let (result, notes) = store.read_result(&q).unwrap().unwrap();
    assert_eq!(notes, "concept A");
    assert_eq!(result.author, Author::Human);
    assert_eq!(result.output, None);
    assert_eq!(result.consumed.len(), 1, "the answer pins its dependencies");
    assert!(
        dirty.captured.borrow().is_empty(),
        "no output commit is minted"
    );

    // Editing the question afterwards invalidates the answer as usual.
    edit(&store, &dirty, &q, "Question: revised".into()).unwrap();
    assert!(!node_state(&store, &dirty, &q).unwrap().is_complete());
}

#[test]
fn a_dirty_project_tree_blocks_only_completion() {
    let (_t, store) = temp_store();
    // The project tree is mid-hack: uncommitted changes unrelated to any node.
    let dirty = FakeVcs {
        dirty: vec!["src/x.rs".into()],
        ..Default::default()
    };

    // Pure graph edits never gate on project state.
    let a = add(&store, &dirty, new_node("a", vec![])).unwrap();
    let b = add(&store, &dirty, new_node("b", vec![])).unwrap();
    link(&store, &dirty, &b, &a, DepKind::DependsOn).unwrap();
    edit(&store, &dirty, &a, "revised".into()).unwrap();

    // A failed attempt may have left the mess; recording it must not block.
    fail(&store, &dirty, &a, "broke", Author::Human).unwrap();

    // Completion asserts output provenance: the undeclared write is refused,
    // and declaring it is what unblocks the completion.
    assert!(complete(&store, &dirty, &a, &[], &[], None, "", Author::Human).is_err());
    complete(
        &store,
        &dirty,
        &a,
        &["src/x.rs".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();
}

#[test]
fn require_clean_except_allows_exactly_the_declared_outputs() {
    let outputs = vec!["src/out.rs".to_string()];
    let ok = FakeVcs {
        dirty: vec!["src/out.rs".into()],
        ..Default::default()
    };
    assert!(require_clean_except(&ok, &outputs).is_ok());
    assert!(require_clean_except(&FakeVcs::default(), &outputs).is_ok());

    let stray = FakeVcs {
        dirty: vec!["src/out.rs".into(), "src/other.rs".into()],
        ..Default::default()
    };
    assert!(require_clean_except(&stray, &outputs).is_err());
}

#[test]
fn pair_records_the_root_and_is_idempotent() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        root: Some("root-1".into()),
        ..Default::default()
    };

    let pairing = pair(&store, &fake, None, false).unwrap();
    assert_eq!(pairing.root_commit, "root-1");
    assert_eq!(*fake.store_commits.borrow(), 1);

    // Same root again: no re-write, no extra store commit.
    let again = pair(&store, &fake, None, false).unwrap();
    assert_eq!(again.root_commit, "root-1");
    assert_eq!(*fake.store_commits.borrow(), 1);
}

#[test]
fn pair_records_and_refreshes_the_informational_fields() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        root: Some("root-1".into()),
        remote: Some("git@host:me/p.git".into()),
        ..Default::default()
    };

    // Name from the caller, remote observed from the repo.
    let pairing = pair(&store, &fake, Some("splurt".into()), false).unwrap();
    assert_eq!(pairing.name.as_deref(), Some("splurt"));
    assert_eq!(pairing.remote.as_deref(), Some("git@host:me/p.git"));
    let at = pairing.paired_at;

    // Same root, new name: the info updates without touching the identity
    // or its timestamp; one extra store commit records it.
    let renamed = pair(&store, &fake, Some("splurt-2".into()), false).unwrap();
    assert_eq!(renamed.name.as_deref(), Some("splurt-2"));
    assert_eq!(renamed.paired_at, at);
    assert_eq!(*fake.store_commits.borrow(), 2);

    // A repo whose remote vanished keeps the last-known one; nothing to
    // update, no commit.
    let no_remote = FakeVcs {
        root: Some("root-1".into()),
        ..Default::default()
    };
    let kept = pair(&store, &no_remote, None, false).unwrap();
    assert_eq!(kept.remote.as_deref(), Some("git@host:me/p.git"));
    assert_eq!(kept.name.as_deref(), Some("splurt-2"));
    assert_eq!(*no_remote.store_commits.borrow(), 0);
}

#[test]
fn pair_refuses_an_empty_project_and_a_different_root_without_force() {
    let (_t, store) = temp_store();
    assert!(
        pair(&store, &FakeVcs::default(), None, false).is_err(),
        "no commits"
    );

    let first = FakeVcs {
        root: Some("root-1".into()),
        ..Default::default()
    };
    pair(&store, &first, None, false).unwrap();

    let other = FakeVcs {
        root: Some("root-2".into()),
        ..Default::default()
    };
    assert!(
        pair(&store, &other, None, false).is_err(),
        "mismatched root"
    );
    // A deliberate re-pair (history rewrite) goes through with force.
    assert_eq!(
        pair(&store, &other, None, true).unwrap().root_commit,
        "root-2"
    );
}

#[test]
fn verify_pairing_reports_unpaired_matching_and_mismatched() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        root: Some("root-1".into()),
        ..Default::default()
    };

    // Unpaired: no problems, no recorded pairing.
    let (recorded, problems) = verify_pairing(&store, &fake, false).unwrap();
    assert!(recorded.is_none());
    assert!(problems.is_empty());

    pair(&store, &fake, None, false).unwrap();
    let (recorded, problems) = verify_pairing(&store, &fake, false).unwrap();
    assert_eq!(recorded.unwrap().root_commit, "root-1");
    assert!(problems.is_empty());

    let moved = FakeVcs {
        root: Some("root-2".into()),
        ..Default::default()
    };
    let (_, problems) = verify_pairing(&store, &moved, false).unwrap();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("root-2"), "{}", problems[0]);

    let empty = FakeVcs::default();
    let (_, problems) = verify_pairing(&store, &empty, false).unwrap();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("no commits"), "{}", problems[0]);
}

#[test]
fn deep_verify_finds_orphaned_output_commits() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        root: Some("root-1".into()),
        next_id: "commit-1".into(),
        ..Default::default()
    };
    pair(&store, &fake, None, false).unwrap();

    // A completes with an output commit; B is built against it.
    let a = add(&store, &fake, new_node("a", vec![])).unwrap();
    std::fs::write(store.project_root().join("out.rs"), "x").unwrap();
    complete(
        &store,
        &fake,
        &a,
        &["out.rs".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap();
    let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
    complete(&store, &fake, &b, &[], &[], None, "", Author::Human).unwrap();

    // The commit exists: deep verify is clean.
    let (_, problems) = verify_pairing(&store, &fake, true).unwrap();
    assert!(problems.is_empty(), "{problems:?}");

    // A history rewrite drops the commit: both the output and the
    // built-against pin are reported.
    fake.commits.borrow_mut().clear();
    let (_, problems) = verify_pairing(&store, &fake, true).unwrap();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems
        .iter()
        .any(|p| p.starts_with(a.as_str()) && p.contains("output commit")));
    assert!(problems
        .iter()
        .any(|p| p.starts_with(b.as_str()) && p.contains("built-against")));
}

fn path(p: &str) -> ProjectPath {
    p.parse().unwrap()
}

#[test]
fn capture_submission_records_a_result_and_retains_the_output() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "out-commit".into(),
        ..Default::default()
    };
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();

    let producer = ProducerEvidence {
        namespace: "orka".into(),
        data: serde_json::json!({ "attempt": "attempt-1" }),
    };
    let commit = capture_submission(
        &store,
        &fake,
        snapshot,
        &[path("src/x.rs")],
        Some("do it".into()),
        Outcome::Done,
        "did it".into(),
        Author::Machine,
        Some(producer.clone()),
    )
    .unwrap();
    assert_eq!(commit.as_deref(), Some("out-commit"));

    let (result, notes) = store.read_result(&id).unwrap().unwrap();
    assert_eq!(notes, "did it");
    assert_eq!(result.author, Author::Machine);
    assert_eq!(
        result.output.as_ref().map(|a| a.id.as_str()),
        Some("out-commit")
    );
    // Producer evidence is preserved verbatim, never interpreted.
    assert_eq!(result.producer.as_ref(), Some(&producer));
    // The declared output was captured and its ref retained.
    assert_eq!(
        fake.captured.borrow().as_slice(),
        &[vec!["src/x.rs".to_string()]]
    );
    assert!(fake.commits.borrow().contains("out-commit"));
}

#[test]
fn capture_submission_supports_graph_only_success() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();

    let commit = capture_submission(
        &store,
        &fake,
        snapshot,
        &[],
        None,
        Outcome::Done,
        "answered".into(),
        Author::Machine,
        None,
    )
    .unwrap();
    assert_eq!(commit, None);
    assert!(
        fake.captured.borrow().is_empty(),
        "no project commit for graph-only work"
    );
    assert!(node_state(&store, &fake, &id).unwrap().is_complete());
}

#[test]
fn capture_submission_refuses_undeclared_changes_for_graph_only_success() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        dirty: vec!["src/undeclared.rs".into()],
        ..Default::default()
    };
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();

    let error = capture_submission(
        &store,
        &fake,
        snapshot,
        &[],
        None,
        Outcome::Done,
        "claimed graph-only work".into(),
        Author::Machine,
        None,
    )
    .unwrap_err();

    assert!(error.to_string().contains("undeclared"), "{error:#}");
    assert!(store.read_result(&id).unwrap().is_none());
    assert!(fake.captured.borrow().is_empty());
}

#[test]
fn capture_submission_records_failure_against_the_frozen_snapshot() {
    let (_t, store) = temp_store();
    let fake = FakeVcs::default();
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();

    let commit = capture_submission(
        &store,
        &fake,
        snapshot,
        &[],
        None,
        Outcome::Failed,
        "could not".into(),
        Author::Machine,
        None,
    )
    .unwrap();
    assert_eq!(commit, None);
    assert_eq!(
        node_state(&store, &fake, &id).unwrap().outcome,
        RecordedOutcome::Failed
    );
    assert!(fake.captured.borrow().is_empty());
}

#[test]
fn capture_submission_refuses_a_moved_graph_and_records_nothing() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "out-commit".into(),
        ..Default::default()
    };
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    let snapshot = snapshot_work(&store, &fake, &id, &[]).unwrap();

    // The definition moves between freeze and submit.
    edit(&store, &fake, &id, "a, revised".into()).unwrap();

    let err = capture_submission(
        &store,
        &fake,
        snapshot,
        &[path("src/x.rs")],
        None,
        Outcome::Done,
        "did it".into(),
        Author::Machine,
        None,
    )
    .unwrap_err();
    match err {
        SubmissionError::Conflict(conflicts) => {
            assert!(
                conflicts.contains(&SubmissionConflict::DefinitionChanged),
                "{conflicts:?}"
            );
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    // No result recorded and no output ref retained on a conflict.
    assert!(store.read_result(&id).unwrap().is_none());
}

#[test]
fn unrecorded_linka_output_at_project_head_is_inconsistent() {
    let (_t, store) = temp_store();
    let setup = FakeVcs::default();
    let id = add(&store, &setup, new_node("a", vec![])).unwrap();
    let fake = FakeVcs {
        root: Some("dangling-output".into()),
        linka_nodes: [("dangling-output".into(), id.to_string())].into(),
        ..Default::default()
    };

    let error = require_consistent_project_head(&store, &fake).unwrap_err();

    assert!(
        error.to_string().contains("has never recorded"),
        "{error:#}"
    );
    assert!(error.to_string().contains(id.as_str()), "{error:#}");
}

#[test]
fn historical_linka_output_at_project_head_is_consistent() {
    let (_t, store) = temp_store();
    let setup = FakeVcs::default();
    let id = add(&store, &setup, new_node("a", vec![])).unwrap();
    let fake = FakeVcs {
        root: Some("historical-output".into()),
        linka_nodes: [("historical-output".into(), id.to_string())].into(),
        recorded_outputs: [(id.to_string(), "historical-output".into())].into(),
        ..Default::default()
    };

    require_consistent_project_head(&store, &fake).unwrap();
}

#[test]
fn completion_refuses_a_dirty_store_before_capturing_project_output() {
    let (_t, store) = temp_store();
    let fake = FakeVcs {
        next_id: "dangling-output".into(),
        ..Default::default()
    };
    let id = add(&store, &fake, new_node("a", vec![])).unwrap();
    fake.dirty_store.borrow_mut().push("interference".into());

    let error = complete(
        &store,
        &fake,
        &id,
        &["out.txt".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap_err();

    assert!(error.to_string().contains("must be clean"), "{error:#}");
    assert!(fake.captured.borrow().is_empty());
    assert!(store.read_result(&id).unwrap().is_none());
}

#[test]
fn library_completion_refuses_an_unrecorded_linka_output_before_capture() {
    let (_t, store) = temp_store();
    let setup = FakeVcs::default();
    let id = add(&store, &setup, new_node("a", vec![])).unwrap();
    let fake = FakeVcs {
        root: Some("unrecorded-output".into()),
        next_id: "must-not-be-captured".into(),
        linka_nodes: [("unrecorded-output".into(), id.to_string())].into(),
        ..Default::default()
    };

    let error = complete(
        &store,
        &fake,
        &id,
        &["out.txt".into()],
        &[],
        None,
        "",
        Author::Human,
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("has never recorded"),
        "{error:#}"
    );
    assert!(fake.captured.borrow().is_empty());
    assert!(store.read_result(&id).unwrap().is_none());
}
