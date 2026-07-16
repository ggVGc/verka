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
                input_commit: "base".into(),
                target: "main".into(),
                external: Some(ExternalIdentity {
                    namespace: "test-runner".into(),
                    id: "run-1".into(),
                }),
                producer: None,
            },
        )
        .unwrap()
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
    candidates
        .accept(&vcs, &candidate.id, Author::Human, "looks good".into())
        .unwrap();
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
    CandidateStore::new(&store)
        .reject(&vcs, &candidate.id, Author::Human, "needs changes".into())
        .unwrap();
    let state = ops::node_state(&store, &vcs, node.as_str()).unwrap();
    assert_eq!(state.integration, IntegrationStatus::Rejected);
    assert!(state.is_ready());
    assert_eq!(
        CandidateStore::new(&store).for_node(&node).unwrap().len(),
        1
    );
}

#[test]
fn a_moved_source_cannot_accept_an_obsolete_candidate() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    ops::edit(&store, &vcs, node.as_str(), "candidate work changed".into()).unwrap();
    let error = CandidateStore::new(&store)
        .accept(&vcs, &candidate.id, Author::Human, String::new())
        .unwrap_err();
    assert!(error.to_string().contains("not the current"), "{error:#}");
}

#[test]
fn publication_is_derived_and_target_corruption_is_detected() {
    let (_temp, store, vcs, node, _) = successful_output();
    let candidate = register(&store, &vcs, &node);
    let candidates = CandidateStore::new(&store);
    candidates
        .accept(&vcs, &candidate.id, Author::Human, String::new())
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
