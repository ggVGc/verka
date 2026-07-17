//! Orka's concrete integration with a Linka store.
//!
//! This is not a backend-neutral port: Orka orchestrates Linka specifically,
//! and uses Linka's public operations and value types directly. The module
//! exists only to keep Linka calls — selection, snapshotting, and
//! version-checked submission — out of the attempt-lifecycle code, and to
//! translate between Orka's execution types and Linka's graph protocol in one
//! trusted place.
//!
//! All access goes through Linka's public API; Orka never reads or writes
//! Linka's on-disk representation.

use crate::attempt::AttemptId;
use crate::executor::ExecutionReport;
use crate::input::{AttemptInput, DependencyContext};
use anyhow::{Context, Result};
use linka::ops::{self, SubmissionError};
use linka::{
    Author, BranchStore, CandidateId, CandidateStore, ConsumedNode, ExternalIdentity, GitVcs,
    NewCandidate, NodeId, Outcome, ProducerEvidence, ProjectPath, Store, SubmissionConflict,
};
use std::path::{Path, PathBuf};

/// A ready node as the orchestrator lists it: Linka's node id plus its title,
/// so the CLI can show something readable beside the id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadyWork {
    pub node: NodeId,
    pub title: String,
}

/// The result of submitting an attempt to Linka. A conflict is an answer, not
/// an operational error: the graph moved between snapshot and submit, and
/// nothing was recorded. Evaluation/storage/git failures surface as `Err`.
#[derive(Clone, Debug)]
pub enum Settled {
    Accepted { output_commit: Option<String> },
    Conflict(Vec<SubmissionConflict>),
}

/// A result Linka already recorded, attributed to a specific Orka attempt by
/// its producer evidence. Lets recovery recognize its own accepted result in
/// the crash window between Linka accepting and Orka sealing, instead of
/// re-submitting (which the now-complete node would reject as stale).
#[derive(Clone, Debug)]
pub struct RecordedResult {
    pub outcome: Outcome,
    pub output_commit: Option<String>,
}

pub struct LinkaWork<'a> {
    store: &'a Store,
}

impl<'a> LinkaWork<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// The project directory results and outputs resolve against.
    pub fn project_root(&self) -> PathBuf {
        self.store.project_root()
    }

    fn vcs(&self) -> GitVcs {
        GitVcs::for_store(self.store)
    }

    /// Project-side operations run in the attempt's execution worktree; graph
    /// state still commits to the workbench repository.
    fn vcs_at(&self, workspace: &Path) -> GitVcs {
        GitVcs::for_execution(self.store, workspace.to_path_buf())
    }

    /// Linka-ready, machine-assignable work, in Linka's selection order. Orka
    /// chooses among Linka-ready results; it does not derive readiness.
    pub fn ready_for_machine(&self) -> Result<Vec<ReadyWork>> {
        let vcs = self.vcs();
        let mut out = Vec::new();
        for id in ops::ready_nodes(self.store, &vcs, Some(Author::Machine))? {
            let (_, description) = self.store.read_node(&id)?;
            out.push(ReadyWork {
                node: id.parse().map_err(anyhow::Error::msg)?,
                title: linka::title_of(&description).to_string(),
            });
        }
        Ok(out)
    }

    /// Ask Linka to validate and snapshot `node`, and gather the prose Orka
    /// hands the agent, as one durable [`AttemptInput`]. Fails if the node is
    /// not ready — snapshotting is Linka's readiness gate.
    pub fn prepare_input(&self, node: &NodeId) -> Result<AttemptInput> {
        let vcs = self.vcs();
        let snapshot = ops::snapshot_work(self.store, &vcs, node.as_str(), &[])
            .with_context(|| format!("snapshotting `{node}`"))?;
        let target_branch = vcs
            .current_branch()?
            .context("project HEAD is detached; check out a target branch before running Orka")?;
        let (_, description) = self.store.read_node(node.as_str())?;
        let dependency_context = self.context_for(&snapshot.dependencies)?;
        let lineage_context = self.context_for(&snapshot.lineage)?;
        Ok(AttemptInput {
            snapshot,
            target_branch,
            description,
            dependency_context,
            lineage_context,
        })
    }

    /// The result Linka currently records for `node`, if it was produced by
    /// the given Orka attempt (matched by namespaced producer evidence). Used
    /// by recovery to settle idempotently across the accept-before-seal crash
    /// window without a spurious stale conflict.
    pub fn result_by_attempt(
        &self,
        node: &NodeId,
        attempt_id: &str,
    ) -> Result<Option<RecordedResult>> {
        let Some((result, _)) = self.store.read_result(node.as_str())? else {
            return Ok(None);
        };
        let Some(producer) = &result.producer else {
            return Ok(None);
        };
        if producer.namespace != "orka"
            || producer.data.get("attempt").and_then(|v| v.as_str()) != Some(attempt_id)
        {
            return Ok(None);
        }
        Ok(Some(RecordedResult {
            outcome: result.outcome,
            output_commit: result.output.map(|artifact| artifact.id),
        }))
    }

    /// Read the prompt prose for a set of pinned related nodes.
    fn context_for(&self, pins: &[ConsumedNode]) -> Result<Vec<DependencyContext>> {
        pins.iter()
            .map(|pin| {
                let (_, description) = self.store.read_node(pin.id.as_str())?;
                let result_notes = self
                    .store
                    .read_result(pin.id.as_str())?
                    .map(|(_, notes)| notes)
                    .unwrap_or_default();
                Ok(DependencyContext {
                    node: pin.id.clone(),
                    title: linka::title_of(&description).to_string(),
                    result_notes,
                })
            })
            .collect()
    }

    /// Submit a successful attempt against its persisted snapshot: capture the
    /// declared outputs in the execution worktree and record the result. A
    /// graph conflict records nothing and is returned as [`Settled::Conflict`].
    pub fn submit_candidate_success(
        &self,
        input: &AttemptInput,
        workspace: &crate::workspace::PreparedWorkspace,
        attempt: &AttemptId,
        outputs: &[ProjectPath],
        message: Option<String>,
        notes: String,
        producer: ProducerEvidence,
    ) -> Result<(Settled, Option<CandidateId>)> {
        let vcs = self.vcs_at(&workspace.path);
        let settled = classify(ops::capture_submission(
            self.store,
            &vcs,
            input.snapshot.clone(),
            outputs,
            message,
            Outcome::Done,
            notes,
            Author::Machine,
            Some(producer.clone()),
        ))?;
        match settled {
            Settled::Accepted {
                output_commit: Some(output_commit),
                ..
            } => {
                let candidate =
                    self.register_candidate(input, workspace, attempt, &output_commit)?;
                Ok((
                    Settled::Accepted {
                        output_commit: Some(output_commit),
                    },
                    Some(candidate),
                ))
            }
            accepted => Ok((accepted, None)),
        }
    }

    /// Submit success without registering a candidate. This remains useful for
    /// graph-only work and non-orchestrator callers; Orka's engine uses
    /// [`submit_candidate_success`] for project-producing attempts.
    pub fn submit_success(
        &self,
        input: &AttemptInput,
        workspace: &Path,
        outputs: &[ProjectPath],
        message: Option<String>,
        notes: String,
        producer: ProducerEvidence,
    ) -> Result<Settled> {
        let vcs = self.vcs_at(workspace);
        classify(ops::capture_submission(
            self.store,
            &vcs,
            input.snapshot.clone(),
            outputs,
            message,
            Outcome::Done,
            notes,
            Author::Machine,
            Some(producer),
        ))
    }

    /// Idempotently attach an accepted project output to Linka's candidate
    /// protocol. The Orka attempt is an opaque external identity; Linka never
    /// interprets it.
    pub fn register_candidate(
        &self,
        input: &AttemptInput,
        workspace: &crate::workspace::PreparedWorkspace,
        attempt: &AttemptId,
        output_commit: &str,
    ) -> Result<CandidateId> {
        let target = if input.target_branch.is_empty() {
            self.vcs()
                .current_branch()?
                .context("cannot recover candidate without a checked-out target branch")?
        } else {
            input.target_branch.clone()
        };
        let candidate = CandidateStore::new(self.store).register(
            &self.vcs(),
            NewCandidate {
                node: input.node().clone(),
                branch: workspace.branch.clone(),
                input_commit: input.input_commit().to_string(),
                target,
                external: Some(ExternalIdentity {
                    namespace: "orka".into(),
                    id: attempt.0.clone(),
                }),
            },
        )?;
        if candidate.artifact.id != output_commit {
            anyhow::bail!(
                "Linka candidate {} records {}, expected {}",
                candidate.id,
                candidate.artifact.id,
                output_commit
            );
        }
        Ok(candidate.id)
    }

    /// Record a failed attempt against its persisted snapshot. Faithful failure
    /// evidence pins exactly what the attempt ran against, so it is submitted
    /// against the frozen snapshot rather than re-observing current inputs.
    pub fn submit_failure(
        &self,
        input: &AttemptInput,
        workspace: &Path,
        notes: String,
        producer: ProducerEvidence,
    ) -> Result<Settled> {
        let vcs = self.vcs_at(workspace);
        classify(ops::capture_submission(
            self.store,
            &vcs,
            input.snapshot.clone(),
            &[],
            None,
            Outcome::Failed,
            notes,
            Author::Machine,
            Some(producer),
        ))
    }
}

/// Map a Linka submission result onto Orka's terminal states: a conflict is a
/// stale-at-submit answer; an evaluation/storage/git failure is operational.
fn classify(result: std::result::Result<Option<String>, SubmissionError>) -> Result<Settled> {
    match result {
        Ok(output_commit) => Ok(Settled::Accepted { output_commit }),
        Err(SubmissionError::Conflict(conflicts)) => Ok(Settled::Conflict(conflicts)),
        Err(SubmissionError::Evaluation(error)) => Err(error),
    }
}

/// Convert declared output strings into validated project paths. Invalid paths
/// are a contract/policy failure surfaced here, never passed raw into git.
pub fn project_paths(outputs: &[String]) -> Result<Vec<ProjectPath>> {
    outputs
        .iter()
        .map(|path| {
            path.parse::<ProjectPath>()
                .map_err(|e| anyhow::anyhow!("invalid declared output `{path}`: {e}"))
        })
        .collect()
}

/// Namespaced producer evidence identifying the Orka attempt that produced a
/// result. Only the harness-observed execution facts are recorded; the
/// transcript and mutable filesystem paths stay in `.orka/`. Linka preserves
/// this verbatim and never interprets it.
pub fn producer_evidence(attempt: &AttemptId, report: &ExecutionReport) -> ProducerEvidence {
    // Built as a table with only present fields: Linka persists this to TOML,
    // which has no null, so an absent backend reference is omitted, not null.
    let mut data = serde_json::Map::new();
    data.insert("attempt".into(), attempt.0.clone().into());
    data.insert("backend".into(), report.backend.clone().into());
    if let Some(reference) = &report.backend_reference {
        data.insert("backend_reference".into(), reference.clone().into());
    }
    data.insert("started_at_ms".into(), report.started_at_ms.into());
    data.insert("finished_at_ms".into(), report.finished_at_ms.into());
    data.insert("exit_code".into(), report.exit_code.into());
    ProducerEvidence {
        namespace: "orka".into(),
        data: serde_json::Value::Object(data),
    }
}
