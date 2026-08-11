//! The operations: fact writers, the snapshot/submission protocol, and
//! publication.
//!
//! Every fact writer takes the workbench-wide mutation lock, requires a clean
//! store, performs its complete action, and commits it as one git commit. The
//! project repository is inspected only where output provenance is asserted;
//! pure graph edits never gate on project state.
//!
//! All git interaction goes through `&dyn Vcs`, so the whole module is
//! unit-testable with an in-memory fake — no git binary, repository, or
//! configured identity required.

use anyhow::{bail, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::graph::project_file_blob;
use crate::model::{
    ArtifactRef, Author, ConsumedNode, ContextPin, DefinitionVersion, NodeId, Outcome, ProjectPath,
    ProjectSnapshot, ResultVersion,
};
use crate::pairing::Pairing;
use crate::store::Store;
use crate::vcs::Vcs;

mod mutate;
mod submit;
#[cfg(test)]
mod tests;

pub use mutate::{
    add, attach, edit, init_workbench, link, pair, record_observed_context, register_candidate,
    EditOutcome, InitializedWorkbench, NewNode,
};
pub use submit::{
    complete, publish, require_consistent_project_head, snapshot, submit, SubmissionError,
};

/// Enforce that the project working tree is clean apart from `allowed` — used
/// where Linka is about to commit exactly the produced outputs.
///
/// Declared outputs match dirty paths by *path prefix*, not string equality:
/// git reports dirty paths per file, so a declared output directory has to
/// accept every file beneath it.
pub fn require_clean_except(vcs: &dyn Vcs, allowed: &[String]) -> Result<()> {
    let stray: Vec<String> = vcs
        .dirty_paths()?
        .into_iter()
        .filter(|dirty| !allowed.iter().any(|allowed| covers(allowed, dirty)))
        .collect();
    if !stray.is_empty() {
        bail!(
            "uncommitted project changes outside the declared outputs; declare or revert them:\n  {}",
            stray.join("\n  ")
        );
    }
    Ok(())
}

/// Whether a declared output path covers a dirty path: the same file, or a
/// directory containing it.
fn covers(declared: &str, dirty: &str) -> bool {
    dirty == declared
        || dirty
            .strip_prefix(declared)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// First 12 characters of a hash, for compact display.
pub fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
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

/// Pin the current definition, result, and output of each node in `nodes`.
fn pin_node_list(store: &Store, nodes: &[NodeId]) -> Result<Vec<ConsumedNode>> {
    nodes
        .iter()
        .map(|id| {
            let definition = store
                .node_version(id)
                .with_context(|| format!("cannot pin unknown related node `{id}`"))?;
            let current = store.read_result(id)?;
            Ok(ConsumedNode {
                id: id.clone(),
                definition,
                result: store.current_result_version(id)?,
                outcome: current.as_ref().map(|(result, _)| result.outcome),
                output: current.and_then(|(result, _)| result.output),
            })
        })
        .collect()
}

/// Pin each context path by its content at `revision`, or in the working tree
/// when the project has no commits yet. A path that cannot be pinned is an
/// error: the caller named a file that is not there.
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
            let path: ProjectPath = path.parse().map_err(anyhow::Error::msg)?;
            let identity = context_blob(vcs, &root, revision, &path)?
                .with_context(|| format!("cannot pin `{path}`: file not found"))?;
            Ok(ContextPin {
                path,
                identity,
                observed: false,
            })
        })
        .collect()
}

/// The identity of one context file: its content at `revision`, or in the
/// project working tree when there is no revision to read it from.
fn context_blob(
    vcs: &dyn Vcs,
    root: &std::path::Path,
    revision: Option<&str>,
    path: &ProjectPath,
) -> Result<Option<String>> {
    match revision {
        Some(revision) => vcs.file_blob_at(revision, path.as_str()),
        None => Ok(vcs
            .file_blob(path.as_str())?
            .or(project_file_blob(root, path)?)),
    }
}

/// The paired project repository's identity (its root commit), or an empty
/// string for an unpaired store — an unpaired store is supported.
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
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// An output commit's message: the caller's message plus the `Linka-Node`
/// trailer and, when the work had an input commit, the `Linka-Input` trailer
/// recording what it was built from.
fn output_commit_message(id: &NodeId, message: String, input: Option<&str>) -> String {
    let mut commit_message = format!("{message}\n\nLinka-Node: {id}");
    if let Some(input) = input {
        commit_message.push_str(&format!("\nLinka-Input: {input}"));
    }
    commit_message
}
