//! Execution- and review-layer operations, extracted from llaundry's ops
//! module when the projects were separated.
//!
//! PARKED: this code is a reference for the future harness/orchestrator. It is
//! not expected to compile yet — it still refers to items that used to live in
//! the composed llaundry crate (`Store`, `Vcs`, core model types, and the
//! former `llaundry_review`/`llaundry_work` crate paths). The plan is to
//! re-home it behind a task-store trait with an adapter for llaundry.

use llaundry_review::CandidateStore as _;
use llaundry_review::FsCandidateStore;
use llaundry_work::AttemptStore as _;
use llaundry_work::FsAttemptStore;

fn attempts(store: &Store) -> FsAttemptStore {
    FsAttemptStore::new(store.root())
}

fn reviews(store: &Store) -> FsCandidateStore {
    FsCandidateStore::new(store.root())
}
#[allow(clippy::too_many_arguments)]
pub fn complete_with_execution(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    outputs: &[String],
    context: &[String],
    message: Option<String>,
    notes: &str,
    author: Author,
    execution: Option<ExecutionIdentity>,
) -> Result<Option<String>> {
    if author == Author::Machine && execution.is_none() {
        bail!("machine-authored work requires a durable execution attempt");
    }
    if let Some(execution) = &execution {
        validate_execution(store, vcs, id, author, execution)?;
        if attempts(store).read_result(&execution.attempt_id)?.is_some() {
            bail!("attempt `{}` already has a result", execution.attempt_id);
        }
    }
    // The only uncommitted project changes allowed are the outputs we are about
    // to commit — completion is where output provenance is asserted.
    require_clean_except(vcs, outputs)?;
    let (meta, description) = store.read_node(id)?;

    let input_commit = vcs.head_commit()?;

    // Pin everything the work saw, before committing anything.
    let context = pin_context(store, vcs, context)?;
    let consumed = pin_deps(store, &meta)?;

    let output_commit = if outputs.is_empty() {
        None
    } else {
        let message = message.unwrap_or_else(|| crate::model::title_of(&description).to_string());
        let mut commit_message = format!("{message}\n\nLlaundry-Node: {id}");
        if let Some(input) = &input_commit {
            commit_message.push_str(&format!("\nLlaundry-Input: {input}"));
        }
        let commit = vcs.capture(outputs, &commit_message)?;
        vcs.retain_output(id, &commit)?;
        Some(commit)
    };

    let result = ResultMeta {
        at: now_millis(),
        author,
        definition: store.node_version(id)?,
        outcome: Outcome::Done,
        consumed,
        context,
        output: output_commit.as_deref().map(git_artifact),
        producer: execution.as_ref().map(|execution| {
            WorkEvidence {
                attempt: Some(execution.attempt_id.clone()),
                backend: None,
                model: None,
            }
            .to_producer()
        }),
    };
    if let Some(execution) = &execution {
        attempts(store).write_result(&execution.attempt_id, &result, notes)?;
    }
    store.write_result(id, &result, notes)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: complete {id}"))?;
    Ok(output_commit)
}
pub fn fail_with_execution(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    notes: &str,
    author: Author,
    execution: Option<ExecutionIdentity>,
) -> Result<()> {
    if author == Author::Machine && execution.is_none() {
        bail!("machine-authored work requires a durable execution attempt");
    }
    if let Some(execution) = &execution {
        validate_execution(store, vcs, id, author, execution)?;
        if attempts(store).read_result(&execution.attempt_id)?.is_some() {
            bail!("attempt `{}` already has a result", execution.attempt_id);
        }
    }
    let (meta, _) = store.read_node(id)?;
    let consumed = pin_deps(store, &meta)?;
    let result = ResultMeta {
        at: now_millis(),
        author,
        definition: store.node_version(id)?,
        outcome: Outcome::Failed,
        consumed,
        context: Vec::new(),
        output: None,
        producer: execution.as_ref().map(|execution| {
            WorkEvidence {
                attempt: Some(execution.attempt_id.clone()),
                backend: None,
                model: None,
            }
            .to_producer()
        }),
    };
    if let Some(execution) = &execution {
        attempts(store).write_result(&execution.attempt_id, &result, notes)?;
    }
    store.write_result(id, &result, notes)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: fail {id}"))?;
    Ok(())
}

fn validate_execution(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    author: Author,
    execution: &ExecutionIdentity,
) -> Result<()> {
    if author != Author::Machine {
        bail!(
            "an isolated machine execution cannot record work as `{}`",
            author.as_str()
        );
    }
    if execution.node_id != id {
        bail!(
            "execution for node `{}` cannot record work on `{id}`",
            execution.node_id
        );
    }
    let attempt = attempts(store).read(&execution.attempt_id)?;
    if !attempt.prepared {
        bail!(
            "attempt `{}` has no prepared workspace",
            execution.attempt_id
        );
    }
    if attempt.work_item != id
        || attempt.branch != execution.candidate_branch
        || attempt.force != execution.force
    {
        bail!(
            "execution identity does not match durable attempt `{}`",
            execution.attempt_id
        );
    }
    if store.node_version(id)? != attempt.definition {
        bail!(
            "node definition changed during attempt `{}`",
            execution.attempt_id
        );
    }
    authorize_execution_start(store, vcs, id, Author::Machine, execution.force)?;
    let expected = format!("llaundry/candidates/{}", execution.attempt_id);
    if execution.candidate_branch != expected {
        bail!(
            "candidate branch `{}` does not match attempt `{}` (expected `{expected}`)",
            execution.candidate_branch,
            execution.attempt_id
        );
    }
    if vcs.current_branch()?.as_deref() != Some(&execution.candidate_branch) {
        bail!(
            "execution worktree is not on candidate branch `{}`",
            execution.candidate_branch
        );
    }
    if vcs.head_commit()?.as_deref() != Some(&attempt.input.id) {
        bail!("attempt worktree HEAD moved before result capture");
    }
    Ok(())
}
/// Enforce the graph policy for starting or finalizing work through an
/// execution driver. `force` is explicit authorization to bypass readiness or
/// assignment restrictions; frontends do not reimplement these rules.
pub fn authorize_execution_start(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    worker: Author,
    force: bool,
) -> Result<()> {
    let (meta, _) = store.read_node(id)?;
    if !force {
        if let Some(assignee) = meta.assignee {
            if assignee != worker {
                bail!(
                    "node `{id}` is assigned to {}; {} may not work it",
                    assignee.as_str(),
                    worker.as_str()
                );
            }
        }
        if !is_dispatchable(store, vcs, id) {
            let blockers = blockers(store, vcs, id);
            if blockers.is_empty() {
                bail!("node `{id}` is not ready to work");
            }
            bail!(
                "node `{id}` is blocked; resolve its dependencies:\n  {}",
                blockers.join("\n  ")
            );
        }
    }
    Ok(())
}

pub struct ExecutionWorkspace {
    pub identity: ExecutionIdentity,
    pub path: std::path::PathBuf,
    pub input_commit: String,
    pub input_tree: String,
    pub rejected_feedback: Option<String>,
}

pub fn prepare_execution(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    worker: Author,
    force: bool,
    explicit_base: Option<&str>,
    materialize: bool,
) -> Result<ExecutionWorkspace> {
    let _lock = materialize.then(|| store.lock_execution(id)).transpose()?;
    let attempt_store = attempts(store);
    for attempt_id in attempt_store.list_ids()? {
        let attempt = attempt_store.read(&attempt_id)?;
        if attempt.work_item == id && attempt_store.read_final(&attempt_id)?.is_none() {
            bail!("node `{id}` already has unfinished attempt `{attempt_id}`; recover or finish it before starting another");
        }
    }
    authorize_execution_start(store, vcs, id, worker, force)?;
    let rejected = rejected_review_feedback(store, id);
    let base = explicit_base
        .or_else(|| rejected.as_ref().map(|(_, commit)| commit.as_str()))
        .unwrap_or("HEAD");
    let (input_commit, input_tree) = vcs.resolve_revision(base)?;
    let attempt_id = Ulid::new().to_string();
    let candidate_branch = format!("llaundry/candidates/{attempt_id}");
    let workbench = store
        .workbench_root()
        .canonicalize()
        .with_context(|| format!("resolving workbench {}", store.workbench_root().display()))?;
    let path = workbench.join(".llaundry-worktrees").join(&attempt_id);
    let mut attempt = Attempt {
        schema: 1,
        id: attempt_id.clone(),
        work_item: id.to_string(),
        worker,
        force,
        definition: store.node_version(id)?,
        input: git_artifact(&input_commit),
        input_tree: input_tree.clone(),
        branch: candidate_branch.clone(),
        workspace: path.to_string_lossy().into_owned(),
        backend: None,
        model: None,
        created_at: now_millis(),
        prepared: false,
    };
    if materialize {
        attempt_store.write(&attempt)?;
        vcs.commit_store(
            &store.store_name(),
            &format!("llaundry: begin attempt {attempt_id}"),
        )?;
        vcs.create_worktree(&path, &candidate_branch, &input_commit)?;
        attempt.prepared = true;
        attempt_store.write(&attempt)?;
        vcs.commit_store(
            &store.store_name(),
            &format!("llaundry: prepare attempt {attempt_id}"),
        )?;
    }
    Ok(ExecutionWorkspace {
        identity: ExecutionIdentity {
            node_id: id.to_string(),
            attempt_id,
            candidate_branch,
            force,
        },
        path,
        input_commit,
        input_tree,
        rejected_feedback: rejected.map(|(notes, _)| notes),
    })
}
/// Finalize the durable record of one backend session. The driver supplies
/// observations; the library applies them in the required order and ensures a
/// successful project-producing execution always gets its review node.
pub fn finalize_execution_attempt(
    store: &Store,
    vcs: &dyn Vcs,
    attempt_id: &str,
    worked_by: WorkedBy,
    started: i64,
    observed_reads: &[String],
    backend_succeeded: bool,
) -> Result<Option<String>> {
    let attempt_store = attempts(store);
    let attempt = attempt_store.read(attempt_id)?;
    if let Some((mut result, notes)) = attempt_store.read_result(attempt_id)? {
        result.producer = Some(
            WorkEvidence {
                attempt: Some(attempt_id.to_string()),
                backend: Some(worked_by.backend.clone()),
                model: worked_by.model.clone(),
            }
            .to_producer(),
        );
        store.write_result(&attempt.work_item, &result, &notes)?;
        attempt_store.write_result(attempt_id, &result, &notes)?;
        amend_context(store, vcs, &attempt.work_item, observed_reads)?;
        let (result, notes) = store.read_result(&attempt.work_item)?.unwrap();
        if attempt_of(&result).as_deref() != Some(attempt_id) {
            bail!("node latest result moved while finalizing attempt `{attempt_id}`");
        }
        attempt_store.write_result(attempt_id, &result, &notes)?;
    }
    attempt_store.finish(
        attempt_id,
        &AttemptFinished {
            at: now_millis(),
            executor_succeeded: backend_succeeded,
        },
    )?;
    vcs.commit_store(
        &store.store_name(),
        &format!("llaundry: finalize attempt {attempt_id}"),
    )?;
    let Some((result, _)) = attempt_store.read_result(attempt_id)? else {
        if backend_succeeded {
            bail!("successful attempt `{attempt_id}` produced no result");
        }
        return Ok(None);
    };
    if result.output.is_some() {
        return create_review_for_attempt(store, vcs, attempt_id).map(Some);
    }
    let _ = started;
    Ok(None)
}

/// Apply the standard post-finalization worktree retention policy. Project
/// candidates, failed backends, dirty worktrees, and explicit retention stay;
/// only clean non-project worktrees are removed.
pub fn finish_attempt_workspace(
    store: &Store,
    vcs: &dyn Vcs,
    attempt_id: &str,
    keep: bool,
) -> Result<bool> {
    let attempt_store = attempts(store);
    let attempt = attempt_store.read(attempt_id)?;
    let final_meta = attempt_store
        .read_final(attempt_id)?
        .with_context(|| format!("attempt `{attempt_id}` is not finalized"))?;
    let has_project_output = attempt_store
        .read_result(attempt_id)?
        .is_some_and(|(result, _)| result.output.is_some());
    let path = std::path::Path::new(&attempt.workspace);
    let clean = if keep || !final_meta.executor_succeeded || has_project_output {
        false
    } else {
        vcs.worktree_clean(path)?
    };
    if !llaundry_work::should_remove_workspace(
        keep,
        final_meta.executor_succeeded,
        has_project_output,
        clean,
    ) {
        return Ok(false);
    }
    vcs.remove_worktree(path)?;
    Ok(true)
}
/// Stamp onto a node's recorded result which backend (and model) produced it.
/// The worker itself does not reliably know what it runs on, so the driver
/// records this after the session — the same move as [`amend_context`].
///
/// `since` guards against mislabelling: only a result recorded at or after it
/// (i.e. by *this* session) is stamped. A rework session that exits without
/// writing a new result leaves the previous result — and its previous stamp —
/// untouched. A no-op, returning `false`, when there is no result to stamp.
pub fn amend_worker(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    worked_by: WorkedBy,
    since: i64,
) -> Result<bool> {
    let Some((mut result, notes)) = store.read_result(id)? else {
        return Ok(false);
    };
    if result.at < since {
        return Ok(false);
    }
    result.producer = Some(
        WorkEvidence {
            attempt: attempt_of(&result),
            backend: Some(worked_by.backend),
            model: worked_by.model,
        }
        .to_producer(),
    );
    store.write_result(id, &result, &notes)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: worked by {id}"))?;
    Ok(true)
}

/// Create the human review for a completed project-producing attempt.
/// Idempotent for the same implementation result.
pub fn create_review(store: &Store, vcs: &dyn Vcs, implementation: &str) -> Result<String> {
    let Some((result, _)) = store.read_result(implementation)? else {
        bail!("node `{implementation}` has no result to review");
    };
    let attempt_id = attempt_of(&result)
        .with_context(|| format!("node `{implementation}` has no execution attempt id"))?;
    create_review_for_attempt(store, vcs, &attempt_id)
}

pub fn create_review_for_attempt(store: &Store, vcs: &dyn Vcs, attempt_id: &str) -> Result<String> {
    let attempt_store = attempts(store);
    let review_store = reviews(store);
    let attempt = attempt_store.read(attempt_id)?;
    let implementation = &attempt.work_item;
    if attempt_store.read_final(attempt_id)?.is_none() {
        bail!("attempt `{attempt_id}` is not finalized");
    }
    let Some((result, _)) = attempt_store.read_result(attempt_id)? else {
        bail!("attempt `{attempt_id}` has no result to review");
    };
    let candidate_artifact = result
        .output
        .clone()
        .with_context(|| format!("node `{implementation}` produced no project content"))?;
    let reviewed_result = attempt_store.result_version(attempt_id)?;

    for id in review_store.list_candidate_ids()? {
        let candidate = review_store.candidate(&id)?;
        if candidate.subject == *implementation
            && candidate.artifact == candidate_artifact
            && candidate.result == reviewed_result
        {
            return Ok(id);
        }
    }

    let id = format!("node-{}", Ulid::new());
    let meta = NodeMeta {
        schema: 1,
        author: Author::Machine,
        assignee: Some(Author::Human),
        depends_on: Vec::new(),
        derived_from: vec![implementation.to_string()],
        extensions: Default::default(),
    };
    let description = format!("Review candidate for {implementation}\n\nInspect the exact candidate named in the review record. Accept it only if this content may be integrated into main; otherwise reject it with actionable comments.");
    store.write_node(&id, &meta, &description)?;
    review_store.create_candidate(&Candidate {
        id: id.clone(),
        subject: implementation.to_string(),
        attempt: attempt_id.to_string(),
        branch: attempt.branch.clone(),
        result: reviewed_result,
        artifact: candidate_artifact,
    })?;
    vcs.commit_store(
        &store.store_name(),
        &format!("llaundry: review {implementation}"),
    )?;
    Ok(id)
}
pub struct ReviewWorkspace {
    pub branch: String,
    pub path: std::path::PathBuf,
    pub candidate_commit: String,
}

pub struct ReviewWorkspaceStatus {
    pub branch: String,
    pub path: std::path::PathBuf,
    pub exists: bool,
    pub clean: Option<bool>,
}

fn review_workspace_location(store: &Store, id: &str) -> Result<(String, std::path::PathBuf)> {
    let branch = format!("llaundry/reviews/{id}");
    let workbench = store
        .workbench_root()
        .canonicalize()
        .with_context(|| format!("resolving workbench {}", store.workbench_root().display()))?;
    Ok((
        branch,
        workbench
            .join(".llaundry-worktrees")
            .join(format!("review-{id}")),
    ))
}

/// The review candidate for a review node id.
fn candidate_of(store: &Store, id: &str) -> Result<Candidate> {
    reviews(store)
        .candidate(id)
        .map_err(|_| anyhow::anyhow!("node `{id}` is not a review"))
}

pub fn review_workspace_status(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
) -> Result<ReviewWorkspaceStatus> {
    candidate_of(store, id)?;
    let (branch, path) = review_workspace_location(store, id)?;
    let exists = path.exists();
    let clean = exists.then(|| vcs.worktree_clean(&path)).transpose()?;
    Ok(ReviewWorkspaceStatus {
        branch,
        path,
        exists,
        clean,
    })
}

/// Prepare the canonical branch and linked worktree for reviewer-proposed
/// edits. The library owns the open-review and exact-candidate checks.
pub fn prepare_review_edits(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<ReviewWorkspace> {
    let candidate = candidate_of(store, id)?;
    if store.read_result(id)?.is_some() {
        bail!("review `{id}` is already closed");
    }
    let candidate_ref = format!("refs/heads/{}", candidate.branch);
    if vcs.ref_commit(&candidate_ref)?.as_deref() != Some(&candidate.artifact.id) {
        bail!(
            "candidate branch `{}` no longer points to reviewed commit {}",
            candidate.branch,
            short(&candidate.artifact.id)
        );
    }
    let (branch, path) = review_workspace_location(store, id)?;
    if !path.exists() {
        vcs.create_worktree(&path, &branch, &candidate.artifact.id)?;
    }
    Ok(ReviewWorkspace {
        branch,
        path,
        candidate_commit: candidate.artifact.id,
    })
}

/// Remove a clean worktree belonging to a closed review. Its review branch is
/// deliberately retained as suggestion history.
pub fn cleanup_review_workspace(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<bool> {
    candidate_of(store, id)?;
    if store.read_result(id)?.is_none() {
        bail!("review `{id}` is still open");
    }
    let status = review_workspace_status(store, vcs, id)?;
    if !status.exists {
        return Ok(false);
    }
    if status.clean != Some(true) {
        bail!(
            "review worktree {} has uncommitted changes",
            status.path.display()
        );
    }
    vcs.remove_worktree(&status.path)?;
    Ok(true)
}

/// Complete a review. Acceptance publishes exactly the pinned candidate;
/// rejection records feedback and permits another implementation attempt.
#[allow(clippy::too_many_arguments)] // mirrors the accept/reject CLI fields
pub fn decide_review(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    decision: ReviewDecision,
    notes: &str,
    target: &str,
    mut suggestion_branch: Option<String>,
    mut suggestion_commit: Option<String>,
) -> Result<()> {
    let candidate = candidate_of(store, id)?;
    if store.read_result(id)?.is_some() {
        bail!("review `{id}` is already closed");
    }
    if decision == ReviewDecision::Accepted && reviews(store).read_publication(id)?.is_some() {
        return recover_publication(store, vcs, id);
    }
    if decision == ReviewDecision::Rejected && notes.trim().is_empty() {
        bail!("a rejected review needs comments");
    }
    if decision == ReviewDecision::Rejected
        && suggestion_branch.is_none()
        && suggestion_commit.is_none()
    {
        let branch = format!("llaundry/reviews/{id}");
        let reference = format!("refs/heads/{branch}");
        if let Some(commit) = vcs.ref_commit(&reference)? {
            if commit != candidate.artifact.id {
                suggestion_branch = Some(branch);
                suggestion_commit = Some(commit);
            }
        }
    }
    if suggestion_branch.is_some() != suggestion_commit.is_some() {
        bail!("suggestion branch and commit must be supplied together");
    }
    if let (Some(branch), Some(commit)) = (&suggestion_branch, &suggestion_commit) {
        let reference = format!("refs/heads/{branch}");
        if vcs.ref_commit(&reference)?.as_deref() != Some(commit) {
            bail!("suggestion branch `{branch}` does not point to `{commit}`");
        }
    }

    let candidate_ref = format!("refs/heads/{}", candidate.branch);
    if vcs.ref_commit(&candidate_ref)?.as_deref() != Some(&candidate.artifact.id) {
        bail!(
            "candidate branch `{}` no longer points to reviewed commit {}",
            candidate.branch,
            short(&candidate.artifact.id)
        );
    }

    if decision == ReviewDecision::Accepted {
        if suggestion_branch.is_some() {
            bail!("accepted reviews cannot carry proposed edits");
        }
        let target_ref = format!("refs/heads/{target}");
        let target_previous = vcs
            .ref_commit(&target_ref)?
            .with_context(|| format!("target branch `{target}` does not exist"))?;
        let publication = PublicationIntent {
            schema: 1,
            review: id.to_string(),
            implementation: candidate.subject.clone(),
            candidate_commit: candidate.artifact.id.clone(),
            target: target.to_string(),
            target_ref,
            target_previous,
            notes: notes.to_string(),
            prepared_at: now_millis(),
            completed_at: None,
        };
        reviews(store).write_publication(&publication)?;
        vcs.commit_store(
            &store.store_name(),
            &format!("llaundry: prepare publication {id}"),
        )?;
        return recover_publication(store, vcs, id);
    }

    close_review(store, id, decision, notes, suggestion_commit.as_deref())?;
    vcs.commit_store(
        &store.store_name(),
        &format!("llaundry: {:?} review {id}", decision),
    )
}

/// Record a review node's closure: its plain core result (the review work is
/// done) and the structured decision owned by the review application.
fn close_review(
    store: &Store,
    id: &str,
    decision: ReviewDecision,
    notes: &str,
    suggestion_commit: Option<&str>,
) -> Result<()> {
    let result = ResultMeta {
        at: now_millis(),
        author: Author::Human,
        definition: store.node_version(id)?,
        outcome: Outcome::Done,
        // The immutable review candidate is the authoritative pin. A normal
        // dependency pin here would make acceptance stale its own review when
        // the implementation's result is later superseded.
        consumed: Vec::new(),
        context: Vec::new(),
        output: None,
        producer: None,
    };
    store.write_result(id, &result, notes)?;
    reviews(store).record_decision(&llaundry_review::Decision {
        candidate: id.into(),
        kind: decision,
        notes: notes.into(),
        suggestion: suggestion_commit.map(git_artifact),
    })?;
    Ok(())
}

/// Resume or finish a prepared review publication. Safe to call before or
/// after the target ref has moved; any unrelated target movement is refused.
pub fn recover_publication(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<()> {
    let review_store = reviews(store);
    let mut publication = review_store
        .read_publication(id)?
        .with_context(|| format!("review `{id}` has no prepared publication"))?;
    if publication.completed_at.is_some() {
        return Ok(());
    }
    let candidate = candidate_of(store, id)?;
    if candidate.subject != publication.implementation
        || candidate.artifact.id != publication.candidate_commit
    {
        bail!("publication intent no longer matches review `{id}`");
    }
    let candidate_ref = format!("refs/heads/{}", candidate.branch);
    if vcs.ref_commit(&candidate_ref)?.as_deref() != Some(&publication.candidate_commit) {
        bail!(
            "candidate branch `{}` no longer points to reviewed commit {}",
            candidate.branch,
            short(&publication.candidate_commit)
        );
    }
    let target_now = vcs
        .ref_commit(&publication.target_ref)?
        .with_context(|| format!("target branch `{}` does not exist", publication.target))?;
    if target_now == publication.target_previous {
        let expected = git_artifact(&publication.target_previous);
        match llaundry_review::publish_exact(
            &VcsPublisher { vcs }, &candidate, &publication.target, &expected,
        ) {
            Ok(()) => {}
            Err(llaundry_review::PublishError::NotFastForward) => bail!("reviewed candidate cannot fast-forward `{}`; create follow-up implementation work and review the changed result", publication.target),
            Err(llaundry_review::PublishError::Backend(error)) => return Err(error),
        }
    } else if target_now != publication.candidate_commit {
        bail!(
            "target `{}` moved from {} to {} while publication was pending",
            publication.target,
            short(&publication.target_previous),
            short(&target_now)
        );
    }

    close_review(store, id, ReviewDecision::Accepted, &publication.notes, None)?;
    let Some((implementation, _)) = store.read_result(&publication.implementation)? else {
        bail!("reviewed implementation disappeared");
    };
    if implementation.output.as_ref().map(|a| a.id.as_str())
        != Some(publication.candidate_commit.as_str())
    {
        bail!("implementation result changed while review was open");
    }
    publication.completed_at = Some(now_millis());
    review_store.write_publication(&publication)?;
    vcs.commit_store(
        &store.store_name(),
        &format!("llaundry: complete publication {id}"),
    )
}

struct VcsPublisher<'a> {
    vcs: &'a dyn Vcs,
}
impl llaundry_review::Publisher for VcsPublisher<'_> {
    type Error = anyhow::Error;
    fn publish(
        &self,
        candidate: &Candidate,
        target: &str,
        expected_previous: &llaundry_core::ArtifactRef,
    ) -> Result<bool> {
        self.vcs
            .publish_fast_forward(target, &expected_previous.id, &candidate.artifact.id)
    }
}

/// The producing attempt recorded in a result's producer evidence, if any.
fn attempt_of(result: &ResultMeta) -> Option<String> {
    result
        .producer
        .as_ref()
        .and_then(WorkEvidence::from_producer)
        .and_then(|evidence| evidence.attempt)
}

fn latest_review_decision(
    store: &Store,
    implementation: &str,
    commit: &str,
) -> Option<ReviewDecision> {
    let review_store = reviews(store);
    let mut matching = Vec::new();
    for id in review_store.list_candidate_ids().ok()? {
        let Ok(candidate) = review_store.candidate(&id) else {
            continue;
        };
        if candidate.subject == implementation && candidate.artifact.id == commit {
            matching.push(id);
        }
    }
    matching.sort();
    matching
        .last()
        .and_then(|id| review_store.decision(id).ok().flatten())
        .map(|decision| decision.kind)
}
/// Human-facing state for review-gated work. Core dependency semantics remain
/// based on [`Status`]; this query explains why an otherwise-open node cannot
/// or should not be worked.
pub fn node_state(store: &Store, id: &str) -> NodeState {
    llaundry_review::node_state(&ReviewAdapter { store }, id, current_status(store, id))
}

/// Derives the review application's presentation state from the candidate,
/// decision, and publication stores — never from core node results.
struct ReviewAdapter<'a> {
    store: &'a Store,
}
impl ReviewAdapter<'_> {
    /// Whether a completed publication integrated `commit` for `implementation`.
    fn published(&self, implementation: &str, commit: &str) -> bool {
        let review_store = reviews(self.store);
        let Ok(ids) = review_store.list_publication_ids() else {
            return false;
        };
        ids.iter().any(|review| {
            review_store
                .read_publication(review)
                .ok()
                .flatten()
                .is_some_and(|publication| {
                    publication.completed_at.is_some()
                        && publication.implementation == implementation
                        && publication.candidate_commit == commit
                })
        })
    }
}
impl llaundry_review::ReviewStateView for ReviewAdapter<'_> {
    type Error = anyhow::Error;
    fn is_review(&self, id: &str) -> Result<bool> {
        Ok(reviews(self.store).is_candidate(id))
    }
    fn decision(&self, id: &str) -> Result<Option<ReviewDecision>> {
        Ok(reviews(self.store)
            .decision(id)?
            .map(|decision| decision.kind))
    }
    fn integrated(&self, id: &str) -> Result<bool> {
        let Some((result, _)) = self.store.read_result(id)? else {
            return Ok(false);
        };
        let Some(output) = &result.output else {
            return Ok(false);
        };
        Ok(self.published(id, &output.id))
    }
    fn pending_artifact(&self, id: &str) -> Result<Option<String>> {
        let Some((result, _)) = self.store.read_result(id)? else {
            return Ok(None);
        };
        let Some(output) = &result.output else {
            return Ok(None);
        };
        // Only isolated machine executions produce review-gated candidates.
        if attempt_of(&result).is_none() || self.published(id, &output.id) {
            return Ok(None);
        }
        Ok(Some(output.id.clone()))
    }
    fn latest_decision(&self, subject: &str, artifact: &str) -> Result<Option<ReviewDecision>> {
        Ok(latest_review_decision(self.store, subject, artifact))
    }
}
/// Review-aware execution policy layered on top of core graph readiness.
///
/// A rejected immutable candidate may be reworked even though its core result
/// remains a valid `done` result. Pending and accepted candidates are not
/// dispatched. This policy belongs to the work/review composition layer and is
/// intentionally separate from [`is_ready`].
pub fn is_dispatchable(store: &Store, vcs: &dyn Vcs, id: &str) -> bool {
    if !blockers(store, vcs, id).is_empty() {
        return false;
    }
    let Some((result, _)) = store.read_result(id).ok().flatten() else {
        return true;
    };
    if current_status(store, id) != Status::Done {
        return true;
    }
    let Some(output) = &result.output else {
        return false;
    };
    latest_review_decision(store, id, &output.id) == Some(ReviewDecision::Rejected)
}
/// Latest rejected review of the implementation's current candidate. Returns
/// its prose feedback and the best commit from which to start rework (reviewer
/// suggestions when present, otherwise the rejected candidate itself).
pub fn rejected_review_feedback(store: &Store, implementation: &str) -> Option<(String, String)> {
    let attempt_store = attempts(store);
    let review_store = reviews(store);
    let mut attempt_ids = attempt_store.list_ids().ok()?;
    attempt_ids.reverse();
    for attempt_id in attempt_ids {
        let Ok(attempt) = attempt_store.read(&attempt_id) else {
            continue;
        };
        if attempt.work_item != implementation {
            continue;
        }
        let Some((attempt_result, _)) = attempt_store.read_result(&attempt_id).ok().flatten()
        else {
            continue;
        };
        let Some(output) = attempt_result.output else {
            continue;
        };
        let mut review_ids = Vec::new();
        for id in review_store.list_candidate_ids().ok()? {
            let Ok(candidate) = review_store.candidate(&id) else {
                continue;
            };
            if candidate.attempt == attempt_id {
                review_ids.push(id);
            }
        }
        review_ids.sort();
        let review_id = review_ids.last()?.clone();
        let decision = review_store.decision(&review_id).ok().flatten()?;
        return (decision.kind == ReviewDecision::Rejected).then(|| {
            let base = decision
                .suggestion
                .map(|artifact| artifact.id)
                .unwrap_or(output.id);
            (decision.notes, base)
        });
    }
    None
}

/// Recover safe incomplete attempt phases. Preparation is retried from the
/// durable input; sealed project results get their missing review recreated.
pub fn recover_attempt(store: &Store, vcs: &dyn Vcs, attempt_id: &str) -> Result<Option<String>> {
    let attempt_store = attempts(store);
    let mut attempt = attempt_store.read(attempt_id)?;
    if !attempt.prepared {
        vcs.create_worktree(
            std::path::Path::new(&attempt.workspace),
            &attempt.branch,
            &attempt.input.id,
        )?;
        attempt.prepared = true;
        attempt_store.write(&attempt)?;
        vcs.commit_store(
            &store.store_name(),
            &format!("llaundry: recover attempt {attempt_id}"),
        )?;
    }
    if attempt_store.read_final(attempt_id)?.is_some()
        && attempt_store
            .read_result(attempt_id)?
            .is_some_and(|(result, _)| result.output.is_some())
    {
        return create_review_for_attempt(store, vcs, attempt_id).map(Some);
    }
    Ok(None)
}
/// The backend/model recorded as having produced a result, if stamped.
pub fn worked_by(result: &ResultMeta) -> Option<WorkedBy> {
    result
        .producer
        .as_ref()
        .and_then(WorkEvidence::from_producer)
        .and_then(|evidence| evidence.worked_by())
}
/// Presentation details of a review node, gathered from the review
/// application's candidate and decision stores. `None` when `id` is not a
/// review candidate.
pub struct ReviewInfo {
    pub subject: String,
    pub branch: String,
    pub candidate_commit: String,
    pub decision: Option<ReviewDecision>,
    pub suggestion_branch: Option<String>,
    pub suggestion_commit: Option<String>,
}

/// Gather review presentation details for a node, or `None` if it is not a
/// review candidate.
pub fn review_info(store: &Store, id: &str) -> Result<Option<ReviewInfo>> {
    let review_store = reviews(store);
    if !review_store.is_candidate(id) {
        return Ok(None);
    }
    let candidate = review_store.candidate(id)?;
    let decision = review_store.decision(id)?;
    let suggestion_commit = decision
        .as_ref()
        .and_then(|decision| decision.suggestion.as_ref())
        .map(|artifact| artifact.id.clone());
    Ok(Some(ReviewInfo {
        subject: candidate.subject,
        branch: candidate.branch,
        candidate_commit: candidate.artifact.id,
        decision: decision.map(|decision| decision.kind),
        suggestion_branch: suggestion_commit
            .as_ref()
            .map(|_| format!("llaundry/reviews/{id}")),
        suggestion_commit,
    }))
}

// --- review/attempt sections of ops::check ---------------------------------
/*
    let review_store = reviews(store);
    let mut reviews_by_attempt: std::collections::HashMap<String, usize> = Default::default();
    for id in review_store.list_candidate_ids()? {
        let candidate = match review_store.candidate(&id) {
            Ok(candidate) => candidate,
            Err(e) => {
                problems.push(format!("review {id}: unreadable candidate ({e:#})"));
                continue;
            }
        };
        *reviews_by_attempt.entry(candidate.attempt).or_default() += 1;
        if !store.exists(&id) {
            problems.push(format!("review {id}: review node is missing"));
            continue;
        }
        if !store.exists(&candidate.subject) {
            problems.push(format!(
                "review {id}: reviewed implementation `{}` is missing",
                candidate.subject
            ));
        }
        if let Ok((meta, _)) = store.read_node(&id) {
            if !meta.derived_from.iter().any(|node| node == &candidate.subject) {
                problems.push(format!(
                    "review {id}: review target `{}` is not also a derived_from relationship",
                    candidate.subject
                ));
            }
        }
    }

    let attempt_store = attempts(store);
    for attempt_id in attempt_store.list_ids()? {
        let attempt = match attempt_store.read(&attempt_id) {
            Ok(attempt) => attempt,
            Err(e) => {
                problems.push(format!("attempt {attempt_id}: unreadable metadata ({e:#})"));
                continue;
            }
        };
        if !store.exists(&attempt.work_item) {
            problems.push(format!(
                "attempt {attempt_id}: node `{}` is missing",
                attempt.work_item
            ));
        }
        let result = attempt_store.read_result(&attempt_id)?;
        if result.is_some() && !attempt.prepared {
            problems.push(format!(
                "attempt {attempt_id}: result exists before workspace preparation"
            ));
        }
        if let Some((result, _)) = &result {
            if attempt_of(result).as_deref() != Some(&attempt_id) {
                problems.push(format!(
                    "attempt {attempt_id}: result names a different attempt"
                ));
            }
        }
        if let Some(final_meta) = attempt_store.read_final(&attempt_id)? {
            if final_meta.executor_succeeded && result.is_none() {
                problems.push(format!(
                    "attempt {attempt_id}: successful final record has no result"
                ));
            }
            if result.as_ref().is_some_and(|(r, _)| r.output.is_some()) {
                let count = reviews_by_attempt.get(&attempt_id).copied().unwrap_or(0);
                if count != 1 {
                    problems.push(format!(
                        "attempt {attempt_id}: sealed project result has {count} reviews"
                    ));
                }
            }
        }
    }
    for review in review_store.list_publication_ids()? {
        let publication = match review_store.read_publication(&review)? {
            Some(publication) => publication,
            None => continue,
        };
        if !store.exists(&review) {
            problems.push(format!("publication {review}: review node is missing"));
            continue;
        }
        if publication.completed_at.is_none() {
            problems.push(format!(
                "publication {review}: incomplete; run recover-publication"
            ));
        }
        if publication.completed_at.is_some() {
            let accepted = review_store
                .decision(&review)?
                .is_some_and(|decision| decision.kind == ReviewDecision::Accepted);
            if !accepted {
                problems.push(format!(
                    "publication {review}: marked complete without matching accepted review"
                ));
            }
        }
    }
*/

// --- deep attempt/review sections of ops::verify_pairing -------------------
/*
    if deep {
        let attempt_store = attempts(store);
        for attempt_id in attempt_store.list_ids()? {
            let attempt = attempt_store.read(&attempt_id)?;
            if attempt.prepared {
                let reference = format!("refs/heads/{}", attempt.branch);
                match vcs.ref_commit(&reference)? {
                    None => problems.push(format!(
                        "attempt {attempt_id}: candidate branch {} is missing",
                        attempt.branch
                    )),
                    Some(tip) => {
                        if let Some((result, _)) = attempt_store.read_result(&attempt_id)? {
                            if let Some(output) = result.output {
                                if tip != output.id {
                                    problems.push(format!(
                                        "attempt {attempt_id}: candidate branch points to {}, result records {}",
                                        short(&tip), short(&output.id)
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        let review_store = reviews(store);
        for id in review_store.list_candidate_ids()? {
            let candidate = review_store.candidate(&id)?;
            let reference = format!("refs/heads/{}", candidate.branch);
            match vcs.ref_commit(&reference)? {
                Some(commit) if commit != candidate.artifact.id => problems.push(format!(
                    "{id}: candidate branch {} moved from reviewed commit {} to {}",
                    candidate.branch,
                    short(&candidate.artifact.id),
                    short(&commit)
                )),
                None => problems.push(format!(
                    "{id}: candidate branch {} is missing",
                    candidate.branch
                )),
                _ => {}
            }
        }
*/
