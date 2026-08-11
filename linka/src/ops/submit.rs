//! The snapshot/submission protocol, and the two operations composed from it.
//!
//! [`snapshot`] freezes the exact graph, context, and project inputs of a unit
//! of work. [`submit`] revalidates every frozen field under the mutation lock
//! and records nothing on conflict. [`complete`] is the one composed
//! convenience: it performs the snapshot/capture/submission sequence without
//! handing control back to a caller between its steps.

use super::mutate::{prepare_attachments, write_attachments};
use super::*;
use crate::graph::Graph;
use crate::model::{
    title_of, Candidate, Conclusion, Submission, SubmissionConflict, WorkSnapshot, RESULT_SCHEMA,
    SNAPSHOT_SCHEMA,
};
use crate::model::{Author, ResultMeta};
use crate::store::MutationLock;

/// Freeze the exact graph, context, and project inputs of one unit of work.
///
/// This is a pure freeze and does not require the node to be ready: readiness
/// is enforced in exactly one place, [`submit`], and only for conclusions that
/// assert success — which is what makes recording a failure always possible
/// without a special-case path around the protocol.
pub fn snapshot(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    context: &[String],
) -> Result<WorkSnapshot> {
    let (meta, _) = store.read_node(id)?;
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
        dependencies: pin_node_list(store, &meta.depends_on)?,
        lineage: pin_node_list(store, &meta.derived_from)?,
        context,
        project,
        previous_result: store.current_result_version(id)?,
    })
}

#[derive(Debug)]
pub enum SubmissionError {
    /// The graph moved under the snapshot. Nothing was recorded.
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

/// Record a result. This is the only way a result is ever written.
pub fn submit(
    store: &Store,
    vcs: &dyn Vcs,
    submission: Submission,
) -> std::result::Result<(), SubmissionError> {
    let mutation = store.mutation_lock(vcs)?;
    submit_locked(store, vcs, submission, mutation)
}

fn submit_locked(
    store: &Store,
    vcs: &dyn Vcs,
    submission: Submission,
    mutation: MutationLock,
) -> std::result::Result<(), SubmissionError> {
    let Submission {
        snapshot,
        conclusion,
        notes,
        author,
        producer,
        attachments,
    } = submission;
    let id = &snapshot.node;
    if snapshot.schema != SNAPSHOT_SCHEMA {
        return Err(evaluation(format!(
            "work snapshot uses unsupported schema {}",
            snapshot.schema
        )));
    }
    let (meta, _) = store.read_node(id)?;
    let outcome = conclusion.outcome();
    if !outcome.suits(meta.is_verification()) {
        return Err(evaluation(if meta.is_verification() {
            format!("review node `{id}` concludes accepted, rejected, or abandoned")
        } else {
            format!("ordinary node `{id}` concludes done or failed")
        }));
    }
    if let Some(output) = conclusion.output() {
        if output.repository != snapshot.project.repository {
            return Err(evaluation(
                "output artifact belongs to a different project repository".to_string(),
            ));
        }
    }

    let mut conflicts = Vec::new();
    if store.node_version(id)? != snapshot.definition {
        conflicts.push(SubmissionConflict::DefinitionChanged);
    }
    if pin_node_list(store, &meta.depends_on)? != snapshot.dependencies {
        conflicts.push(SubmissionConflict::DependenciesChanged);
    }
    if pin_node_list(store, &meta.derived_from)? != snapshot.lineage {
        conflicts.push(SubmissionConflict::LineageChanged);
    }
    let revision = vcs.head_commit()?.unwrap_or_default();
    let root = store.project_root();
    for pin in &snapshot.context {
        let current = context_blob(
            vcs,
            &root,
            (!revision.is_empty()).then_some(revision.as_str()),
            &pin.path,
        )?;
        if current.as_deref() != Some(&pin.identity) {
            conflicts.push(SubmissionConflict::ContextChanged {
                path: pin.path.clone(),
            });
        }
    }
    // The project may legitimately have moved onto the output this submission
    // is reporting, but nowhere else.
    let expected = conclusion
        .output()
        .map(|output| output.id.as_str())
        .unwrap_or(snapshot.project.revision.as_str());
    if !snapshot.project.revision.is_empty()
        && revision != expected
        && revision != snapshot.project.revision
    {
        conflicts.push(SubmissionConflict::ProjectChanged);
    }
    if store.current_result_version(id)? != snapshot.previous_result {
        conflicts.push(SubmissionConflict::PreviousResultChanged);
    }
    // Only a claim of success asserts that the work's foundations held.
    if conclusion.asserts_success() {
        let graph = Graph::load(store, vcs)?;
        let unready = !graph.state(id).is_ready();
        let unpinned = snapshot.dependencies.iter().any(|dependency| {
            !graph.state(&dependency.id).is_complete()
                || dependency.result.is_none()
                || !dependency
                    .outcome
                    .is_some_and(Outcome::satisfies_dependency)
        });
        if unready || unpinned {
            conflicts.push(SubmissionConflict::ReadinessChanged);
        }
    }
    if !conflicts.is_empty() {
        return Err(SubmissionError::Conflict(conflicts));
    }

    let mut consumed = snapshot.dependencies;
    consumed.extend(snapshot.lineage);
    let result = ResultMeta {
        schema: RESULT_SCHEMA,
        at: now_millis(),
        author,
        definition: snapshot.definition,
        outcome,
        project: snapshot.project,
        consumed,
        context: snapshot.context,
        output: conclusion.output().cloned(),
        producer,
    };
    // Validate the whole attachment batch before writing any of it, so a
    // rejected batch leaves neither attachments nor a result behind.
    let (_, pending) = prepare_attachments(store, id, attachments)?;
    write_attachments(store, id, &pending)?;
    store.write_result(id, &result, &notes)?;
    mutation.commit(vcs, &format!("linka: result {id}"))?;
    Ok(())
}

fn evaluation(message: String) -> SubmissionError {
    SubmissionError::Evaluation(anyhow::anyhow!(message))
}

/// Complete a node's work in one short-lived transaction: commit the declared
/// outputs in the project repository, then record the result in the store.
///
/// The mutation lock is held from the clean-store precondition through the
/// result commit, and the output retention ref is written only *after* the
/// submission is accepted — a rejected submission leaves no dangling Linka ref.
/// Returns the output commit, or `None` when the work produced no files.
#[allow(clippy::too_many_arguments)] // mirrors the CLI surface one-to-one
pub fn complete(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    outputs: &[String],
    context: &[String],
    message: Option<String>,
    notes: &str,
    author: Author,
) -> Result<Option<String>> {
    let outputs: Vec<String> = outputs
        .iter()
        .map(|path| {
            path.parse::<ProjectPath>()
                .map(|path| path.to_string())
                .map_err(anyhow::Error::msg)
        })
        .collect::<Result<_>>()?;
    // Establish a clean, stable store before inspecting or changing the
    // project, so an interrupted earlier completion cannot be silently built
    // upon and a dirty store cannot leave a new project commit behind before
    // being rejected.
    let mutation = store.mutation_lock(vcs)?;
    require_consistent_project_head(store, vcs)?;
    let (meta, description) = store.read_node(id)?;
    if meta.is_verification() {
        bail!("review node `{id}` concludes accepted, rejected, or abandoned");
    }
    // The only uncommitted project changes allowed are the outputs about to be
    // committed: completion is where output provenance is asserted.
    require_clean_except(vcs, &outputs)?;

    let input = vcs.head_commit()?;
    let snapshot = snapshot(store, vcs, id, context)?;

    let output_commit = if outputs.is_empty() {
        None
    } else {
        let message = message.unwrap_or_else(|| title_of(&description).to_string());
        let message = output_commit_message(id, message, input.as_deref());
        Some(vcs.capture(&outputs, &message)?)
    };
    let output = output_commit
        .as_deref()
        .map(|commit| git_artifact(store, commit))
        .transpose()?;

    let submitted = submit_locked(
        store,
        vcs,
        Submission {
            snapshot,
            conclusion: Conclusion::Done { output },
            notes: notes.into(),
            author,
            producer: None,
            attachments: Vec::new(),
        },
        mutation,
    );
    match (submitted, &output_commit) {
        (Ok(()), Some(commit)) => {
            vcs.retain_output(id, commit)?;
            Ok(output_commit)
        }
        (Ok(()), None) => Ok(None),
        (Err(error), Some(commit)) => bail!(
            "inconsistent completion: project output commit {commit} was created but its \
             Linka result was not recorded: {error}"
        ),
        (Err(error), None) => Err(anyhow::anyhow!(error)),
    }
}

/// Refuse a project checkout whose `HEAD` identifies itself as a Linka output
/// but is not recorded as that node's output in the store. This detects the
/// durable partial state a completion interrupted between the output commit
/// and the result commit would leave behind.
pub fn require_consistent_project_head(store: &Store, vcs: &dyn Vcs) -> Result<()> {
    let Some(head) = vcs.head_commit()? else {
        return Ok(());
    };
    let Some(declared) = vcs.linka_node(&head)? else {
        return Ok(());
    };
    match origin_of(store, &head)? {
        Some(recorded) if recorded.as_str() == declared => return Ok(()),
        Some(recorded) => bail!(
            "inconsistent Linka state: project HEAD {} declares node `{declared}`, but the \
             store records it as output of `{recorded}`",
            short(&head)
        ),
        None => {}
    }
    declared.parse::<NodeId>().map_err(|error| {
        anyhow::anyhow!(
            "project HEAD {} has an invalid Linka-Node trailer: {error}",
            short(&head)
        )
    })?;
    if vcs.output_was_recorded(&store.store_name(), &declared, &head)? {
        return Ok(());
    }
    bail!(
        "inconsistent Linka state: project HEAD {} declares itself as output of node \
         `{declared}`, but the store has never recorded that output; restore the project \
         changes and run `linka complete` again, or move the project checkout to a \
         consistent commit",
        short(&head)
    )
}

/// The node whose recorded result produced `commit`, read straight from the
/// store: this runs before a graph evaluation would be meaningful.
fn origin_of(store: &Store, commit: &str) -> Result<Option<NodeId>> {
    for id in store.node_ids()? {
        if let Some((result, _)) = store.read_result(&id)? {
            if result.output.as_ref().map(|artifact| artifact.id.as_str()) == Some(commit) {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}

/// Fast-forward a candidate's target branch onto its artifact.
///
/// Publication writes nothing to the store: whether it succeeded is always
/// re-derivable from git ancestry, so retrying after a crash is safe and no
/// journal is needed. A target that moved forward independently fails with a
/// plain "cannot fast-forward" error.
pub fn publish(vcs: &dyn Vcs, candidate: &Candidate) -> Result<()> {
    let target_ref = candidate.target_ref();
    let target = vcs
        .ref_commit(&target_ref)?
        .with_context(|| format!("target branch `{}` does not exist", candidate.target))?;
    if vcs.is_ancestor(&candidate.artifact.id, &target)? {
        return Ok(()); // already published
    }
    if !vcs.is_ancestor(&target, &candidate.artifact.id)? {
        bail!(
            "candidate `{}` cannot fast-forward `{}`: {} is not contained in {}",
            candidate.id,
            candidate.target,
            short(&target),
            short(&candidate.artifact.id)
        );
    }
    if !vcs.publish_fast_forward(&target_ref, &target, &candidate.artifact.id)? {
        bail!(
            "candidate `{}` cannot fast-forward `{}`: the branch moved while publishing",
            candidate.id,
            candidate.target
        );
    }
    Ok(())
}
