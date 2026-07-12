//! The attempt lifecycle: select → freeze → record → prepare → execute →
//! decide → version-checked submit → seal → clean up.
//!
//! Every step that has an external side effect is durably recorded before it
//! happens, so [`Engine::recover`] can classify any crash by the files the
//! attempt store holds and finish the idempotent remainder. Recovery never
//! invents results: an attempt without exit evidence seals as interrupted,
//! and a dirty or missing workspace is reported, not discarded or guessed at.

use crate::attempt::{AttemptId, AttemptPhase, FsAttemptStore, SealedState};
use crate::outcome::{self, Decision, PROMPT_FILE};
use crate::ports::{
    CleanupOutcome, ExecutionSpec, FrozenInput, IsolatedExecutor, MountSpec, NodeId,
    PreparedWorkspace, SubmitOutcome, Submission, WorkGraph, WorkOutcome, WorkspaceManager,
};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How an agent command is granted capabilities for one attempt: the command
/// itself plus everything Orka chooses to expose. The workspace and exchange
/// directory are the only writable mounts; anything else must be listed.
#[derive(Clone, Debug)]
pub struct ExecutionPolicy {
    pub command: Vec<String>,
    /// Where the attempt worktree appears inside the environment.
    pub workspace_destination: PathBuf,
    /// Where the exchange directory (prompt in, outcome out) appears.
    pub io_destination: PathBuf,
    /// Additional explicitly chosen context mounts.
    pub extra_mounts: Vec<MountSpec>,
    pub environment: BTreeMap<String, String>,
    pub network: bool,
}

impl ExecutionPolicy {
    pub fn new(command: Vec<String>) -> Self {
        Self {
            command,
            workspace_destination: "/workspace".into(),
            io_destination: "/orka".into(),
            extra_mounts: Vec::new(),
            environment: BTreeMap::new(),
            network: false,
        }
    }
}

pub struct Engine<'a> {
    pub graph: &'a dyn WorkGraph,
    pub executor: &'a dyn IsolatedExecutor,
    pub workspaces: &'a dyn WorkspaceManager,
    pub attempts: &'a FsAttemptStore,
    pub policy: ExecutionPolicy,
}

/// What one attempt came to.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub attempt: AttemptId,
    pub node: NodeId,
    pub sealed: SealedState,
    pub exit_code: i32,
    /// The command exited nonzero even though an outcome was declared and
    /// handled — reportable backend trouble, not attempt state.
    pub backend_failed: bool,
    pub cleanup: CleanupOutcome,
}

/// What recovery did (or could not do) for one attempt.
#[derive(Clone, Debug)]
pub struct RecoveryReport {
    pub attempt: AttemptId,
    pub node: NodeId,
    pub action: String,
    pub sealed: Option<SealedState>,
}

impl Engine<'_> {
    /// Run the first ready node, if any.
    pub fn run_next(&self) -> Result<Option<RunReport>> {
        match self.graph.select_ready()?.into_iter().next() {
            Some(item) => self.run_node(&item.id).map(Some),
            None => Ok(None),
        }
    }

    /// Run one node through a complete attempt.
    pub fn run_node(&self, id: &NodeId) -> Result<RunReport> {
        if self.policy.command.is_empty() {
            bail!("no agent command configured");
        }
        // 1–2. Freeze the graph input and durably record the attempt.
        let frozen = self.graph.freeze(id)?;
        let attempt = AttemptId::new();
        self.attempts.create(&attempt, &frozen)?;

        // 3. Record the workspace plan, then create it.
        let plan = self.workspaces.plan(&attempt.0, &frozen.input_commit);
        self.attempts.plan_workspace(&attempt, &plan)?;
        let workspace = self
            .workspaces
            .prepare(&attempt.0, &frozen.input_commit)
            .with_context(|| format!("preparing workspace for {attempt}"))?;
        self.attempts.mark_prepared(&attempt)?;

        // 4. Stage the exchange directory and record the exact request.
        let io_dir = self.attempts.io_dir(&attempt)?;
        std::fs::write(io_dir.join(PROMPT_FILE), build_prompt(&frozen, &self.policy))
            .context("writing the attempt prompt")?;
        let spec = self.execution_spec(id, &workspace, &io_dir);
        self.attempts.record_request(&attempt, &spec)?;

        // 5. Execute and capture evidence.
        let report = self
            .executor
            .run(&spec, &self.attempts.transcript_path(&attempt))?;
        self.attempts.record_evidence(&attempt, &report)?;

        // 6–7. Decide from the evidence, submit version-checked, seal.
        let (sealed, backend_failed) =
            self.settle(&attempt, &frozen, &workspace, report.exit_code)?;
        let cleanup = self.workspaces.cleanup(&workspace)?;
        Ok(RunReport {
            attempt,
            node: id.clone(),
            sealed,
            exit_code: report.exit_code,
            backend_failed,
            cleanup,
        })
    }

    /// The concrete capability grant for one attempt: the worktree and the
    /// exchange directory writable, chosen context read-only, nothing else.
    fn execution_spec(
        &self,
        node: &NodeId,
        workspace: &PreparedWorkspace,
        io_dir: &std::path::Path,
    ) -> ExecutionSpec {
        let mut mounts = vec![
            MountSpec {
                source: workspace.path.clone(),
                destination: self.policy.workspace_destination.clone(),
                writable: true,
            },
            MountSpec {
                source: io_dir.to_path_buf(),
                destination: self.policy.io_destination.clone(),
                writable: true,
            },
        ];
        mounts.extend(self.policy.extra_mounts.iter().cloned());

        let mut environment = self.policy.environment.clone();
        let io = |file: &str| {
            self.policy
                .io_destination
                .join(file)
                .to_string_lossy()
                .into_owned()
        };
        environment.insert("ORKA_NODE".into(), node.0.clone());
        environment.insert(
            "ORKA_WORKSPACE".into(),
            self.policy.workspace_destination.to_string_lossy().into_owned(),
        );
        environment.insert("ORKA_PROMPT".into(), io(PROMPT_FILE));
        environment.insert("ORKA_OUTCOME".into(), io(outcome::OUTCOME_FILE));

        ExecutionSpec {
            command: self.policy.command.clone(),
            working_directory: self.policy.workspace_destination.clone(),
            mounts,
            environment,
            network: self.policy.network,
        }
    }

    /// Turn captured evidence into a sealed attempt: read the declaration,
    /// apply the failure matrix, submit anything submittable, and seal.
    /// Idempotent — recovery re-runs it for executed-but-unsealed attempts.
    fn settle(
        &self,
        attempt: &AttemptId,
        frozen: &FrozenInput,
        workspace: &PreparedWorkspace,
        exit_code: i32,
    ) -> Result<(SealedState, bool)> {
        let declared = outcome::read_declared(&self.attempts.io_dir(attempt)?)?;
        match outcome::decide(declared, exit_code) {
            Decision::Submit {
                outcome,
                backend_failed,
            } => {
                let succeeded = matches!(outcome, WorkOutcome::Succeeded { .. });
                if succeeded && !workspace.path.exists() {
                    // The declared outputs lived in the workspace; without it
                    // there is nothing to capture and nothing to submit.
                    bail!(
                        "attempt {attempt} declared success but its workspace {} is missing",
                        workspace.path.display()
                    );
                }
                let submitted = self.graph.submit(&Submission {
                    frozen: frozen.clone(),
                    outcome,
                    workspace: Some(workspace.path.clone()),
                })?;
                let sealed = match submitted {
                    SubmitOutcome::Accepted { output_commit } if succeeded => {
                        SealedState::Submitted { output_commit }
                    }
                    SubmitOutcome::Accepted { .. } => SealedState::FailureRecorded,
                    SubmitOutcome::Stale { reasons } => SealedState::StaleAtSubmit { reasons },
                };
                self.attempts.seal(attempt, sealed.clone())?;
                Ok((sealed, backend_failed))
            }
            Decision::ContractViolation { reason } => {
                let sealed = SealedState::ContractViolation { reason };
                self.attempts.seal(attempt, sealed.clone())?;
                Ok((sealed, false))
            }
            Decision::Interrupted { reason } => {
                let sealed = SealedState::Interrupted { reason };
                self.attempts.seal(attempt, sealed.clone())?;
                Ok((sealed, false))
            }
        }
    }

    /// Classify every recorded attempt and finish what can be finished.
    pub fn recover(&self) -> Result<Vec<RecoveryReport>> {
        let mut reports = Vec::new();
        for id in self.attempts.list()? {
            let snapshot = self.attempts.load(&id)?;
            let node = snapshot.record.frozen.node.clone();
            let report = match snapshot.phase() {
                AttemptPhase::Sealed => {
                    let action = self.recover_cleanup(snapshot.workspace.as_ref())?;
                    RecoveryReport {
                        attempt: id,
                        node,
                        action,
                        sealed: snapshot.seal.map(|s| s.state),
                    }
                }
                AttemptPhase::Executed => {
                    // Evidence exists; finish the decide → submit → seal tail.
                    let evidence = snapshot.evidence.expect("phase Executed has evidence");
                    let workspace = snapshot
                        .workspace
                        .clone()
                        .context("executed attempt has no recorded workspace")?;
                    match self.settle(
                        &id,
                        &snapshot.record.frozen,
                        &workspace,
                        evidence.exit_code,
                    ) {
                        Ok((sealed, _)) => {
                            let cleanup = self.recover_cleanup(Some(&workspace))?;
                            RecoveryReport {
                                attempt: id,
                                node,
                                action: format!("settled from recorded evidence; {cleanup}"),
                                sealed: Some(sealed),
                            }
                        }
                        Err(e) => RecoveryReport {
                            attempt: id,
                            node,
                            action: format!("unrecoverable without intervention: {e:#}"),
                            sealed: None,
                        },
                    }
                }
                // No exit evidence: whether the command ran is unknowable
                // from here, so nothing may be submitted. Seal as
                // interrupted; the workspace is cleaned only if untouched.
                AttemptPhase::Created
                | AttemptPhase::WorkspacePlanned
                | AttemptPhase::Prepared
                | AttemptPhase::Requested => {
                    let sealed = self.attempts.seal(
                        &id,
                        SealedState::Interrupted {
                            reason: "recovered: attempt ended without exit evidence".into(),
                        },
                    )?;
                    let cleanup = self.recover_cleanup(snapshot.workspace.as_ref())?;
                    RecoveryReport {
                        attempt: id,
                        node,
                        action: format!("sealed as interrupted; {cleanup}"),
                        sealed: Some(sealed.state),
                    }
                }
            };
            reports.push(report);
        }
        Ok(reports)
    }

    fn recover_cleanup(&self, workspace: Option<&PreparedWorkspace>) -> Result<String> {
        Ok(match workspace {
            None => "no workspace to clean".into(),
            Some(ws) => match self.workspaces.cleanup(ws)? {
                CleanupOutcome::Removed => "workspace removed".into(),
                CleanupOutcome::RetainedDirty => {
                    format!("workspace retained (uncommitted changes): {}", ws.path.display())
                }
                CleanupOutcome::AlreadyAbsent => "workspace already absent".into(),
            },
        })
    }
}

/// The prompt handed to the agent: the frozen definition, its completed
/// dependencies' results as context, and the outcome contract.
fn build_prompt(frozen: &FrozenInput, policy: &ExecutionPolicy) -> String {
    use std::fmt::Write;
    let mut prompt = String::new();
    let _ = writeln!(prompt, "# Task ({})\n\n{}", frozen.node, frozen.description.trim());
    if !frozen.dependencies.is_empty() {
        let _ = writeln!(prompt, "\n# Completed dependencies");
        for dep in &frozen.dependencies {
            let _ = writeln!(prompt, "\n## {} ({})", dep.title, dep.id);
            if !dep.result_notes.trim().is_empty() {
                let _ = writeln!(prompt, "\n{}", dep.result_notes.trim());
            }
            if let Some(output) = &dep.output {
                let _ = writeln!(prompt, "\n(output: {} {})", output.scheme, output.id);
            }
        }
    }
    let workspace = policy.workspace_destination.display();
    let outcome_path = policy.io_destination.join(outcome::OUTCOME_FILE);
    let _ = write!(
        prompt,
        "\n# Contract\n\n\
         Work only inside `{workspace}` (also `$ORKA_WORKSPACE`).\n\
         When finished, declare your outcome by writing `{}` (also\n\
         `$ORKA_OUTCOME`) as TOML:\n\n\
         ```toml\n\
         outcome = \"succeeded\"   # or \"failed\"\n\
         outputs = [\"path/relative/to/workspace\"]\n\
         message = \"one-line summary of the change\"\n\
         notes = \"what was done and why\"\n\
         ```\n\n\
         Declare every file you created or changed in `outputs`. Undeclared\n\
         changes are not captured and will block completion.\n",
        outcome_path.display()
    );
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeExecutor, FakeWorkGraph, FakeWorkspaces};
    use crate::ports::{DefinitionFingerprint, WorkItem};
    use std::path::{Path, PathBuf};

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> (TempDir, PathBuf) {
        let dir = std::env::temp_dir().join(format!("orka-engine-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        (TempDir(dir.clone()), dir)
    }

    fn frozen(node: &str) -> FrozenInput {
        FrozenInput {
            node: NodeId(node.into()),
            definition: DefinitionFingerprint {
                metadata: "m".into(),
                description: "d".into(),
            },
            description: "Do the thing".into(),
            dependencies: vec![],
            input_commit: "c0ffee".into(),
        }
    }

    fn graph_with(node: &str) -> FakeWorkGraph {
        let mut graph = FakeWorkGraph::default();
        graph.items.push(WorkItem {
            id: NodeId(node.into()),
            title: "Do the thing".into(),
        });
        graph.frozen.insert(node.into(), frozen(node));
        graph
    }

    type RunHook = Box<dyn Fn(&ExecutionSpec) -> Result<()>>;

    /// An executor hook that writes the given outcome declaration into the
    /// exchange mount, as a conforming agent would.
    fn declare(outcome_toml: &'static str) -> RunHook {
        Box::new(move |spec| {
            let io = spec
                .mounts
                .iter()
                .find(|m| m.destination == Path::new("/orka"))
                .expect("io mount");
            std::fs::write(io.source.join("outcome.toml"), outcome_toml)?;
            Ok(())
        })
    }

    struct Harness {
        _temp: TempDir,
        graph: FakeWorkGraph,
        executor: FakeExecutor,
        workspaces: FakeWorkspaces,
        attempts: FsAttemptStore,
    }

    impl Harness {
        fn new(graph: FakeWorkGraph, executor: FakeExecutor) -> Self {
            let (_temp, dir) = scratch();
            Self {
                _temp,
                graph,
                executor,
                workspaces: FakeWorkspaces::new(dir.join("worktrees")),
                attempts: FsAttemptStore::new(dir.join(".orka")),
            }
        }

        fn engine(&self) -> Engine<'_> {
            Engine {
                graph: &self.graph,
                executor: &self.executor,
                workspaces: &self.workspaces,
                attempts: &self.attempts,
                policy: ExecutionPolicy::new(vec!["agent".into()]),
            }
        }
    }

    #[test]
    fn a_successful_attempt_submits_seals_and_cleans_up() {
        let executor = FakeExecutor {
            transcript: "worked\n".into(),
            on_run: Some(declare(
                "outcome = \"succeeded\"\noutputs = [\"out.txt\"]\nnotes = \"did it\"\n",
            )),
            ..Default::default()
        };
        let mut graph = graph_with("node-1");
        graph.output_commit = Some("beef".into());
        let harness = Harness::new(graph, executor);

        let report = harness.engine().run_next().unwrap().expect("ready work");
        assert_eq!(report.node.0, "node-1");
        assert_eq!(
            report.sealed,
            SealedState::Submitted {
                output_commit: Some("beef".into())
            }
        );
        assert!(!report.backend_failed);
        assert_eq!(report.cleanup, CleanupOutcome::Removed);

        // The submission carried the declared outcome.
        let submissions = harness.graph.submissions.borrow();
        assert_eq!(submissions.len(), 1);
        assert!(matches!(
            &submissions[0].1,
            WorkOutcome::Succeeded { outputs, .. } if outputs == &vec!["out.txt".to_string()]
        ));

        // Everything about the attempt is durably recorded.
        let snapshot = harness.attempts.load(&report.attempt).unwrap();
        assert_eq!(snapshot.phase(), AttemptPhase::Sealed);
        assert_eq!(snapshot.record.frozen, frozen("node-1"));
        let request = snapshot.request.unwrap();
        assert_eq!(request.command, vec!["agent"]);
        assert!(!request.network, "network stays denied by default");
        assert_eq!(
            request.environment.get("ORKA_OUTCOME").map(String::as_str),
            Some("/orka/outcome.toml")
        );
        assert_eq!(
            std::fs::read_to_string(harness.attempts.transcript_path(&report.attempt)).unwrap(),
            "worked\n"
        );
        // The prompt was staged for the agent.
        let prompt = std::fs::read_to_string(
            harness.attempts.io_dir(&report.attempt).unwrap().join(PROMPT_FILE),
        )
        .unwrap();
        assert!(prompt.contains("Do the thing"));
        assert!(prompt.contains("outcome = \"succeeded\""));
    }

    #[test]
    fn a_declared_failure_records_evidence() {
        let executor = FakeExecutor {
            exit_code: 2,
            on_run: Some(declare("outcome = \"failed\"\nnotes = \"cannot\"\n")),
            ..Default::default()
        };
        let harness = Harness::new(graph_with("node-1"), executor);
        let report = harness.engine().run_node(&NodeId("node-1".into())).unwrap();
        assert_eq!(report.sealed, SealedState::FailureRecorded);
        assert!(report.backend_failed, "nonzero exit rides along");
        assert!(matches!(
            harness.graph.submissions.borrow()[0].1,
            WorkOutcome::Failed { .. }
        ));
    }

    #[test]
    fn no_declaration_is_a_contract_violation_or_interruption_and_submits_nothing() {
        for (exit_code, expect_violation) in [(0, true), (137, false)] {
            let executor = FakeExecutor {
                exit_code,
                ..Default::default()
            };
            let harness = Harness::new(graph_with("node-1"), executor);
            let report = harness.engine().run_node(&NodeId("node-1".into())).unwrap();
            match report.sealed {
                SealedState::ContractViolation { .. } => assert!(expect_violation),
                SealedState::Interrupted { .. } => assert!(!expect_violation),
                other => panic!("unexpected seal {other:?}"),
            }
            assert!(harness.graph.submissions.borrow().is_empty());
        }
    }

    #[test]
    fn a_moved_graph_seals_stale_and_completes_nothing() {
        let executor = FakeExecutor {
            on_run: Some(declare("outcome = \"succeeded\"\nnotes = \"n\"\n")),
            ..Default::default()
        };
        let mut graph = graph_with("node-1");
        graph.stale.push("node-1".into());
        let harness = Harness::new(graph, executor);
        let report = harness.engine().run_node(&NodeId("node-1".into())).unwrap();
        assert!(matches!(report.sealed, SealedState::StaleAtSubmit { .. }));
        assert!(harness.graph.submissions.borrow().is_empty());
    }

    #[test]
    fn recovery_settles_executed_attempts_from_their_recorded_evidence() {
        // Build an attempt that crashed after evidence capture: everything
        // recorded, nothing sealed, outcome file present in the io dir.
        let harness = Harness::new(graph_with("node-1"), FakeExecutor::default());
        let engine = harness.engine();
        let id = AttemptId::new();
        harness.attempts.create(&id, &frozen("node-1")).unwrap();
        let ws = harness.workspaces.prepare(&id.0, "c0ffee").unwrap();
        harness.attempts.plan_workspace(&id, &ws).unwrap();
        harness.attempts.mark_prepared(&id).unwrap();
        harness
            .attempts
            .record_request(&id, &engine.execution_spec(&NodeId("node-1".into()), &ws, &harness.attempts.io_dir(&id).unwrap()))
            .unwrap();
        harness
            .attempts
            .record_evidence(
                &id,
                &crate::ports::ExecutionReport {
                    backend: "fake".into(),
                    backend_reference: None,
                    exit_code: 0,
                    started_at_ms: 1,
                    finished_at_ms: 2,
                },
            )
            .unwrap();
        std::fs::write(
            harness.attempts.io_dir(&id).unwrap().join("outcome.toml"),
            "outcome = \"succeeded\"\nnotes = \"done before the crash\"\n",
        )
        .unwrap();

        let reports = engine.recover().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].sealed,
            Some(SealedState::Submitted {
                output_commit: None
            })
        );
        assert_eq!(harness.graph.submissions.borrow().len(), 1);
        assert_eq!(
            harness.attempts.load(&id).unwrap().phase(),
            AttemptPhase::Sealed
        );

        // Recovery is idempotent: a second pass re-reports, resubmits nothing.
        let again = harness.engine().recover().unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(harness.graph.submissions.borrow().len(), 1);
    }

    #[test]
    fn recovery_seals_attempts_without_evidence_as_interrupted() {
        let harness = Harness::new(graph_with("node-1"), FakeExecutor::default());
        let id = AttemptId::new();
        harness.attempts.create(&id, &frozen("node-1")).unwrap();
        let ws = harness.workspaces.prepare(&id.0, "c0ffee").unwrap();
        harness.attempts.plan_workspace(&id, &ws).unwrap();
        harness.attempts.mark_prepared(&id).unwrap();

        let reports = harness.engine().recover().unwrap();
        assert!(matches!(
            reports[0].sealed,
            Some(SealedState::Interrupted { .. })
        ));
        assert!(harness.graph.submissions.borrow().is_empty());
        assert!(!ws.path.exists(), "untouched workspace was cleaned");
    }

    #[test]
    fn recovery_retains_dirty_workspaces_and_says_so() {
        let harness = Harness::new(graph_with("node-1"), FakeExecutor::default());
        let id = AttemptId::new();
        harness.attempts.create(&id, &frozen("node-1")).unwrap();
        let ws = harness.workspaces.prepare(&id.0, "c0ffee").unwrap();
        harness.attempts.plan_workspace(&id, &ws).unwrap();
        harness.attempts.mark_prepared(&id).unwrap();

        // Make cleanup consider this workspace dirty.
        let mut workspaces = FakeWorkspaces::new(harness.workspaces.root.clone());
        workspaces.dirty.push(id.0.clone());
        let engine = Engine {
            workspaces: &workspaces,
            ..harness.engine()
        };
        let reports = engine.recover().unwrap();
        assert!(reports[0].action.contains("retained"));
        assert!(ws.path.exists());
    }
}
