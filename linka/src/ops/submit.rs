//! The snapshot/submission protocol external callers work against.
//!
//! [`snapshot_work`] freezes the inputs of ready work; the `submit_*` and
//! `capture_*` entry points revalidate every frozen field before recording
//! anything, reporting graph conflicts as [`SubmissionConflict`] values.

use super::mutate::{prepare_node_attachments, write_node_attachments};
use super::*;

/// Freeze the exact graph, context, and project inputs for ready work.
pub fn snapshot_work(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    context: &[String],
) -> Result<WorkSnapshot> {
    let state = node_state(store, vcs, id)?;
    if !state.is_ready() {
        bail!("node `{id}` is not ready");
    }
    let (meta, _) = store.read_node(id)?;
    let dependencies = pin_node_list(store, &meta.depends_on)?;
    let lineage = pin_node_list(store, &meta.derived_from)?;
    let project = current_project_snapshot(store, vcs)?;
    let context = pin_context(
        store,
        vcs,
        (!project.revision.is_empty()).then_some(project.revision.as_str()),
        context,
    )?;
    Ok(WorkSnapshot {
        schema: SNAPSHOT_SCHEMA,
        node: id.clone(),
        definition: store.node_version(id)?,
        dependencies,
        lineage,
        context,
        project,
        previous_result: store.current_result_version(id)?,
    })
}

#[derive(Debug)]
pub enum SubmissionError {
    Conflict(Vec<SubmissionConflict>),
    Evaluation(anyhow::Error),
}

impl std::fmt::Display for SubmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(conflicts) => write!(f, "result submission conflicts: {conflicts:?}"),
            Self::Evaluation(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for SubmissionError {}
impl From<anyhow::Error> for SubmissionError {
    fn from(error: anyhow::Error) -> Self {
        Self::Evaluation(error)
    }
}

pub fn submit_result(
    store: &Store,
    vcs: &dyn Vcs,
    submission: ResultSubmission,
) -> std::result::Result<(), SubmissionError> {
    submit_result_with_attachments(store, vcs, submission, Vec::new())
}

/// Submit a result and immutable node attachments in the same Linka store
/// commit. The attachment batch is validated before any result is written;
/// snapshot conflicts record neither the result nor the attachments.
pub fn submit_result_with_attachments(
    store: &Store,
    vcs: &dyn Vcs,
    submission: ResultSubmission,
    attachments: Vec<NewNodeAttachment>,
) -> std::result::Result<(), SubmissionError> {
    let mutation = store.mutation_lock(vcs)?;
    submit_result_locked(
        store,
        vcs,
        RecordedSubmission {
            snapshot: submission.snapshot,
            outcome: submission.outcome.into(),
            output: submission.output,
            notes: submission.notes,
            author: submission.author,
            producer: submission.producer,
        },
        attachments,
        mutation,
    )
}

/// Submit the accepted, rejected, or abandoned conclusion for a verification node.
pub fn submit_verification(
    store: &Store,
    vcs: &dyn Vcs,
    submission: VerificationSubmission,
) -> std::result::Result<(), SubmissionError> {
    let mutation = store.mutation_lock(vcs)?;
    submit_result_locked(
        store,
        vcs,
        RecordedSubmission {
            snapshot: submission.snapshot,
            outcome: submission.outcome.into(),
            output: None,
            notes: submission.notes,
            author: submission.author,
            producer: submission.producer,
        },
        Vec::new(),
        mutation,
    )
}

pub(super) struct RecordedSubmission {
    pub(super) snapshot: WorkSnapshot,
    pub(super) outcome: ResultOutcome,
    pub(super) output: Option<ArtifactRef>,
    pub(super) notes: String,
    pub(super) author: Author,
    pub(super) producer: Option<ProducerEvidence>,
}

pub(super) fn submit_result_locked(
    store: &Store,
    vcs: &dyn Vcs,
    submission: RecordedSubmission,
    attachments: Vec<NewNodeAttachment>,
    mutation: MutationLock,
) -> std::result::Result<(), SubmissionError> {
    let snapshot = &submission.snapshot;
    let id = &snapshot.node;
    if snapshot.schema != SNAPSHOT_SCHEMA {
        return Err(SubmissionError::Evaluation(anyhow::anyhow!(
            "work snapshot uses unsupported schema {}",
            snapshot.schema
        )));
    }
    let mut conflicts = Vec::new();
    if let Some(output) = &submission.output {
        if output.repository != snapshot.project.repository {
            return Err(SubmissionError::Evaluation(anyhow::anyhow!(
                "output artifact belongs to a different project repository"
            )));
        }
    }
    let (meta, _) = store.read_node(id)?;
    if !outcome_kind_matches(meta.verifies.is_some(), submission.outcome) {
        let message = if meta.verifies.is_some() {
            verification_requires_review_result(id)
        } else {
            ordinary_requires_work_result(id)
        };
        return Err(SubmissionError::Evaluation(anyhow::anyhow!(message)));
    }
    if store.node_version(id)? != snapshot.definition {
        conflicts.push(SubmissionConflict::DefinitionChanged);
    }
    if pin_node_list(store, &meta.depends_on)? != snapshot.dependencies {
        conflicts.push(SubmissionConflict::DependenciesChanged);
    }
    if pin_node_list(store, &meta.derived_from)? != snapshot.lineage {
        conflicts.push(SubmissionConflict::LineageChanged);
    }
    let current_revision = vcs.head_commit()?.unwrap_or_default();
    for pin in &snapshot.context {
        let current = if current_revision.is_empty() {
            project_file_blob(&store.project_root(), &pin.path)?
        } else {
            vcs.file_blob_at(&current_revision, pin.path.as_str())?
        };
        if current.as_deref() != Some(&pin.identity) {
            conflicts.push(SubmissionConflict::ContextChanged {
                path: pin.path.clone(),
            });
        }
    }
    let expected_revision = submission
        .output
        .as_ref()
        .map(|output| output.id.as_str())
        .unwrap_or(snapshot.project.revision.as_str());
    if !snapshot.project.revision.is_empty()
        && current_revision != expected_revision
        && current_revision != snapshot.project.revision
    {
        conflicts.push(SubmissionConflict::ProjectChanged);
    }
    if !node_state(store, vcs, id)?.is_ready() {
        conflicts.push(SubmissionConflict::ReadinessChanged);
    }
    if outcome_requires_full_pins(submission.outcome) {
        for dependency in &snapshot.dependencies {
            let state = node_state(store, vcs, &dependency.id)?;
            if !state.is_complete()
                || !dependency.outcome.is_some_and(result_satisfies_dependency)
                || dependency.result.is_none()
            {
                if !conflicts.contains(&SubmissionConflict::ReadinessChanged) {
                    conflicts.push(SubmissionConflict::ReadinessChanged);
                }
                break;
            }
        }
    }
    let previous = store.current_result_version(id)?;
    if previous != snapshot.previous_result {
        conflicts.push(SubmissionConflict::PreviousResultChanged);
    }
    if !conflicts.is_empty() {
        return Err(SubmissionError::Conflict(conflicts));
    }
    let mut consumed = snapshot.dependencies.clone();
    consumed.extend(snapshot.lineage.clone());
    let result = ResultMeta {
        schema: RESULT_SCHEMA,
        at: now_millis(),
        author: submission.author,
        definition: snapshot.definition.clone(),
        outcome: submission.outcome,
        project: snapshot.project.clone(),
        consumed,
        context: snapshot.context.clone(),
        output: submission.output,
        producer: submission.producer,
    };
    let candidate_decision = match result.outcome {
        ResultOutcome::Verification(
            VerificationOutcome::Accepted | VerificationOutcome::Rejected,
        ) => {
            let candidate = meta
                .verifies
                .as_ref()
                .expect("verification outcome was validated against node kind");
            Some(CandidateStore::new(store).prepare_verification_decision(
                vcs,
                candidate,
                &snapshot.node,
                &result,
                submission.author,
                submission.notes.clone(),
            )?)
        }
        ResultOutcome::Verification(VerificationOutcome::Abandoned) | ResultOutcome::Work(_) => {
            None
        }
    };
    let (_, pending_attachments) = prepare_node_attachments(store, id, attachments)?;
    write_node_attachments(store, id, &pending_attachments)?;
    store.write_result(id, &result, &submission.notes)?;
    if let Some(candidate) = &candidate_decision {
        CandidateStore::new(store).write_prepared_decision(candidate)?;
    }
    mutation.commit(vcs, &format!("linka: result {id}"))?;
    Ok(())
}

/// Capture work performed in an execution context against a caller's frozen
/// [`WorkSnapshot`], and submit a version-checked result. This is the entry
/// point for an orchestrator that snapshots ready work with [`snapshot_work`],
/// performs it in a separate worktree, and later submits success or failure
/// against that exact snapshot — Linka still owns output capture, artifact
/// identity, path validation, and every version check.
///
/// `vcs` is the execution context the work happened in (for git, a linked
/// worktree via [`crate::GitVcs::for_execution`]); graph state still commits to
/// the store's workbench repository. Returns the captured output commit, or
/// `None` for graph-only success and for failure.
///
/// Ordering guarantee: a graph conflict records no result. For a successful
/// submission with outputs the output commit is captured *before* the version
/// check, but its output ref is retained only after the submission is accepted.
/// A conflict therefore retains no Linka output ref and mutates no graph
/// state. The caller still owns the execution branch: an orchestrator such as
/// Orka deliberately retains that branch as attempt evidence, so its captured
/// commit remains reachable until an explicit caller-side pruning policy
/// removes it.
#[allow(clippy::too_many_arguments)]
pub fn capture_submission(
    store: &Store,
    vcs: &dyn Vcs,
    snapshot: WorkSnapshot,
    outputs: &[ProjectPath],
    message: Option<String>,
    outcome: Outcome,
    notes: String,
    author: Author,
    producer: Option<ProducerEvidence>,
) -> std::result::Result<Option<String>, SubmissionError> {
    let id = snapshot.node.clone();
    let (meta, _) = store.read_node(&id)?;
    require_ordinary_node(&meta, &id)?;
    let output_paths: Vec<String> = outputs.iter().map(ToString::to_string).collect();
    if outcome == Outcome::Done {
        // The only uncommitted project changes allowed are the declared
        // outputs — output provenance is asserted exactly here. This check is
        // required even for graph-only success: an empty declaration means
        // the execution tree must have no changes, not that changes may be
        // silently omitted.
        require_clean_except(vcs, &output_paths)?;
    }
    let output_commit = if outcome == Outcome::Done && !outputs.is_empty() {
        let message = match message {
            Some(message) => message,
            None => {
                let (_, description) = store.read_node(&id)?;
                crate::model::title_of(&description).to_string()
            }
        };
        let input =
            (!snapshot.project.revision.is_empty()).then_some(snapshot.project.revision.as_str());
        let commit_message = output_commit_message(&id, message, input);
        Some(vcs.capture(&output_paths, &commit_message)?)
    } else {
        None
    };

    let output = output_commit
        .as_deref()
        .map(|commit| git_artifact(store, commit))
        .transpose()?;
    submit_result(
        store,
        vcs,
        ResultSubmission {
            snapshot,
            outcome,
            output,
            notes,
            author,
            producer,
        },
    )?;
    // Accepted: keep the output reachable independently of the worktree.
    if let Some(commit) = &output_commit {
        vcs.retain_output(&id, commit)?;
    }
    Ok(output_commit)
}

/// Submit an already captured execution and its producer evidence attachments
/// in one Linka store commit.
#[allow(clippy::too_many_arguments)]
pub fn submit_captured_execution_with_attachments(
    store: &Store,
    vcs: &dyn Vcs,
    snapshot: WorkSnapshot,
    output_commit: Option<&str>,
    notes: String,
    author: Author,
    producer: Option<ProducerEvidence>,
    attachments: Vec<NewNodeAttachment>,
) -> std::result::Result<(), SubmissionError> {
    let id = snapshot.node.clone();
    let (meta, _) = store.read_node(&id)?;
    require_ordinary_node(&meta, &id)?;
    if let Some(commit) = output_commit {
        if !vcs.commit_exists(commit)? {
            return Err(SubmissionError::Evaluation(anyhow::anyhow!(
                "promoted execution output `{commit}` is missing from the project repository"
            )));
        }
    }
    let output = output_commit
        .map(|commit| git_artifact(store, commit))
        .transpose()?;
    submit_result_with_attachments(
        store,
        vcs,
        ResultSubmission {
            snapshot,
            outcome: Outcome::Done,
            output,
            notes,
            author,
            producer,
        },
        attachments,
    )?;
    if let Some(commit) = output_commit {
        vcs.retain_output(&id, commit)?;
    }
    Ok(())
}
