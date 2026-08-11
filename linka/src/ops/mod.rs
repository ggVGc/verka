//! Graph operations and derived queries.
//!
//! Every mutating operation ([`add`], [`link`], [`edit`], [`complete`], [`fail`])
//! takes the workbench-wide mutation lock, requires a clean store, commits its
//! complete store change as one git commit, and verifies the store is clean
//! before releasing the lock. The project repository is checked only where output
//! provenance is asserted: [`complete`] refuses undeclared dirty writes
//! ([`require_clean_except`]); pure graph edits never gate on project state.
//! The derived queries ([`node_state`], [`staleness`], [`blockers`],
//! [`is_ready`]) recompute from the node files and are never stored.
//!
//! All git interaction goes through `&dyn Vcs`, so the whole module is
//! unit-testable with an in-memory fake — no git binary, repository, or identity
//! required. (Blob hashing for versions and pins is computed locally.)
//!
//! ## Snapshot/submission protocol for orchestrators
//!
//! External callers (an orchestrator such as Orka, or any other tool) work
//! against a stable, version-checked protocol rather than reimplementing
//! capture or validation:
//!
//! * [`snapshot_work`] freezes the exact graph, context, and project inputs of
//!   ready work into a [`WorkSnapshot`] — the authoritative frozen input.
//! * [`capture_submission`] consumes a caller's frozen snapshot, captures the
//!   declared outputs in the caller's [`Vcs`] execution context, and submits a
//!   version-checked result (success with or without outputs, or failure). It
//!   revalidates every frozen field; on a graph conflict it records nothing and
//!   returns [`SubmissionError::Conflict`] carrying structured
//!   [`SubmissionConflict`] values.
//! * [`submit_result`] is the lower-level version-checked write for callers
//!   that captured their own artifact.
//!
//! Producer-specific evidence rides along as a namespaced [`ProducerEvidence`],
//! which Linka preserves verbatim and never interprets.
//! The operations are grouped into submodules and re-exported here, so
//! `linka::ops::*` is unchanged: [`mutate`] writes, [`state`] derives,
//! [`submit`] runs the snapshot/submission protocol, [`query`] scans,
//! [`check`] verifies, and [`pairing`] binds the store to its project.

use anyhow::{bail, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::candidate::{CandidateRecord, CandidateState, CandidateStore};
use crate::model::{
    ArtifactRef, Author, Blocker, BlockerReason, CandidateId, ConsumedNode, ContextPin, Currency,
    DefinitionVersion, DepKind, IntegrationStatus, NewNodeAttachment, NodeAttachment, NodeId,
    NodeMeta, NodeState, Outcome, ProducerEvidence, ProjectPath, ProjectSnapshot, RecordedOutcome,
    ResultMeta, ResultOutcome, ResultSubmission, ResultVersion, StalenessReason,
    SubmissionConflict, VerificationOutcome, VerificationSubmission, WorkSnapshot,
};
use crate::model::{
    ATTACHMENT_SCHEMA, DEFINITION_SCHEMA, OBSERVATION_SCHEMA, RESULT_SCHEMA, SNAPSHOT_SCHEMA,
};
use crate::pairing::Pairing;
use crate::store::{blob_id, file_blob, MutationLock, Store};
use crate::vcs::Vcs;

mod check;
mod mutate;
mod pairing_ops;
mod query;
mod state;
mod submit;
#[cfg(test)]
mod tests;

pub use check::*;
pub use mutate::*;
pub use pairing_ops::*;
pub use query::*;
pub use state::*;
pub use submit::*;

/// Enforce that the project working tree is clean apart from `allowed` paths —
/// used by [`complete`], whose job is to commit exactly the produced outputs.
/// This is the whole clean-tree rule now: the workbench repository is entirely
/// machine-written and swept by every mutating operation, so only the project
/// repository — and only at completion, where output provenance is asserted —
/// needs checking.
pub fn require_clean_except(vcs: &dyn Vcs, allowed: &[String]) -> Result<()> {
    let allowed: std::collections::HashSet<&str> = allowed.iter().map(String::as_str).collect();
    let stray: Vec<String> = vcs
        .dirty_paths()?
        .into_iter()
        .filter(|p| !allowed.contains(p.as_str()))
        .collect();
    if !stray.is_empty() {
        bail!(
            "uncommitted project changes outside the declared outputs; declare or revert them:\n  {}",
            stray.join("\n  ")
        );
    }
    Ok(())
}

/// First 12 characters of a hash, for compact display.
pub fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// A result's output commit id, if its work produced project content.
pub fn output_commit(result: &ResultMeta) -> Option<&str> {
    result.output.as_ref().map(|artifact| artifact.id.as_str())
}

pub fn short_definition(version: &DefinitionVersion) -> String {
    format!(
        "{}/{}",
        short(&version.metadata),
        short(&version.description)
    )
}

pub fn short_result(version: &ResultVersion) -> String {
    format!(
        "{}/{}",
        short(&version.metadata),
        version.notes.as_deref().map_or("none", short)
    )
}

/// Pin the current version, result, and output of each node in `nodes`.
fn pin_node_list(store: &Store, nodes: &[crate::model::NodeId]) -> Result<Vec<ConsumedNode>> {
    nodes
        .iter()
        .map(|dep| {
            let definition = store
                .node_version(dep)
                .with_context(|| format!("cannot pin unknown dependency `{dep}`"))?;
            let current = store.read_result(dep)?;
            Ok(ConsumedNode {
                id: dep.clone(),
                definition,
                result: store.current_result_version(dep)?,
                outcome: current.as_ref().map(|(result, _)| result.outcome),
                output: current.and_then(|(result, _)| result.output),
            })
        })
        .collect()
}

/// Pin each context path by its current content; errors if a file is missing.
fn pin_context(
    store: &Store,
    vcs: &dyn Vcs,
    revision: Option<&str>,
    paths: &[String],
) -> Result<Vec<ContextPin>> {
    let root = store.project_root();
    paths
        .iter()
        .map(|path| {
            let path: crate::model::ProjectPath = path.parse().map_err(anyhow::Error::msg)?;
            let blob = match revision {
                Some(revision) => vcs.file_blob_at(revision, path.as_str())?,
                None => vcs
                    .file_blob(path.as_str())?
                    .or(project_file_blob(&root, &path)?),
            }
            .with_context(|| format!("cannot pin `{path}`: file not found"))?;
            Ok(ContextPin {
                path,
                identity: blob,
                observed: false,
            })
        })
        .collect()
}

fn project_file_blob(
    root: &std::path::Path,
    path: &crate::model::ProjectPath,
) -> Result<Option<String>> {
    let candidate = root.join(path.as_str());
    match std::fs::canonicalize(&candidate) {
        Ok(resolved) => {
            let root = std::fs::canonicalize(root)?;
            if !resolved.starts_with(&root) {
                bail!("project path `{path}` escapes the project root through a symlink");
            }
            file_blob(&resolved)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("resolving project path `{path}`")),
    }
}

/// The paired project repository's identity (its root commit), or an empty
/// string for an unpaired store — stores predating pairing exist.
fn paired_repository(store: &Store) -> Result<String> {
    Ok(Pairing::load(store.root())?
        .map(|pairing| pairing.root_commit)
        .unwrap_or_default())
}

fn current_project_snapshot(store: &Store, vcs: &dyn Vcs) -> Result<ProjectSnapshot> {
    let revision = vcs.head_commit()?.unwrap_or_default();
    let tree = if revision.is_empty() {
        String::new()
    } else {
        vcs.tree_id(&revision)?
    };
    Ok(ProjectSnapshot {
        scheme: "git".into(),
        repository: paired_repository(store)?,
        tree,
        revision,
    })
}

fn git_artifact(store: &Store, commit: &str) -> Result<ArtifactRef> {
    Ok(ArtifactRef {
        scheme: "git-commit".into(),
        repository: paired_repository(store)?,
        id: commit.into(),
    })
}

pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Build an output commit's message: the caller's message (or the node's
/// title as a fallback) plus the `Linka-Node` trailer and, when the work had
/// an input commit, the `Linka-Input` trailer recording what it was built from.
pub(super) fn output_commit_message(id: &NodeId, message: String, input: Option<&str>) -> String {
    let mut commit_message = format!("{message}\n\nLinka-Node: {id}");
    if let Some(input) = input {
        commit_message.push_str(&format!("\nLinka-Input: {input}"));
    }
    commit_message
}
/// Whether `outcome`'s kind (work vs. verification) is the kind a node that
/// does (`verifies == true`) or does not itself verify a candidate may record.
pub(super) fn outcome_kind_matches(verifies: bool, outcome: ResultOutcome) -> bool {
    matches!(
        (verifies, outcome),
        (false, ResultOutcome::Work(_)) | (true, ResultOutcome::Verification(_))
    )
}
pub(super) fn verification_requires_review_result(id: &NodeId) -> String {
    format!("verification node `{id}` requires an accepted, rejected, or abandoned review result")
}
pub(super) fn ordinary_requires_work_result(id: &NodeId) -> String {
    format!("ordinary node `{id}` requires a done or failed work result")
}
/// Bail if `id` (identified by `meta`) is a verification node: [`complete`] and
/// [`fail`] only ever record a work outcome, which a verification node never
/// accepts.
pub(super) fn require_ordinary_node(meta: &NodeMeta, id: &NodeId) -> Result<()> {
    if meta.verifies.is_some() {
        bail!(verification_requires_review_result(id));
    }
    Ok(())
}
pub(super) fn result_satisfies_dependency(outcome: ResultOutcome) -> bool {
    matches!(
        outcome,
        ResultOutcome::Work(Outcome::Done)
            | ResultOutcome::Verification(VerificationOutcome::Accepted)
    )
}
/// Whether a result with this outcome must carry a full pin for every declared
/// edge (a done work result, or any verification decision).
pub(super) fn outcome_requires_full_pins(outcome: ResultOutcome) -> bool {
    matches!(
        outcome,
        ResultOutcome::Work(Outcome::Done) | ResultOutcome::Verification(_)
    )
}
