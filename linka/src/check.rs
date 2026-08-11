//! Integrity checking, fsck-style: the problems write-time validation cannot
//! see because they entered sideways — hand edits, merges of individually
//! valid branches, or unsupported writers.
//!
//! [`check`] is read-only and git-free, and never stops at the first problem.

use anyhow::{Context, Result};
use std::collections::HashSet;

use crate::graph::Graph;
use crate::model::{
    ArtifactRef, CandidateId, NodeId, NodeMeta, Outcome, ResultMeta, DEFINITION_SCHEMA,
    RESULT_SCHEMA,
};
use crate::pairing::Pairing;
use crate::store::Store;
use crate::vcs::{OfflineVcs, Vcs};

/// Integrity-check the whole store. Returns explicit problem reports; empty
/// means the store is consistent.
pub fn check(store: &Store) -> Result<Vec<String>> {
    let offline = OfflineVcs;
    let graph = Graph::load(store, &offline)?;
    let mut problems = graph.discovery_problems().to_vec();
    let repository = Pairing::load(store.root())?.map(|pairing| pairing.root_commit);

    for id in store.node_ids()? {
        let Some(meta) = graph.meta(&id) else {
            // The node's own records are why it cannot be evaluated; the state
            // carries the reason.
            problems.push(format!(
                "{id}: {}",
                graph.state(&id).error().unwrap_or("unreadable records")
            ));
            continue;
        };
        if meta.schema != DEFINITION_SCHEMA {
            problems.push(format!(
                "{id}: unsupported definition schema {}",
                meta.schema
            ));
        }
        check_edges(store, &id, meta, &mut problems);
        check_verifies(&graph, &id, meta, &mut problems);
        if let Some(result) = graph.result(&id) {
            check_result(&id, meta, result, repository.as_deref(), &mut problems);
        }
        if let Err(error) = store.list_attachments(&id) {
            problems.push(format!("{id}: unreadable attachment ({error:#})"));
        }
        if let Some(observed) = store
            .read_observed_context(&id)
            .with_context(|| format!("reading observed context for `{id}`"))
            .unwrap_or(None)
        {
            let current = store.current_result_version(&id)?;
            if current.as_ref() != Some(&observed.result) {
                problems.push(format!(
                    "{id}: observed context belongs to a result the node no longer has"
                ));
            }
        }
    }

    for candidate in graph.candidates() {
        if !store.exists(&candidate.node) {
            problems.push(format!(
                "{}: source node `{}` is missing",
                candidate.id, candidate.node
            ));
        }
        if let Err(problem) = graph.decision(&candidate.id) {
            problems.push(problem);
        }
    }

    problems.extend(graph.cycle_reports());
    problems.sort();
    problems.dedup();
    Ok(problems)
}

fn check_edges(store: &Store, id: &NodeId, meta: &NodeMeta, problems: &mut Vec<String>) {
    for (kind, list) in [
        ("depends_on", &meta.depends_on),
        ("derived_from", &meta.derived_from),
    ] {
        let mut seen = HashSet::new();
        for edge in list {
            if !seen.insert(edge.as_str()) {
                problems.push(format!("{id}: duplicate {kind} entry `{edge}`"));
            }
            if edge == id {
                problems.push(format!("{id}: {kind} refers to the node itself"));
                continue;
            }
            if !store.exists(edge) {
                problems.push(format!("{id}: {kind} target `{edge}` is missing"));
            }
        }
    }
}

fn check_verifies(graph: &Graph, id: &NodeId, meta: &NodeMeta, problems: &mut Vec<String>) {
    let Some(candidate_id) = &meta.verifies else {
        return;
    };
    let Some(candidate) = graph.candidate(candidate_id) else {
        problems.push(format!(
            "{id}: verifies candidate `{candidate_id}` which is missing or unreadable"
        ));
        return;
    };
    if !meta.derived_from.contains(&candidate.node) {
        problems.push(format!(
            "{id}: verifies candidate `{candidate_id}` but does not derive from its source node `{}`",
            candidate.node
        ));
    }
}

fn check_result(
    id: &NodeId,
    meta: &NodeMeta,
    result: &ResultMeta,
    repository: Option<&str>,
    problems: &mut Vec<String>,
) {
    if result.schema != RESULT_SCHEMA {
        problems.push(format!("{id}: unsupported result schema {}", result.schema));
    }
    if !result.outcome.suits(meta.is_verification()) {
        problems.push(if meta.is_verification() {
            format!(
                "{id}: review node records the work outcome `{}`",
                result.outcome.as_str()
            )
        } else {
            format!(
                "{id}: ordinary node records the verification outcome `{}`",
                result.outcome.as_str()
            )
        });
    }
    if meta.is_verification() && result.output.is_some() {
        problems.push(format!("{id}: verification result declares project output"));
    }

    let mut seen = HashSet::new();
    for pin in &result.consumed {
        if !seen.insert(pin.id.as_str()) {
            problems.push(format!("{id}: duplicate consumed-node pin `{}`", pin.id));
        }
        let required = meta.depends_on.contains(&pin.id);
        if !required && !meta.derived_from.contains(&pin.id) {
            problems.push(format!(
                "{id}: consumed pin `{}` has no declared edge",
                pin.id
            ));
        }
        if required
            && result.outcome.requires_full_pins()
            && (pin.result.is_none() || !pin.outcome.is_some_and(Outcome::satisfies_dependency))
        {
            problems.push(format!(
                "{id}: result has no successful evidence for required dependency `{}`",
                pin.id
            ));
        }
        if let Some(output) = &pin.output {
            check_artifact(id, output, repository, problems);
        }
    }
    if result.outcome.requires_full_pins() {
        for edge in meta.depends_on.iter().chain(&meta.derived_from) {
            if !result.consumed.iter().any(|pin| pin.id == *edge) {
                problems.push(format!("{id}: result is missing the pin for `{edge}`"));
            }
        }
    }

    let mut context = HashSet::new();
    for pin in &result.context {
        if !context.insert(pin.path.as_str()) {
            problems.push(format!("{id}: duplicate context pin `{}`", pin.path));
        }
    }
    if let Some(output) = &result.output {
        check_artifact(id, output, repository, problems);
    }
}

fn check_artifact(
    id: &NodeId,
    artifact: &ArtifactRef,
    repository: Option<&str>,
    problems: &mut Vec<String>,
) {
    if artifact.scheme != "git-commit" {
        problems.push(format!(
            "{id}: unsupported artifact scheme `{}`",
            artifact.scheme
        ));
    }
    if artifact.repository.len() != 40
        || !artifact
            .repository
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        problems.push(format!(
            "{id}: invalid artifact repository identity `{}`",
            artifact.repository
        ));
    }
    // An unpaired store is a supported configuration and must not fail its own
    // integrity check, so the repository is only compared when it is recorded.
    if let Some(expected) = repository {
        if artifact.repository != expected {
            problems.push(format!("{id}: artifact belongs to a different repository"));
        }
    }
}

/// The structural check plus everything only a repository can answer: that
/// recorded output commits exist, and that each result's output retention ref
/// still points at its artifact.
pub fn check_artifacts(store: &Store, vcs: &dyn Vcs) -> Result<Vec<String>> {
    let mut problems = check_workbench(store, vcs)?;
    for id in store.node_ids()? {
        let Some((result, _)) = store.read_result(&id)? else {
            continue;
        };
        if let Some(artifact) = &result.output {
            if artifact.scheme == "git-commit" {
                let reference = format!("refs/linka/outputs/{id}");
                match vcs.ref_commit(&reference)? {
                    None => problems.push(format!(
                        "{id}: output retention ref is missing for artifact {}",
                        artifact.id
                    )),
                    Some(retained) if retained != artifact.id => problems.push(format!(
                        "{id}: output retention ref points at {retained}, expected {}",
                        artifact.id
                    )),
                    Some(_) => {}
                }
            }
        }
        for artifact in result
            .output
            .iter()
            .chain(result.consumed.iter().filter_map(|pin| pin.output.as_ref()))
        {
            if artifact.scheme == "git-commit" && !vcs.commit_exists(&artifact.id)? {
                problems.push(format!("{id}: artifact {} is not retained", artifact.id));
            }
        }
    }
    Ok(problems)
}

/// The structural check plus the question git can answer about the store
/// itself: whether its on-disk state is fully recorded in workbench history,
/// catching interrupted or partial mutations that leave valid files behind.
pub fn check_workbench(store: &Store, vcs: &dyn Vcs) -> Result<Vec<String>> {
    let mut problems = check(store)?;
    if let Err(error) = vcs.require_clean_store(&store.store_name()) {
        problems.push(format!("store has uncommitted changes: {error:#}"));
    }
    Ok(problems)
}

/// Verify the store↔project pairing. Read-only and manual — nothing calls it
/// implicitly. Returns the recorded pairing (`None` means the store is not
/// paired, which is a notice rather than a problem) and the problems found.
///
/// The default check is one comparison of the actual root against the recorded
/// root. With `deep`, every hash the store points at is checked to exist,
/// catching a partial rewrite that leaves the root intact but orphans recorded
/// outputs.
pub fn verify_pairing(
    store: &Store,
    vcs: &dyn Vcs,
    deep: bool,
) -> Result<(Option<Pairing>, Vec<String>)> {
    let Some(pairing) = Pairing::load(store.root())? else {
        return Ok((None, Vec::new()));
    };
    let mut problems = Vec::new();
    match vcs.root_commit()? {
        None => problems.push(format!(
            "project repository has no commits, but the store is paired to root {}",
            crate::ops::short(&pairing.root_commit)
        )),
        Some(actual) if actual != pairing.root_commit => problems.push(format!(
            "project root commit is {} but the store is paired to {} — \
             wrong project in the workbench, or a rewritten history \
             (`linka pair --force` re-pairs deliberately)",
            crate::ops::short(&actual),
            crate::ops::short(&pairing.root_commit)
        )),
        Some(_) => {}
    }
    if deep {
        for id in store.node_ids()? {
            let Some((result, _)) = store.read_result(&id)? else {
                continue;
            };
            if let Some(output) = &result.output {
                if !vcs.commit_exists(&output.id)? {
                    problems.push(format!(
                        "{id}: output commit {} does not exist in the project repository",
                        crate::ops::short(&output.id)
                    ));
                }
            }
            for consumed in &result.consumed {
                if let Some(output) = &consumed.output {
                    if !vcs.commit_exists(&output.id)? {
                        problems.push(format!(
                            "{id}: built-against output {} (of {}) does not exist in the project repository",
                            crate::ops::short(&output.id),
                            consumed.id
                        ));
                    }
                }
            }
        }
    }
    Ok((Some(pairing), problems))
}

/// Ids of the review nodes that name a candidate.
pub fn verifications_for(store: &Store, candidate: &CandidateId) -> Result<Vec<NodeId>> {
    store.read_candidate(candidate)?;
    let mut nodes = Vec::new();
    for id in store.node_ids()? {
        let (meta, _) = store.read_node(&id)?;
        if meta.verifies.as_ref() == Some(candidate) {
            nodes.push(id);
        }
    }
    Ok(nodes)
}
