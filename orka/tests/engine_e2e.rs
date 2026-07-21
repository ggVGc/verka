//! The whole lifecycle against real everything except the container engine:
//! a real workbench (two git repositories), a real Linka store, real worktree
//! workspaces, and the real attempt store. The executor is a fake whose
//! "agent" writes into the mounted workspace and declares its outcome, exactly
//! as an isolated command would through the mounts.

mod common;

use common::*;
use linka::Store;
use orka::attempt::{AttemptId, AttemptPhase, FsAttemptStore, SealedState};
use orka::candidate::Candidates;
use orka::engine::{Engine, ExecutionPolicy, RunProgress};
use orka::executor::{ExecutionReport, ExecutionSpec};
use orka::fakes::FakeExecutor;
use orka::linka_work::LinkaWork;
use orka::workspace::{CleanupOutcome, GitWorkspaces};
use std::path::Path;

/// Locate a writable mount by its destination inside the environment.
fn mount<'a>(spec: &'a ExecutionSpec, destination: &str) -> &'a Path {
    &spec
        .mounts
        .iter()
        .find(|m| m.destination == Path::new(destination))
        .expect("mount")
        .source
}

/// An executor whose agent writes `greeting.txt` into the workspace and
/// declares it as its output.
fn conforming_agent() -> FakeExecutor {
    FakeExecutor {
        transcript: "agent transcript\n".into(),
        on_run: Some(Box::new(|spec: &ExecutionSpec| {
            std::fs::write(
                mount(spec, "/tmp/orka/workspace").join("greeting.txt"),
                "hello\n",
            )?;
            std::fs::write(
                mount(spec, "/tmp/orka/exchange").join("outcome.toml"),
                "outcome = \"succeeded\"\noutputs = [\"greeting.txt\"]\n\
                 message = \"add the greeting\"\nnotes = \"wrote greeting.txt\"\n",
            )?;
            Ok(())
        })),
        ..Default::default()
    }
}

/// An executor that declares the given outcome TOML and exits with `exit_code`.
fn declaring_agent(outcome_toml: &'static str, exit_code: i32) -> FakeExecutor {
    FakeExecutor {
        exit_code,
        on_run: Some(Box::new(move |spec: &ExecutionSpec| {
            std::fs::write(
                mount(spec, "/tmp/orka/exchange").join("outcome.toml"),
                outcome_toml,
            )?;
            Ok(())
        })),
        ..Default::default()
    }
}

macro_rules! engine {
    ($root:expr, $store:expr, $executor:expr, $workspaces:expr, $attempts:expr) => {
        Engine {
            linka: LinkaWork::new(&$store),
            executor: &$executor,
            workspaces: &$workspaces,
            attempts: &$attempts,
            policy: ExecutionPolicy::new(vec!["agent".into()]),
        }
    };
}

fn parts(root: &Path) -> (Store, GitWorkspaces, FsAttemptStore) {
    (
        store_at(root),
        GitWorkspaces::new(root.join("project"), root.join(".orka/worktrees")),
        FsAttemptStore::new(root.join(".orka")),
    )
}

#[test]
fn a_full_attempt_lands_a_version_checked_result_from_an_isolated_worktree() {
    let (_temp, root) = workbench();
    let project = root.join("project");
    let node = add_node(&root, "Greet\n\nCreate greeting.txt saying hello.", vec![]);
    let user_head = git(&project, &["rev-parse", "HEAD"]);

    // The user's checkout is dirty — an isolated attempt must not care.
    std::fs::write(project.join("wip.txt"), "user's half-done work\n").unwrap();

    let (store, workspaces, attempts) = parts(&root);
    let executor = conforming_agent();
    let engine = engine!(&root, store, executor, workspaces, attempts);

    let mut progress = Vec::new();
    let report = engine
        .run_next_with_progress(&mut |event| progress.push(event.clone()))
        .unwrap()
        .expect("the node is ready");
    assert!(matches!(
        &progress[..],
        [
            RunProgress::Selected { .. },
            RunProgress::AttemptCreated { .. },
            RunProgress::WorkspacePrepared { .. },
            RunProgress::ExecutionStarted { .. },
            RunProgress::ExecutionFinished { exit_code: 0, .. },
            RunProgress::Sealed { .. },
        ]
    ));
    assert_eq!(report.node.as_str(), node);
    let SealedState::Submitted { output_commit } = &report.sealed else {
        panic!("expected submission, got {:?}", report.sealed);
    };
    let output = output_commit.clone().expect("an output commit");
    assert_eq!(report.cleanup, CleanupOutcome::Removed);

    // The result is recorded and pinned. The node is succeeded-but-not-complete:
    // the output lives on the candidate branch, and until it is published into
    // the checkout Linka truthfully reports the output as drifted there.
    let vcs = linka::GitVcs::for_store(&store);
    assert_eq!(
        linka::ops::output_of(&store, &node).unwrap().unwrap(),
        output
    );
    let state = linka::ops::node_state(&store, &vcs, &node).unwrap();
    assert_eq!(state.outcome, linka::RecordedOutcome::Succeeded);
    assert!(!state.is_complete());

    // The output sits on the attempt's candidate branch, parented on the frozen
    // input commit, and carries exactly the declared file.
    let branch = format!("orka/attempts/{}", report.attempt);
    assert_eq!(git(&project, &["rev-parse", &branch]), output);
    assert_eq!(
        git(&project, &["rev-parse", &format!("{output}^")]),
        user_head
    );
    assert_eq!(
        git(&project, &["show", &format!("{output}:greeting.txt")]),
        "hello"
    );

    // The user's checkout never moved and keeps its uncommitted work.
    assert_eq!(git(&project, &["rev-parse", "HEAD"]), user_head);
    assert_eq!(
        std::fs::read_to_string(project.join("wip.txt")).unwrap(),
        "user's half-done work\n"
    );

    // The attempt record durably stores Linka's exact snapshot.
    let snapshot = attempts.load(&report.attempt).unwrap();
    assert_eq!(snapshot.phase(), AttemptPhase::Sealed);
    assert_eq!(snapshot.record.input.input_commit(), user_head);
    assert_eq!(snapshot.record.input.snapshot.node.as_str(), node);
    assert_eq!(
        std::fs::read_to_string(attempts.transcript_path(&report.attempt)).unwrap(),
        "agent transcript\n"
    );
    let (attachment, attached_transcript) = store
        .read_node_attachment(&node, "orka", &format!("{}/transcript", report.attempt))
        .unwrap()
        .expect("Orka transcript attached to its Linka node");
    assert_eq!(
        attachment.media_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(attached_transcript, b"agent transcript\n");

    // Orka exposes the candidate with its source node and its complete patch.
    let candidates = Candidates::new(&store, &attempts);
    let listed = candidates.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].attempt.as_ref(), Some(&report.attempt));
    assert_eq!(listed[0].node.as_str(), node);
    assert_eq!(listed[0].integration, linka::IntegrationStatus::Pending);
    assert_eq!(report.candidate.as_ref(), Some(&listed[0].id));
    assert!(candidates
        .patch(&report.attempt.0)
        .unwrap()
        .contains("greeting.txt"));

    // Acceptance is durable and separate from publication. Publication then
    // refuses to trample a dirty human checkout.
    let accepted = candidates
        .accept(&listed[0].id.0, "looks good".into())
        .unwrap();
    assert_eq!(accepted.integration, linka::IntegrationStatus::Accepted);
    let error = candidates.publish(&listed[0].id.0).unwrap_err();
    assert!(error.to_string().contains("checkout is dirty"), "{error:#}");

    // A human accepts through Orka. The node is then complete and no machine
    // work is ready; repeating publication is harmless.
    std::fs::remove_file(project.join("wip.txt")).unwrap();
    let published = candidates.publish(&listed[0].id.0).unwrap();
    assert_eq!(published.integration, linka::IntegrationStatus::Published);
    assert_eq!(published.head_commit, output);
    assert_eq!(
        candidates.publish(&listed[0].id.0).unwrap().integration,
        linka::IntegrationStatus::Published
    );
    assert!(linka::ops::node_state(&store, &vcs, &node)
        .unwrap()
        .is_complete());
    assert!(LinkaWork::new(&store)
        .ready_for_machine()
        .unwrap()
        .is_empty());
}

#[test]
fn graph_only_success_records_a_result_with_no_project_commit() {
    let (_temp, root) = workbench();
    let project = root.join("project");
    let node = add_node(&root, "Answer the question", vec![]);
    let head = git(&project, &["rev-parse", "HEAD"]);

    let (store, workspaces, attempts) = parts(&root);
    let executor = declaring_agent("outcome = \"succeeded\"\nnotes = \"answered\"\n", 0);
    let engine = engine!(&root, store, executor, workspaces, attempts);
    let report = engine.run_node(&node.parse().unwrap()).unwrap();

    assert_eq!(
        report.sealed,
        SealedState::Submitted {
            output_commit: None
        }
    );
    assert_eq!(
        git(&project, &["rev-parse", "HEAD"]),
        head,
        "no project commit"
    );
    let vcs = linka::GitVcs::for_store(&store);
    assert!(linka::ops::node_state(&store, &vcs, &node)
        .unwrap()
        .is_complete());
}

#[test]
fn graph_only_success_refuses_undeclared_workspace_changes() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Claimed graph-only work", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor {
        on_run: Some(Box::new(|spec: &ExecutionSpec| {
            std::fs::write(
                mount(spec, "/tmp/orka/workspace").join("undeclared.txt"),
                "hidden\n",
            )?;
            std::fs::write(
                mount(spec, "/tmp/orka/exchange").join("outcome.toml"),
                "outcome = \"succeeded\"\nnotes = \"graph only\"\n",
            )?;
            Ok(())
        })),
        ..Default::default()
    };
    let engine = engine!(&root, store, executor, workspaces, attempts);

    let error = engine.run_node(&node.parse().unwrap()).unwrap_err();
    assert!(error.to_string().contains("undeclared"), "{error:#}");
    assert!(store.read_result(&node).unwrap().is_none());

    let attempt = attempts.list().unwrap().into_iter().next().unwrap();
    let snapshot = attempts.load(&attempt).unwrap();
    assert_eq!(snapshot.phase(), AttemptPhase::Executed);
    let workspace = snapshot.workspace.unwrap();
    assert!(workspace.path.join("undeclared.txt").is_file());
}

#[test]
fn a_declared_failure_is_recorded_as_evidence() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Doomed", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = declaring_agent("outcome = \"failed\"\nnotes = \"cannot\"\n", 2);
    let engine = engine!(&root, store, executor, workspaces, attempts);
    let report = engine.run_node(&node.parse().unwrap()).unwrap();

    assert_eq!(report.sealed, SealedState::FailureRecorded);
    assert!(report.backend_failed, "the nonzero exit rides along");
    let vcs = linka::GitVcs::for_store(&store);
    let state = linka::ops::node_state(&store, &vcs, &node).unwrap();
    assert_eq!(state.outcome, linka::RecordedOutcome::Failed);
}

#[test]
fn a_nonzero_exit_with_a_declared_success_still_submits_and_reports() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Flaky but done", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    // The agent produces its output and declares success but exits nonzero.
    let executor = FakeExecutor {
        exit_code: 3,
        on_run: Some(Box::new(|spec: &ExecutionSpec| {
            std::fs::write(mount(spec, "/tmp/orka/workspace").join("out.txt"), "x\n")?;
            std::fs::write(
                mount(spec, "/tmp/orka/exchange").join("outcome.toml"),
                "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\nnotes = \"done\"\n",
            )?;
            Ok(())
        })),
        ..Default::default()
    };
    let engine = engine!(&root, store, executor, workspaces, attempts);
    let report = engine.run_node(&node.parse().unwrap()).unwrap();

    assert!(matches!(report.sealed, SealedState::Submitted { .. }));
    assert!(report.backend_failed);
}

#[test]
fn an_executor_error_discards_an_unchanged_attempt_completely() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Backend cannot start", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor {
        on_run: Some(Box::new(|_| anyhow::bail!("backend refused the request"))),
        ..Default::default()
    };
    let engine = engine!(&root, store, executor, workspaces, attempts);
    let mut progress = Vec::new();

    let error = engine
        .run_node_with_progress(&node.parse().unwrap(), &mut |event| {
            progress.push(event.clone())
        })
        .unwrap_err();

    assert!(error.to_string().contains("backend refused"), "{error:#}");
    let attempt = progress
        .iter()
        .find_map(|event| match event {
            RunProgress::AttemptCreated { attempt } => Some(attempt.clone()),
            _ => None,
        })
        .unwrap();
    assert!(attempts.list().unwrap().is_empty());
    assert!(
        !workspaces.path_for(&attempt.0).exists(),
        "a backend startup error must not leave a clean worktree behind"
    );
    assert!(
        git(
            &root.join("project"),
            &["branch", "--list", &format!("orka/attempts/{attempt}")]
        )
        .is_empty(),
        "an empty attempt must not leave a candidate branch behind"
    );
    assert!(!progress
        .iter()
        .any(|event| matches!(event, RunProgress::Sealed { .. })));
}

#[test]
fn an_executor_error_retains_a_worktree_it_may_have_changed() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Backend fails after starting", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor {
        on_run: Some(Box::new(|spec: &ExecutionSpec| {
            std::fs::write(
                mount(spec, "/tmp/orka/workspace").join("partial.txt"),
                "partial\n",
            )?;
            anyhow::bail!("backend connection was lost")
        })),
        ..Default::default()
    };
    let engine = engine!(&root, store, executor, workspaces, attempts);

    engine.run_node(&node.parse().unwrap()).unwrap_err();

    let attempt = attempts.list().unwrap().pop().unwrap();
    let snapshot = attempts.load(&attempt).unwrap();
    assert!(matches!(
        snapshot.seal.unwrap().state,
        SealedState::Interrupted { .. }
    ));
    assert!(
        snapshot
            .workspace
            .unwrap()
            .path
            .join("partial.txt")
            .is_file(),
        "possibly useful partial work must be retained"
    );
}

#[test]
fn an_attempt_against_a_graph_that_moved_mid_run_seals_stale() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Moving target", vec![]);

    let (store, workspaces, attempts) = parts(&root);
    // This "agent" edits the node's definition mid-run, then declares success.
    let node_for_agent = node.clone();
    let root_for_agent = root.clone();
    let executor = FakeExecutor {
        transcript: "stale transcript\n".into(),
        on_run: Some(Box::new(move |spec: &ExecutionSpec| {
            edit_node(&root_for_agent, &node_for_agent, "Moved mid-run");
            std::fs::write(mount(spec, "/tmp/orka/workspace").join("out.txt"), "x\n")?;
            std::fs::write(
                mount(spec, "/tmp/orka/exchange").join("outcome.toml"),
                "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\nnotes = \"n\"\n",
            )?;
            Ok(())
        })),
        ..Default::default()
    };
    let engine = engine!(&root, store, executor, workspaces, attempts);
    let report = engine.run_node(&node.parse().unwrap()).unwrap();

    let SealedState::StaleAtSubmit { conflicts } = &report.sealed else {
        panic!(
            "stale work must never silently complete: {:?}",
            report.sealed
        );
    };
    assert!(!conflicts.is_empty());
    assert!(store.read_result(&node).unwrap().is_none());
    assert_eq!(
        store
            .read_node_attachment(&node, "orka", &format!("{}/transcript", report.attempt))
            .unwrap()
            .unwrap()
            .1,
        b"stale transcript\n"
    );

    // Linka records no candidate for a result it rejected. The attempt and
    // retained branch remain Orka evidence for inspection/recovery.
    let candidates = Candidates::new(&store, &attempts);
    assert!(candidates.list().unwrap().is_empty());
    let error = candidates.get(&report.attempt.0).unwrap_err();
    assert!(
        error.to_string().contains("no Linka candidate"),
        "{error:#}"
    );
}

#[test]
fn recovery_settles_an_executed_attempt_and_a_second_pass_duplicates_nothing() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Crashed after evidence", vec![]);

    // Build an attempt that crashed after evidence capture, before sealing:
    // everything recorded, an outcome declared, nothing sealed.
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor::default();
    let engine = engine!(&root, store, executor, workspaces, attempts);

    let id = AttemptId::new();
    let input = LinkaWork::new(&store)
        .prepare_input(&node.parse().unwrap())
        .unwrap();
    attempts.create(&id, &input).unwrap();
    let ws = workspaces_prepare(&root, &id, input.input_commit());
    attempts.plan_workspace(&id, &ws).unwrap();
    attempts.mark_prepared(&id).unwrap();
    std::fs::write(ws.path.join("out.txt"), "produced\n").unwrap();
    let io = attempts.io_dir(&id).unwrap();
    std::fs::write(
        io.join("outcome.toml"),
        "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\nnotes = \"done before the crash\"\n",
    )
    .unwrap();
    std::fs::write(attempts.transcript_path(&id), "recovered transcript\n").unwrap();
    attempts
        .record_evidence(
            &id,
            &ExecutionReport {
                backend: "fake".into(),
                backend_reference: None,
                exit_code: 0,
                started_at_ms: 1,
                finished_at_ms: 2,
            },
        )
        .unwrap();

    let reports = engine.recover().unwrap();
    assert_eq!(reports.len(), 1);
    assert!(matches!(
        reports[0].sealed,
        Some(SealedState::Submitted { .. })
    ));
    assert!(store.read_result(&node).unwrap().is_some());
    assert_eq!(attempts.load(&id).unwrap().phase(), AttemptPhase::Sealed);

    // A second recovery pass performs no duplicate mutation.
    let (result_before, _) = store.read_result(&node).unwrap().unwrap();
    let again = engine.recover().unwrap();
    assert_eq!(again.len(), 1);
    let (result_after, _) = store.read_result(&node).unwrap().unwrap();
    assert_eq!(
        result_before.at, result_after.at,
        "the recorded result is unchanged"
    );
}

#[test]
fn recovery_after_linka_accepted_but_before_seal_recognizes_its_own_result() {
    // The crash window: Linka accepted the result but Orka never sealed. The
    // A naive resubmit would conflict. Recovery must recognize its own result
    // and finish the missing Linka candidate registration before sealing.
    let (_temp, root) = workbench();
    let node = add_node(&root, "Accepted then crashed", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor::default();
    let engine = engine!(&root, store, executor, workspaces, attempts);

    let id = AttemptId::new();
    let linka = LinkaWork::new(&store);
    let input = linka.prepare_input(&node.parse().unwrap()).unwrap();
    attempts.create(&id, &input).unwrap();
    let ws = workspaces_prepare(&root, &id, input.input_commit());
    attempts.plan_workspace(&id, &ws).unwrap();
    attempts.mark_prepared(&id).unwrap();
    std::fs::write(ws.path.join("out.txt"), "recovered output\n").unwrap();
    let io = attempts.io_dir(&id).unwrap();
    std::fs::write(
        io.join("outcome.toml"),
        "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\nnotes = \"done\"\n",
    )
    .unwrap();
    let evidence = ExecutionReport {
        backend: "fake".into(),
        backend_reference: None,
        exit_code: 0,
        started_at_ms: 1,
        finished_at_ms: 2,
    };
    std::fs::write(attempts.transcript_path(&id), "accepted transcript\n").unwrap();
    attempts.record_evidence(&id, &evidence).unwrap();

    // Linka accepts and captures the result, attributed to this attempt — then
    // "the crash" happens before candidate registration and Orka sealing.
    linka
        .submit_success(
            &input,
            &ws.path,
            &["out.txt".parse().unwrap()],
            None,
            "done".into(),
            orka::linka_work::producer_evidence(&id, &evidence),
        )
        .unwrap();
    assert!(attempts.load(&id).unwrap().seal.is_none(), "not yet sealed");

    let reports = engine.recover().unwrap();
    assert!(
        matches!(reports[0].sealed, Some(SealedState::Submitted { .. })),
        "recovery recognized its own accepted result: {:?}",
        reports[0].sealed
    );
    let candidate = Candidates::new(&store, &attempts).get(&id.0).unwrap();
    assert_eq!(candidate.node.as_str(), node);
    assert_eq!(candidate.integration, linka::IntegrationStatus::Pending);
}

#[test]
fn recovery_discards_an_unchanged_pre_evidence_attempt() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Never ran", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor::default();
    let engine = engine!(&root, store, executor, workspaces, attempts);

    let id = AttemptId::new();
    let input = LinkaWork::new(&store)
        .prepare_input(&node.parse().unwrap())
        .unwrap();
    attempts.create(&id, &input).unwrap();
    let ws = workspaces_prepare(&root, &id, input.input_commit());
    attempts.plan_workspace(&id, &ws).unwrap();
    attempts.mark_prepared(&id).unwrap();

    let reports = engine.recover().unwrap();
    assert_eq!(reports[0].sealed, None);
    assert!(reports[0].action.contains("discarded empty"));
    assert!(store.read_result(&node).unwrap().is_none());
    assert!(!ws.path.exists(), "the untouched workspace was cleaned");
    assert!(attempts.list().unwrap().is_empty());
}

#[test]
fn recovery_prunes_a_legacy_empty_interrupted_attempt() {
    use orka::workspace::WorkspaceManager;

    let (_temp, root) = workbench();
    let node = add_node(&root, "Old backend failure", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor::default();
    let engine = engine!(&root, store, executor, workspaces, attempts);

    let id = AttemptId::new();
    let input = LinkaWork::new(&store)
        .prepare_input(&node.parse().unwrap())
        .unwrap();
    attempts.create(&id, &input).unwrap();
    let ws = workspaces_prepare(&root, &id, input.input_commit());
    attempts.plan_workspace(&id, &ws).unwrap();
    attempts.mark_prepared(&id).unwrap();
    attempts
        .seal(
            &id,
            SealedState::Interrupted {
                reason: "execution failed before exit evidence".into(),
            },
        )
        .unwrap();
    assert_eq!(workspaces.cleanup(&ws).unwrap(), CleanupOutcome::Removed);

    let reports = engine.recover().unwrap();
    assert!(reports[0].action.contains("discarded empty"));
    assert!(attempts.list().unwrap().is_empty());
    assert!(git(&root.join("project"), &["branch", "--list", &ws.branch]).is_empty());
}

#[test]
fn recovery_attaches_a_transcript_from_an_older_sealed_attempt() {
    let (_temp, root) = workbench();
    let node = add_node(&root, "Historical transcript", vec![]);
    let (store, workspaces, attempts) = parts(&root);
    let executor = FakeExecutor::default();
    let engine = engine!(&root, store, executor, workspaces, attempts);

    let id = AttemptId::new();
    let input = LinkaWork::new(&store)
        .prepare_input(&node.parse().unwrap())
        .unwrap();
    attempts.create(&id, &input).unwrap();
    std::fs::write(attempts.transcript_path(&id), "historical agent output\n").unwrap();
    attempts
        .seal(
            &id,
            SealedState::ContractViolation {
                reason: "old sealed attempt".into(),
            },
        )
        .unwrap();

    engine.recover().unwrap();

    assert_eq!(
        store
            .read_node_attachment(&node, "orka", &format!("{id}/transcript"))
            .unwrap()
            .unwrap()
            .1,
        b"historical agent output\n"
    );
}

// A standalone worktree preparation matching what the engine would do, so the
// recovery tests can stage a mid-lifecycle attempt.
fn workspaces_prepare(
    root: &Path,
    id: &AttemptId,
    input_commit: &str,
) -> orka::workspace::PreparedWorkspace {
    use orka::workspace::WorkspaceManager;
    GitWorkspaces::new(root.join("project"), root.join(".orka/worktrees"))
        .prepare(&id.0, input_commit)
        .unwrap()
}
