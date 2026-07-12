//! The Linka adapter against a real workbench: two git repositories, a real
//! store, and the real `git` CLI — the same contract the fake satisfies.

use orka::linka_graph::LinkaWorkGraph;
use orka::ports::{NodeId, SubmitOutcome, Submission, WorkGraph, WorkOutcome};
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &PathBuf, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("running git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(dir: &PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.name", "orka test"]);
    git(dir, &["config", "user.email", "test@orka.invalid"]);
}

/// A real workbench: outer repo holding `.linka/`, inner `project/` repo.
fn workbench() -> (TempDir, PathBuf) {
    let root = std::env::temp_dir().join(format!("orka-linka-test-{}", ulid::Ulid::new()));
    init_repo(&root);
    init_repo(&root.join("project"));
    linka::ops::init_workbench(root.join(".linka"), None).unwrap();
    (TempDir(root.clone()), root)
}

fn add_node(root: &Path, description: &str, depends_on: Vec<String>) -> String {
    let store = linka::Store::open(root.join(".linka")).unwrap();
    let vcs = linka::GitVcs::for_store(&store);
    linka::ops::add(
        &store,
        &vcs,
        linka::ops::NewNode {
            description: description.into(),
            author: linka::Author::Human,
            assignee: None,
            depends_on,
            derived_from: vec![],
        },
    )
    .unwrap()
}

fn complete_node(root: &Path, id: &str, outputs: &[String], notes: &str) {
    let store = linka::Store::open(root.join(".linka")).unwrap();
    let vcs = linka::GitVcs::for_store(&store);
    linka::ops::complete(
        &store,
        &vcs,
        id,
        outputs,
        &[],
        None,
        notes,
        linka::Author::Human,
    )
    .unwrap();
}

#[test]
fn freeze_carries_the_pinned_graph_and_project_anchor() {
    let (_temp, root) = workbench();
    let project = root.join("project");

    let dep = add_node(&root, "The dependency\n\nBuild the base.", vec![]);
    std::fs::write(project.join("base.txt"), "base\n").unwrap();
    complete_node(&root, &dep, &["base.txt".into()], "base built");
    let work = add_node(&root, "The work\n\nUse the base.", vec![dep.clone()]);

    let graph = LinkaWorkGraph::open(root.join(".linka")).unwrap();
    let ready = graph.select_ready().unwrap();
    assert_eq!(
        ready.iter().map(|w| w.id.0.as_str()).collect::<Vec<_>>(),
        vec![work.as_str()],
        "only the unblocked node is ready"
    );
    assert_eq!(ready[0].title, "The work");

    let frozen = graph.freeze(&NodeId(work.clone())).unwrap();
    assert_eq!(frozen.input_commit, git(&project, &["rev-parse", "HEAD"]));
    assert_eq!(frozen.description, "The work\n\nUse the base.");
    assert_eq!(frozen.dependencies.len(), 1);
    let pinned = &frozen.dependencies[0];
    assert_eq!(pinned.id.0, dep);
    assert_eq!(pinned.title, "The dependency");
    assert_eq!(pinned.result_notes, "base built");
    assert!(pinned.output.is_some(), "dependency output commit is pinned");

    // A blocked node refuses to freeze.
    let blocked = add_node(&root, "Blocked work", vec![work.clone()]);
    assert!(graph.freeze(&NodeId(blocked)).is_err());
}

#[test]
fn submission_completes_the_node_while_the_graph_is_unmoved() {
    let (_temp, root) = workbench();
    let project = root.join("project");
    let work = add_node(&root, "Write the file", vec![]);

    let graph = LinkaWorkGraph::open(root.join(".linka")).unwrap();
    let frozen = graph.freeze(&NodeId(work.clone())).unwrap();

    std::fs::write(project.join("out.txt"), "produced\n").unwrap();
    let outcome = graph
        .submit(&Submission {
            frozen,
            outcome: WorkOutcome::Succeeded {
                outputs: vec!["out.txt".into()],
                message: Some("write the file".into()),
                notes: "wrote it".into(),
            },
            workspace: None,
        })
        .unwrap();
    let SubmitOutcome::Accepted { output_commit } = outcome else {
        panic!("expected acceptance, got {outcome:?}");
    };
    let commit = output_commit.expect("an output commit");
    assert_eq!(git(&project, &["rev-parse", "HEAD"]), commit);

    let store = linka::Store::open(root.join(".linka")).unwrap();
    let vcs = linka::GitVcs::for_store(&store);
    assert!(linka::ops::node_state(&store, &vcs, &work)
        .unwrap()
        .is_complete());
}

#[test]
fn submission_is_refused_when_the_graph_moved_after_freeze() {
    let (_temp, root) = workbench();
    let work = add_node(&root, "Moving target", vec![]);

    let graph = LinkaWorkGraph::open(root.join(".linka")).unwrap();
    let frozen = graph.freeze(&NodeId(work.clone())).unwrap();

    // The definition moves between freeze and submit.
    let store = linka::Store::open(root.join(".linka")).unwrap();
    let vcs = linka::GitVcs::for_store(&store);
    linka::ops::edit(&store, &vcs, &work, "Moving target, redefined".into()).unwrap();

    for outcome in [
        WorkOutcome::Succeeded {
            outputs: vec![],
            message: None,
            notes: "done".into(),
        },
        WorkOutcome::Failed {
            notes: "gave up".into(),
        },
    ] {
        let result = graph
            .submit(&Submission {
                frozen: frozen.clone(),
                outcome,
                workspace: None,
            })
            .unwrap();
        assert!(
            matches!(result, SubmitOutcome::Stale { .. }),
            "stale work must never silently complete: {result:?}"
        );
    }
    assert!(
        store.read_result(&work).unwrap().is_none(),
        "nothing was recorded for the stale submissions"
    );
}

#[test]
fn failure_is_recorded_as_evidence_without_completing() {
    let (_temp, root) = workbench();
    let work = add_node(&root, "Doomed work", vec![]);

    let graph = LinkaWorkGraph::open(root.join(".linka")).unwrap();
    let frozen = graph.freeze(&NodeId(work.clone())).unwrap();
    let outcome = graph
        .submit(&Submission {
            frozen,
            outcome: WorkOutcome::Failed {
                notes: "the approach does not work".into(),
            },
            workspace: None,
        })
        .unwrap();
    assert!(matches!(
        outcome,
        SubmitOutcome::Accepted { output_commit: None }
    ));

    let store = linka::Store::open(root.join(".linka")).unwrap();
    let vcs = linka::GitVcs::for_store(&store);
    let state = linka::ops::node_state(&store, &vcs, &work).unwrap();
    assert!(!state.is_complete());
    assert!(state.is_ready(), "a failed node can be retried");
}
