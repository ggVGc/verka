//! The whole lifecycle against real everything except the container engine:
//! a real workbench (two git repositories), the real Linka adapter, real
//! worktree workspaces, and the real attempt store. The executor is a fake
//! whose "agent" writes into the mounted workspace and declares its outcome,
//! exactly as an isolated command would through the mounts.

use orka::attempt::{AttemptPhase, FsAttemptStore, SealedState};
use orka::engine::{Engine, ExecutionPolicy};
use orka::fakes::FakeExecutor;
use orka::linka_graph::LinkaWorkGraph;
use orka::ports::{CleanupOutcome, ExecutionSpec, NodeId, WorkGraph};
use orka::workspace::GitWorkspaces;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
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

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.name", "orka test"]);
    git(dir, &["config", "user.email", "test@orka.invalid"]);
}

fn workbench() -> (TempDir, PathBuf) {
    let root = std::env::temp_dir().join(format!("orka-e2e-{}", ulid::Ulid::new()));
    init_repo(&root);
    init_repo(&root.join("project"));
    linka::ops::init_workbench(root.join(".linka"), None).unwrap();
    (TempDir(root.clone()), root)
}

fn add_node(root: &Path, description: &str) -> String {
    let store = linka::Store::open(root.join(".linka")).unwrap();
    let vcs = linka::GitVcs::for_store(&store);
    linka::ops::add(
        &store,
        &vcs,
        linka::ops::NewNode {
            description: description.into(),
            author: linka::Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
        },
    )
    .unwrap()
}

/// An executor whose agent writes `greeting.txt` into the mounted workspace
/// and declares it as its output.
fn conforming_agent() -> FakeExecutor {
    FakeExecutor {
        transcript: "agent transcript\n".into(),
        on_run: Some(Box::new(|spec: &ExecutionSpec| {
            let workspace = &spec
                .mounts
                .iter()
                .find(|m| m.destination == Path::new("/workspace"))
                .expect("workspace mount")
                .source;
            let io = &spec
                .mounts
                .iter()
                .find(|m| m.destination == Path::new("/orka"))
                .expect("io mount")
                .source;
            std::fs::write(workspace.join("greeting.txt"), "hello from the agent\n")?;
            std::fs::write(
                io.join("outcome.toml"),
                "outcome = \"succeeded\"\noutputs = [\"greeting.txt\"]\n\
                 message = \"add the greeting\"\nnotes = \"wrote greeting.txt\"\n",
            )?;
            Ok(())
        })),
        ..Default::default()
    }
}

#[test]
fn a_full_attempt_lands_a_version_checked_result_from_an_isolated_worktree() {
    let (_temp, root) = workbench();
    let project = root.join("project");
    let node = add_node(&root, "Greet\n\nCreate greeting.txt saying hello.");
    let user_head = git(&project, &["rev-parse", "HEAD"]);

    // The user's checkout is dirty — an isolated attempt must not care.
    std::fs::write(project.join("wip.txt"), "user's half-done work\n").unwrap();

    let graph = LinkaWorkGraph::open(root.join(".linka")).unwrap();
    let executor = conforming_agent();
    let workspaces = GitWorkspaces::new(&project, root.join(".orka/worktrees"));
    let attempts = FsAttemptStore::new(root.join(".orka"));
    let engine = Engine {
        graph: &graph,
        executor: &executor,
        workspaces: &workspaces,
        attempts: &attempts,
        policy: ExecutionPolicy::new(vec!["agent".into()]),
    };

    let report = engine.run_next().unwrap().expect("the node is ready");
    assert_eq!(report.node.0, node);
    let SealedState::Submitted { output_commit } = &report.sealed else {
        panic!("expected submission, got {:?}", report.sealed);
    };
    let output = output_commit.clone().expect("an output commit");
    assert_eq!(report.cleanup, CleanupOutcome::Removed);

    // The result is recorded and pinned to the output commit. The node is
    // *succeeded but not complete*: the output lives on the candidate branch,
    // and until someone publishes it into the project checkout, linka
    // truthfully reports the output as drifted (absent) there. Publication is
    // a review decision and explicitly not Orka's job.
    let store = linka::Store::open(root.join(".linka")).unwrap();
    let vcs = linka::GitVcs::for_store(&store);
    assert_eq!(linka::ops::output_of(&store, &node).unwrap().unwrap(), output);
    let state = linka::ops::node_state(&store, &vcs, &node).unwrap();
    assert_eq!(state.outcome, linka::RecordedOutcome::Succeeded);
    assert!(!state.is_complete());
    assert!(state
        .staleness
        .iter()
        .any(|r| matches!(r, linka::StalenessReason::OutputDrifted { .. })));

    // The output commit sits on the attempt's candidate branch, parented on
    // the frozen input commit, and carries exactly the declared file.
    let branch = format!("orka/attempts/{}", report.attempt);
    assert_eq!(git(&project, &["rev-parse", &branch]), output);
    assert_eq!(
        git(&project, &["rev-parse", &format!("{output}^")]),
        user_head
    );
    let shown = git(&project, &["show", &format!("{output}:greeting.txt")]);
    assert_eq!(shown, "hello from the agent");

    // The user's checkout never moved and keeps its uncommitted work.
    assert_eq!(git(&project, &["rev-parse", "HEAD"]), user_head);
    assert_eq!(
        std::fs::read_to_string(project.join("wip.txt")).unwrap(),
        "user's half-done work\n"
    );

    // The attempt record tells the whole story.
    let snapshot = attempts.load(&report.attempt).unwrap();
    assert_eq!(snapshot.phase(), AttemptPhase::Sealed);
    assert_eq!(snapshot.record.frozen.input_commit, user_head);
    assert_eq!(
        std::fs::read_to_string(attempts.transcript_path(&report.attempt)).unwrap(),
        "agent transcript\n"
    );

    // A human accepts the candidate: clean the checkout and fast-forward the
    // checked-out branch onto the output. Now the node is complete and no
    // work is ready.
    std::fs::remove_file(project.join("wip.txt")).unwrap();
    let current = git(&project, &["symbolic-ref", "--short", "HEAD"]);
    assert!(linka::Vcs::publish_fast_forward(&vcs, &current, &user_head, &output).unwrap());
    assert!(linka::ops::node_state(&store, &vcs, &node)
        .unwrap()
        .is_complete());
    assert!(graph.select_ready().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(project.join("greeting.txt")).unwrap(),
        "hello from the agent\n"
    );
}

#[test]
fn an_attempt_against_a_graph_that_moved_mid_run_seals_stale() {
    let (_temp, root) = workbench();
    let project = root.join("project");
    let node = add_node(&root, "Moving target");

    let graph = LinkaWorkGraph::open(root.join(".linka")).unwrap();
    // This "agent" edits the node's definition mid-run (as a human racing the
    // orchestrator would), then declares success.
    let node_for_agent = node.clone();
    let root_for_agent = root.clone();
    let executor = FakeExecutor {
        on_run: Some(Box::new(move |spec: &ExecutionSpec| {
            let store = linka::Store::open(root_for_agent.join(".linka")).unwrap();
            let vcs = linka::GitVcs::for_store(&store);
            linka::ops::edit(&store, &vcs, &node_for_agent, "Moved mid-run".into()).unwrap();
            let io = &spec
                .mounts
                .iter()
                .find(|m| m.destination == Path::new("/orka"))
                .unwrap()
                .source;
            std::fs::write(io.join("outcome.toml"), "outcome = \"succeeded\"\n")?;
            Ok(())
        })),
        ..Default::default()
    };
    let workspaces = GitWorkspaces::new(&project, root.join(".orka/worktrees"));
    let attempts = FsAttemptStore::new(root.join(".orka"));
    let engine = Engine {
        graph: &graph,
        executor: &executor,
        workspaces: &workspaces,
        attempts: &attempts,
        policy: ExecutionPolicy::new(vec!["agent".into()]),
    };

    let report = engine.run_node(&NodeId(node.clone())).unwrap();
    assert!(
        matches!(report.sealed, SealedState::StaleAtSubmit { .. }),
        "stale work must never silently complete: {:?}",
        report.sealed
    );
    let store = linka::Store::open(root.join(".linka")).unwrap();
    assert!(store.read_result(&node).unwrap().is_none());
}
