//! The attempt lifecycle: select → snapshot → record → prepare → execute →
//! decide → version-checked submit → seal → clean up.
//!
//! Every step that has an external side effect is durably recorded before it
//! happens, so [`Engine::recover`] can classify any crash by the files the
//! attempt store holds and finish the idempotent remainder. Recovery never
//! invents results: an attempt without exit evidence seals as interrupted,
//! and a dirty or missing workspace is reported, not discarded or guessed at.
//!
//! Orka orchestrates a Linka store specifically. Selection, snapshotting, and
//! submission go through [`LinkaWork`]; graph semantics stay Linka's, and Orka
//! never models a graph of its own.

use crate::attempt::{AttemptId, AttemptPhase, FsAttemptStore, SealedState};
use crate::executor::{ExecutionReport, ExecutionSpec, IsolatedExecutor, MountSpec};
use crate::input::AttemptInput;
use crate::linka_work::{self, LinkaWork, Settled};
use crate::outcome::{self, AgentOutcome, Decision, PROMPT_FILE};
use crate::workspace::{CleanupOutcome, PreparedWorkspace, WorkspaceManager};
use anyhow::{bail, Context, Result};
use linka::NodeId;
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
    pub linka: LinkaWork<'a>,
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
    pub candidate: Option<linka::CandidateId>,
    pub sealed: SealedState,
    pub exit_code: i32,
    /// The command exited nonzero even though an outcome was declared and
    /// handled — reportable backend trouble, not attempt state.
    pub backend_failed: bool,
    pub cleanup: CleanupOutcome,
}

/// A visible milestone in a live attempt. The CLI reports these on stderr so
/// long-running workspace preparation and agent execution do not look hung,
/// while stdout remains reserved for the final, script-friendly report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunProgress {
    Selected {
        node: NodeId,
    },
    AttemptCreated {
        attempt: AttemptId,
    },
    WorkspacePrepared {
        attempt: AttemptId,
    },
    ExecutionStarted {
        attempt: AttemptId,
        transcript: PathBuf,
    },
    ExecutionFinished {
        attempt: AttemptId,
        exit_code: i32,
    },
    Sealed {
        attempt: AttemptId,
        state: SealedState,
    },
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
        self.run_next_with_progress(&mut |_| {})
    }

    /// Run the first ready node and report durable lifecycle milestones.
    pub fn run_next_with_progress(
        &self,
        progress: &mut dyn FnMut(&RunProgress),
    ) -> Result<Option<RunReport>> {
        match self.linka.ready_for_machine()?.into_iter().next() {
            Some(item) => self.run_node_with_progress(&item.node, progress).map(Some),
            None => Ok(None),
        }
    }

    /// Run one node through a complete attempt.
    pub fn run_node(&self, id: &NodeId) -> Result<RunReport> {
        self.run_node_with_progress(id, &mut |_| {})
    }

    /// Run one node through a complete attempt and report lifecycle
    /// milestones as they happen.
    pub fn run_node_with_progress(
        &self,
        id: &NodeId,
        progress: &mut dyn FnMut(&RunProgress),
    ) -> Result<RunReport> {
        if self.policy.command.is_empty() {
            bail!("no agent command configured");
        }
        progress(&RunProgress::Selected { node: id.clone() });
        // 1–2. Snapshot the Linka work input and durably record the attempt.
        let input = self.linka.prepare_input(id)?;
        let attempt = AttemptId::new();
        self.attempts.create(&attempt, &input)?;
        progress(&RunProgress::AttemptCreated {
            attempt: attempt.clone(),
        });

        // 3. Record the workspace plan, then create it at the frozen revision.
        let plan = self.workspaces.plan(&attempt.0, input.input_commit());
        self.attempts.plan_workspace(&attempt, &plan)?;
        let workspace = self
            .workspaces
            .prepare(&attempt.0, input.input_commit())
            .with_context(|| format!("preparing workspace for {attempt}"))?;
        self.attempts.mark_prepared(&attempt)?;
        progress(&RunProgress::WorkspacePrepared {
            attempt: attempt.clone(),
        });

        // 4. Stage the exchange directory and record the exact request.
        let io_dir = self.attempts.io_dir(&attempt)?;
        std::fs::write(io_dir.join(PROMPT_FILE), build_prompt(&input, &self.policy))
            .context("writing the attempt prompt")?;
        let spec = self.execution_spec(id, &workspace, &io_dir);
        self.attempts.record_request(&attempt, &spec)?;

        // 5. Execute and capture evidence.
        let transcript = self.attempts.transcript_path(&attempt);
        progress(&RunProgress::ExecutionStarted {
            attempt: attempt.clone(),
            transcript: transcript.clone(),
        });
        let report = self.executor.run(&spec, &transcript)?;
        self.attempts.record_evidence(&attempt, &report)?;
        progress(&RunProgress::ExecutionFinished {
            attempt: attempt.clone(),
            exit_code: report.exit_code,
        });

        // 6–7. Decide from the evidence, submit version-checked, seal.
        let (sealed, backend_failed, candidate) =
            self.settle(&attempt, &input, &workspace, &report)?;
        progress(&RunProgress::Sealed {
            attempt: attempt.clone(),
            state: sealed.clone(),
        });
        let cleanup = self.workspaces.cleanup(&workspace)?;
        Ok(RunReport {
            attempt,
            node: id.clone(),
            candidate,
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
        environment.insert("ORKA_NODE".into(), node.to_string());
        environment.insert(
            "ORKA_WORKSPACE".into(),
            self.policy
                .workspace_destination
                .to_string_lossy()
                .into_owned(),
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
    /// apply the failure matrix, submit anything submittable to Linka, and
    /// seal. Idempotent — recovery re-runs it for executed-but-unsealed
    /// attempts, and Linka's version check makes re-submission safe.
    fn settle(
        &self,
        attempt: &AttemptId,
        input: &AttemptInput,
        workspace: &PreparedWorkspace,
        report: &ExecutionReport,
    ) -> Result<(SealedState, bool, Option<linka::CandidateId>)> {
        let declared = outcome::read_declared(&self.attempts.io_dir(attempt)?)?;
        match outcome::decide(declared, report.exit_code) {
            Decision::Submit {
                outcome,
                backend_failed,
            } => {
                // Idempotency across the accept-before-seal crash window: if
                // Linka already holds a result this attempt produced, seal from
                // it rather than resubmitting into a now-complete (and so
                // stale-looking) node.
                if let Some(recorded) = self.linka.result_by_attempt(input.node(), &attempt.0)? {
                    let candidate = match (&recorded.outcome, &recorded.output_commit) {
                        (linka::Outcome::Done, Some(output)) => Some(
                            self.linka
                                .register_candidate(input, workspace, attempt, output)?,
                        ),
                        _ => None,
                    };
                    let sealed = match recorded.outcome {
                        linka::Outcome::Done => SealedState::Submitted {
                            output_commit: recorded.output_commit,
                        },
                        linka::Outcome::Failed => SealedState::FailureRecorded,
                    };
                    self.attempts.seal(attempt, sealed.clone())?;
                    return Ok((sealed, backend_failed, candidate));
                }
                let producer = linka_work::producer_evidence(attempt, report);
                let (settled, succeeded, candidate) = match outcome {
                    AgentOutcome::Succeeded {
                        outputs,
                        message,
                        notes,
                    } => {
                        if !workspace.path.exists() {
                            // The declared outputs lived in the workspace;
                            // without it there is nothing to capture or submit.
                            bail!(
                                "attempt {attempt} declared success but its workspace {} is missing",
                                workspace.path.display()
                            );
                        }
                        let outputs = match linka_work::project_paths(&outputs) {
                            Ok(outputs) => outputs,
                            Err(error) => {
                                // Invalid declared paths never reach git: this
                                // is a contract violation, not stale work.
                                let sealed = SealedState::ContractViolation {
                                    reason: format!("{error:#}"),
                                };
                                self.attempts.seal(attempt, sealed.clone())?;
                                return Ok((sealed, backend_failed, None));
                            }
                        };
                        let (settled, candidate) = self.linka.submit_candidate_success(
                            input, workspace, attempt, &outputs, message, notes, producer,
                        )?;
                        (settled, true, candidate)
                    }
                    AgentOutcome::Failed { notes } => {
                        let settled =
                            self.linka
                                .submit_failure(input, &workspace.path, notes, producer)?;
                        (settled, false, None)
                    }
                };
                let sealed = match settled {
                    Settled::Accepted { output_commit } if succeeded => {
                        SealedState::Submitted { output_commit }
                    }
                    Settled::Accepted { .. } => SealedState::FailureRecorded,
                    Settled::Conflict(conflicts) => SealedState::StaleAtSubmit { conflicts },
                };
                self.attempts.seal(attempt, sealed.clone())?;
                Ok((sealed, backend_failed, candidate))
            }
            Decision::ContractViolation { reason } => {
                let sealed = SealedState::ContractViolation { reason };
                self.attempts.seal(attempt, sealed.clone())?;
                Ok((sealed, false, None))
            }
            Decision::Interrupted { reason } => {
                let sealed = SealedState::Interrupted { reason };
                self.attempts.seal(attempt, sealed.clone())?;
                Ok((sealed, false, None))
            }
        }
    }

    /// Classify every recorded attempt and finish what can be finished.
    pub fn recover(&self) -> Result<Vec<RecoveryReport>> {
        let mut reports = Vec::new();
        for id in self.attempts.list()? {
            let snapshot = self.attempts.load(&id)?;
            let node = snapshot.record.input.node().clone();
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
                    match self.settle(&id, &snapshot.record.input, &workspace, &evidence) {
                        Ok((sealed, _, _)) => {
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
                    format!(
                        "workspace retained (uncommitted changes): {}",
                        ws.path.display()
                    )
                }
                CleanupOutcome::AlreadyAbsent => "workspace already absent".into(),
            },
        })
    }
}

/// The prompt handed to the agent: the frozen definition, its completed
/// dependencies' and lineage's results as context, and the outcome contract.
/// The prose here is frozen audit material — Linka's snapshot alone is
/// authoritative for submission.
fn build_prompt(input: &AttemptInput, policy: &ExecutionPolicy) -> String {
    use crate::input::DependencyContext;
    use std::fmt::Write;
    let mut prompt = String::new();
    let _ = writeln!(
        prompt,
        "# Task ({})\n\n{}",
        input.node(),
        input.description.trim()
    );
    let mut section = |title: &str, items: &[DependencyContext]| {
        if items.is_empty() {
            return;
        }
        let _ = writeln!(prompt, "\n# {title}");
        for item in items {
            let _ = writeln!(prompt, "\n## {} ({})", item.title, item.node);
            if !item.result_notes.trim().is_empty() {
                let _ = writeln!(prompt, "\n{}", item.result_notes.trim());
            }
        }
    };
    section("Completed dependencies", &input.dependency_context);
    section("Derived from", &input.lineage_context);

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
    use crate::input::{sample_input, DependencyContext};

    #[test]
    fn the_prompt_carries_the_task_related_work_and_the_contract() {
        let mut input = sample_input("node-1");
        input.description = "Implement the parser".into();
        input.dependency_context = vec![DependencyContext {
            node: "node-dep".parse().unwrap(),
            title: "Define the grammar".into(),
            result_notes: "grammar lives in grammar.md".into(),
        }];
        input.lineage_context = vec![DependencyContext {
            node: "node-src".parse().unwrap(),
            title: "Original spec".into(),
            result_notes: String::new(),
        }];

        let prompt = build_prompt(&input, &ExecutionPolicy::new(vec!["agent".into()]));
        assert!(prompt.contains("# Task (node-1)"));
        assert!(prompt.contains("Implement the parser"));
        assert!(prompt.contains("Completed dependencies"));
        assert!(prompt.contains("Define the grammar (node-dep)"));
        assert!(prompt.contains("grammar lives in grammar.md"));
        assert!(prompt.contains("Derived from"));
        assert!(prompt.contains("Original spec (node-src)"));
        assert!(prompt.contains("outcome = \"succeeded\""));
    }
}
