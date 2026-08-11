//! The derived state of a node, recomputed from its files and never stored.
//!
//! [`node_state`] is the single fallible evaluation; staleness, blockers,
//! readiness and the ready-work listing are all projections over it.

use super::*;

/// Derive all graph state through one fallible evaluation.
pub fn node_state(store: &Store, vcs: &dyn Vcs, id: &NodeId) -> Result<NodeState> {
    let mut visiting = std::collections::HashSet::new();
    node_state_inner(store, vcs, id, &mut visiting)
}

fn node_state_inner(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    visiting: &mut std::collections::HashSet<NodeId>,
) -> Result<NodeState> {
    let (meta, _) = store
        .read_node(id)
        .with_context(|| format!("reading definition for `{id}`"))?;
    if !visiting.insert(id.clone()) {
        bail!("dependency cycle while deriving state at `{id}`");
    }
    let result = (|| {
        let result = store.read_result(id)?;
        let (outcome, integration, staleness) = match result.as_ref() {
            None => (
                RecordedOutcome::Open,
                IntegrationStatus::NotRequired,
                Vec::new(),
            ),
            Some((result, _)) => {
                if !outcome_kind_matches(meta.verifies.is_some(), result.outcome) {
                    if meta.verifies.is_some() {
                        bail!("verification node `{id}` has a work outcome");
                    }
                    bail!("ordinary node `{id}` has a verification outcome");
                }
                let outcome = RecordedOutcome::from(result.outcome);
                let candidate = candidate_for_result(store, id, result)?;
                (
                    outcome,
                    candidate
                        .as_ref()
                        .map(|candidate| candidate.integration(vcs))
                        .transpose()?
                        .unwrap_or(IntegrationStatus::NotRequired),
                    staleness_for_result(store, vcs, id, result, candidate.as_ref())?,
                )
            }
        };
        let currency = if staleness.is_empty() {
            Currency::Current
        } else {
            Currency::Stale
        };
        let mut blockers = Vec::new();
        for dependency in &meta.depends_on {
            if !store.exists(dependency) {
                blockers.push(Blocker {
                    id: dependency.clone(),
                    reason: BlockerReason::Missing,
                });
                continue;
            }
            let dependency_state = node_state_inner(store, vcs, dependency, visiting)?;
            if !dependency_state.is_complete()
                || matches!(
                    dependency_state.outcome,
                    RecordedOutcome::Rejected | RecordedOutcome::Abandoned
                )
            {
                let reason = if dependency_state.currency == Currency::Stale {
                    BlockerReason::Stale
                } else {
                    match dependency_state.outcome {
                        RecordedOutcome::Open => BlockerReason::Open,
                        RecordedOutcome::Failed => BlockerReason::Failed,
                        RecordedOutcome::Rejected => BlockerReason::Rejected,
                        RecordedOutcome::Abandoned => BlockerReason::Abandoned,
                        RecordedOutcome::Succeeded => BlockerReason::AwaitingIntegration,
                        RecordedOutcome::Accepted => unreachable!(),
                    }
                };
                blockers.push(Blocker {
                    id: dependency.clone(),
                    reason,
                });
            }
        }
        Ok(NodeState {
            outcome,
            currency,
            integration,
            staleness,
            blockers,
        })
    })();
    visiting.remove(id);
    result
}

fn staleness_for_result(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    result: &ResultMeta,
    candidate: Option<&CandidateRecord>,
) -> Result<Vec<StalenessReason>> {
    let mut reasons = Vec::new();
    let current = store.node_version(id)?;
    if current != result.definition {
        reasons.push(StalenessReason::DefinitionChanged {
            metadata: current.metadata != result.definition.metadata,
            description: current.description != result.definition.description,
        });
    }
    for consumed in &result.consumed {
        if !store.exists(&consumed.id) {
            reasons.push(StalenessReason::ConsumedNodeMissing {
                id: consumed.id.clone(),
            });
            continue;
        }
        if store.node_version(&consumed.id)? != consumed.definition {
            reasons.push(StalenessReason::ConsumedDefinitionChanged {
                id: consumed.id.clone(),
            });
        }
        let current_result = store.read_result(&consumed.id)?;
        let current_version = current_result
            .is_some()
            .then(|| store.result_version(&consumed.id))
            .transpose()?;
        if current_version != consumed.result {
            reasons.push(StalenessReason::ConsumedResultChanged {
                id: consumed.id.clone(),
            });
        }
        let current_output = current_result.and_then(|(r, _)| r.output);
        if current_output != consumed.output {
            reasons.push(StalenessReason::ConsumedOutputChanged {
                id: consumed.id.clone(),
            });
        }
    }
    let root = store.project_root();
    let result_version = store.result_version(id)?;
    let observations = store.read_context_observations(id)?;
    let observed_context = observations
        .iter()
        .filter(|observation| observation.result == result_version)
        .flat_map(|observation| observation.context.iter());
    for pin in result.context.iter().chain(observed_context) {
        let current = project_file_blob(&root, &pin.path)?;
        match current {
            Some(now) if now != pin.identity => reasons.push(StalenessReason::ContextChanged {
                path: pin.path.clone(),
            }),
            None => reasons.push(StalenessReason::ContextMissing {
                path: pin.path.clone(),
            }),
            _ => {}
        }
    }
    if let Some(output) = &result.output {
        let detail = if let Some(candidate) = candidate {
            if candidate.integration(vcs)? == IntegrationStatus::Published {
                let target_ref = format!("refs/heads/{}", candidate.target);
                let target = vcs
                    .ref_commit(&target_ref)?
                    .with_context(|| format!("published target `{target_ref}` is missing"))?;
                vcs.drift(&output.id, Some(&target))?
            } else {
                None
            }
        } else {
            vcs.drift(&output.id, None)?
        };
        if let Some(detail) = detail {
            reasons.push(StalenessReason::OutputDrifted {
                artifact: output.id.clone(),
                detail,
            });
        }
    }
    Ok(reasons)
}

fn candidate_for_result(
    store: &Store,
    id: &NodeId,
    result: &ResultMeta,
) -> Result<Option<CandidateRecord>> {
    let Some(artifact) = &result.output else {
        return Ok(None);
    };
    let version = store.result_version(id)?;
    CandidateStore::new(store).for_result(id, &version, artifact)
}

pub fn staleness(store: &Store, vcs: &dyn Vcs, id: &NodeId) -> Result<Vec<StalenessReason>> {
    Ok(node_state(store, vcs, id)?.staleness)
}

pub fn blockers(store: &Store, vcs: &dyn Vcs, id: &NodeId) -> Result<Vec<Blocker>> {
    Ok(node_state(store, vcs, id)?.blockers)
}

pub fn is_ready(store: &Store, vcs: &dyn Vcs, id: &NodeId) -> Result<bool> {
    Ok(node_state(store, vcs, id)?.is_ready())
}

pub fn ready_nodes(store: &Store, vcs: &dyn Vcs, worker: Option<Author>) -> Result<Vec<NodeId>> {
    let mut ready = Vec::new();
    for id in store.list_ids()? {
        if !node_state(store, vcs, &id)?.is_ready() {
            continue;
        }
        let (meta, _) = store.read_node(&id)?;
        if matches!((worker, meta.assignee), (Some(want), Some(has)) if want != has) {
            continue;
        }
        ready.push(id);
    }
    Ok(ready)
}
