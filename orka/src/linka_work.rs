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

use crate::access::{read_access_summary_bytes, AccessSummary};
use crate::attempt::{AttemptId, AttemptRecord};
use crate::executor::ExecutionReport;
use crate::input::{AttemptInput, DependencyContext};
use anyhow::{bail, Context, Result};
use linka::ops::{self, SubmissionError};
use linka::{
    ArtifactStore, Author, BranchStore, CandidateId, CandidateStore, ConsumedNode,
    ExternalIdentity, GitVcs, NewCandidate, NewNodeAttachment, NodeId, Outcome,
    ProducerEvidence, ProjectPath, ResultVersion, Store, SubmissionConflict,
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
    pub version: ResultVersion,
}

pub const OUTPUT_EVIDENCE_PARTS: [&str; 8] = [
    "attempt",
    "prompt",
    "request",
    "agent-output",
    "diagnostics",
    "evidence",
    "outcome",
    "accesses",
];

pub struct AttemptEvidencePart {
    pub name: &'static str,
    pub media_type: &'static str,
    pub data: Vec<u8>,
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

    /// Project-side operations run in the attempt's audited worktree; graph
    /// state still commits to the workbench repository.
    fn vcs_at(&self, workspace: &Path) -> GitVcs {
        GitVcs::for_execution(self.store, workspace.to_path_buf())
    }

    /// Convert complete attempt evidence into opaque Linka attachments. The
    /// submission methods pass this batch into Linka's result transaction so
    /// the result and evidence become durable in one store commit.
    fn evidence_attachments(
        attempt: &AttemptId,
        parts: Vec<AttemptEvidencePart>,
    ) -> Vec<NewNodeAttachment> {
        parts
            .into_iter()
            .map(|part| NewNodeAttachment {
                namespace: "orka".into(),
                key: format!("{attempt}/{}", part.name),
                media_type: Some(part.media_type.into()),
                data: part.data,
            })
            .collect()
    }

    /// Verify that every Orka-produced project candidate retains both its Git
    /// artifact and the complete durable evidence set.
    pub fn audit_output_evidence(&self) -> Result<Vec<String>> {
        let candidates = CandidateStore::new(self.store);
        let vcs = self.vcs();
        let mut problems = Vec::new();
        for id in candidates.list()? {
            let candidate = candidates.load(&id)?;
            let Some(external) = candidate
                .external
                .as_ref()
                .filter(|external| external.namespace == "orka")
            else {
                continue;
            };
            if candidate.artifact.scheme != "git-commit"
                || !vcs.commit_exists(&candidate.artifact.id)?
            {
                problems.push(format!(
                    "{}: project artifact {} is not retained",
                    candidate.id, candidate.artifact.id
                ));
            }
            for part in OUTPUT_EVIDENCE_PARTS {
                let key = format!("{}/{}", external.id, part);
                let attachment =
                    self.store
                        .read_node_attachment(candidate.node.as_str(), "orka", &key)?;
                if attachment.is_none() {
                    problems.push(format!(
                        "{}: missing node attachment orka/{key}",
                        candidate.id
                    ));
                }
            }
            let result = self.store.read_result(candidate.node.as_str())?;
            let tracking = result
                .as_ref()
                .and_then(|(result, _)| result.producer.as_ref())
                .filter(|producer| producer.namespace == "orka")
                .and_then(|producer| producer.data.get("context_tracking"));
            let producer_complete = tracking
                .and_then(|tracking| tracking.get("complete"))
                .and_then(serde_json::Value::as_bool);
            match producer_complete {
                Some(false) => problems.push(format!(
                    "{}: producer evidence records incomplete access tracking",
                    candidate.id
                )),
                None => problems.push(format!(
                    "{}: producer evidence is missing access-tracking completeness",
                    candidate.id
                )),
                Some(true) => {}
            }

            let accesses_key = format!("{}/accesses", external.id);
            if let Some((_, data)) =
                self.store
                    .read_node_attachment(candidate.node.as_str(), "orka", &accesses_key)?
            {
                match read_access_summary_bytes(&data) {
                    Ok(summary) => {
                        if !summary.complete {
                            let reason = summary
                                .reason
                                .as_deref()
                                .unwrap_or("no reason was recorded");
                            problems.push(format!(
                                "{}: attached access journal is incomplete: {reason}",
                                candidate.id
                            ));
                        }
                        if producer_complete.is_some_and(|complete| complete != summary.complete) {
                            problems.push(format!(
                                "{}: producer evidence and attached access journal disagree on completeness",
                                candidate.id
                            ));
                        }
                        if let Some(tracking) = tracking {
                            let expected_method =
                                tracking.get("method").and_then(serde_json::Value::as_str);
                            let expected_files = tracking
                                .get("observed_files")
                                .and_then(serde_json::Value::as_u64);
                            if expected_method != Some(summary.method.as_str())
                                || expected_files != Some(summary.distinct_paths().len() as u64)
                            {
                                problems.push(format!(
                                    "{}: producer evidence and attached access journal disagree on observed access data",
                                    candidate.id
                                ));
                            }
                        }
                    }
                    Err(error) => problems.push(format!(
                        "{}: invalid attached access journal: {error:#}",
                        candidate.id
                    )),
                }
            }
            let key = format!("{}/attempt", external.id);
            if let Some((_, data)) =
                self.store
                    .read_node_attachment(candidate.node.as_str(), "orka", &key)?
            {
                let text = std::str::from_utf8(&data).with_context(|| {
                    format!("{}: attempt attachment is not UTF-8", candidate.id)
                })?;
                let record: AttemptRecord = toml::from_str(text)
                    .with_context(|| format!("{}: invalid attempt attachment", candidate.id))?;
                if record.id.0 != external.id || record.input.node() != &candidate.node {
                    problems.push(format!(
                        "{}: attempt attachment identity does not match the candidate",
                        candidate.id
                    ));
                }
            }
        }
        Ok(problems)
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
        let linka::ResultOutcome::Work(outcome) = result.outcome else {
            bail!(
                "Orka attempt `{attempt_id}` produced a verification result for ordinary node `{node}`"
            );
        };
        Ok(Some(RecordedResult {
            outcome,
            output_commit: result.output.map(|artifact| artifact.id),
            version: self.store.result_version(node.as_str())?,
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
    /// agent's complete work from the private execution repository and record
    /// the result.
    /// The produced file set is the diff between the frozen input commit and
    /// the committed file tree — discovered here, never declared by the agent.
    /// The engine has already verified the repository is clean (the agent is
    /// required to commit all its work), so this folds the agent's own commits
    /// into one output on the input. A graph conflict records nothing and is
    /// returned as [`Settled::Conflict`].
    #[allow(clippy::too_many_arguments)]
    pub fn submit_candidate_success(
        &self,
        input: &AttemptInput,
        workspace: &crate::workspace::ValidatedWorkspace,
        workspaces: &dyn crate::workspace::WorkspaceManager,
        attempt: &AttemptId,
        message: Option<String>,
        notes: String,
        producer: ProducerEvidence,
        evidence: Vec<AttemptEvidencePart>,
    ) -> Result<(Settled, Option<CandidateId>)> {
        let private_vcs = self.vcs_at(&workspace.workspace.path);
        let title = message.unwrap_or_else(|| linka::title_of(&input.description).to_string());
        let mut commit_message = format!("{title}\n\nLinka-Node: {}", input.node());
        if !input.input_commit().is_empty() {
            commit_message.push_str(&format!("\nLinka-Input: {}", input.input_commit()));
        }
        let output_commit = private_vcs.capture_worktree(input.input_commit(), &commit_message)?;

        if let Some(output) = &output_commit {
            // Capture advances the private branch. Re-attest that exact state
            // before importing any object or ref into the project repository.
            let captured = workspaces.validate(&workspace.workspace)?;
            if captured.head != *output {
                bail!(
                    "captured output {output} does not match revalidated private HEAD {}",
                    captured.head
                );
            }
            workspaces.promote(&captured, output)?;
        }

        let project_vcs = self.vcs();
        let settled = classify(
            ops::submit_captured_execution_with_attachments(
                self.store,
                &project_vcs,
                input.snapshot.clone(),
                output_commit.as_deref(),
                notes,
                Author::Machine,
                Some(producer.clone()),
                Self::evidence_attachments(attempt, evidence),
            )
            .map(|_| output_commit.clone()),
        )?;
        match settled {
            Settled::Accepted {
                output_commit: Some(output_commit),
                ..
            } => {
                let candidate =
                    self.register_candidate(input, &workspace.workspace, attempt, &output_commit)?;
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
        let candidate = CandidateStore::new(self.store).register(
            &self.vcs(),
            NewCandidate {
                node: input.node().clone(),
                branch: workspace.branch.clone(),
                target: input.target_branch.clone(),
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

    /// Record a failed orchestrated attempt and its complete evidence in the
    /// same Linka result transaction.
    pub fn submit_failure_with_evidence(
        &self,
        input: &AttemptInput,
        workspace: &Path,
        attempt: &AttemptId,
        notes: String,
        producer: ProducerEvidence,
        evidence: Vec<AttemptEvidencePart>,
    ) -> Result<Settled> {
        let vcs = self.vcs_at(workspace);
        classify(
            ops::submit_result_with_attachments(
                self.store,
                &vcs,
                linka::ResultSubmission {
                    snapshot: input.snapshot.clone(),
                    outcome: Outcome::Failed,
                    output: None,
                    notes,
                    author: Author::Machine,
                    producer: Some(producer),
                },
                Self::evidence_attachments(attempt, evidence),
            )
            .map(|()| None),
        )
    }

    /// Attach observed project reads to one exact accepted result. The Linka
    /// operation is immutable and idempotent, so recovery may repeat it.
    pub fn record_observed_context(
        &self,
        input: &AttemptInput,
        workspace: &Path,
        expected_result: &ResultVersion,
        paths: &[String],
    ) -> Result<usize> {
        ops::record_context_observation(
            self.store,
            &self.vcs_at(workspace),
            input.node().as_str(),
            expected_result,
            paths,
        )
        .with_context(|| format!("recording observed context for `{}`", input.node()))
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

/// Namespaced producer evidence identifying the Orka attempt that produced a
/// result. Only the harness-observed execution facts are recorded; the
/// transcript and mutable filesystem paths stay in `.orka/`. Linka preserves
/// this verbatim and never interprets it.
pub fn producer_evidence(attempt: &AttemptId, report: &ExecutionReport) -> ProducerEvidence {
    producer_evidence_with_accesses(attempt, report, None)
}

pub fn producer_evidence_with_accesses(
    attempt: &AttemptId,
    report: &ExecutionReport,
    accesses: Option<&AccessSummary>,
) -> ProducerEvidence {
    let mut data = serde_json::Map::new();
    data.insert("attempt".into(), attempt.0.clone().into());
    data.insert("backend".into(), report.backend.clone().into());
    data.insert("started_at_ms".into(), report.started_at_ms.into());
    data.insert("finished_at_ms".into(), report.finished_at_ms.into());
    data.insert("exit_code".into(), report.exit_code.into());
    if let Some(accesses) = accesses {
        let mut tracking = serde_json::Map::new();
        tracking.insert("method".into(), accesses.method.clone().into());
        tracking.insert("complete".into(), accesses.complete.into());
        tracking.insert(
            "observed_files".into(),
            accesses.distinct_paths().len().into(),
        );
        if let Some(reason) = &accesses.reason {
            tracking.insert("reason".into(), reason.clone().into());
        }
        data.insert(
            "context_tracking".into(),
            serde_json::Value::Object(tracking),
        );
    }
    ProducerEvidence {
        namespace: "orka".into(),
        data: serde_json::Value::Object(data),
    }
}
