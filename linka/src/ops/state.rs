//! The derived state of a node, recomputed from its files and never stored.
//!
//! [`node_state`] is the single fallible evaluation; staleness, blockers,
//! readiness and the ready-work listing are all projections over it.

use super::*;

/// Per-operation derived-state cache. A view is discarded after each query and
/// is never used as authority for a mutation.
pub struct GraphView<'a> {
    store: &'a Store,
    vcs: &'a dyn Vcs,
    cached: std::cell::RefCell<std::collections::HashMap<NodeId, NodeState>>,
    visiting: std::cell::RefCell<std::collections::HashSet<NodeId>>,
}

impl<'a> GraphView<'a> {
    pub fn new(store: &'a Store, vcs: &'a dyn Vcs) -> Self {
        Self {
            store,
            vcs,
            cached: Default::default(),
            visiting: Default::default(),
        }
    }
    pub fn node_state(&self, id: &NodeId) -> Result<NodeState> {
        if let Some(state) = self.cached.borrow().get(id).cloned() {
            return Ok(state);
        }
        if !self.visiting.borrow_mut().insert(id.clone()) {
            bail!("dependency cycle while deriving state at `{id}");
        }
        let result = node_state_inner(self, id);
        self.visiting.borrow_mut().remove(id);
        if let Ok(state) = &result {
            self.cached.borrow_mut().insert(id.clone(), state.clone());
        }
        result
    }
}

/// Derive all graph state through one fallible evaluation.
pub fn node_state(store: &Store, vcs: &dyn Vcs, id: &NodeId) -> Result<NodeState> {
    GraphView::new(store, vcs).node_state(id)
}

fn node_state_inner(view: &GraphView<'_>, id: &NodeId) -> Result<NodeState> {
    let store = view.store;
    let vcs = view.vcs;
    let definition = store
        .load_definition(id)
        .with_context(|| format!("reading definition for `{id}`"))?;
    let meta = definition.meta;
    let result = (|| {
        let result = store.load_result(id)?;
        let (outcome, integration, staleness) = match result.as_ref() {
            None => (
                RecordedOutcome::Open,
                IntegrationStatus::NotRequired,
                Vec::new(),
            ),
            Some(result) => {
                if !outcome_kind_matches(meta.verifies.is_some(), result.meta.outcome) {
                    if meta.verifies.is_some() {
                        bail!("verification node `{id}` has a work outcome");
                    }
                    bail!("ordinary node `{id}` has a verification outcome");
                }
                let outcome = RecordedOutcome::from(result.meta.outcome);
                let candidate = candidate_for_result(store, id, &result.meta, &result.version)?;
                (
                    outcome,
                    candidate
                        .as_ref()
                        .map(|candidate| candidate.integration(vcs))
                        .transpose()?
                        .unwrap_or(IntegrationStatus::NotRequired),
                    staleness_for_result(
                        store,
                        vcs,
                        id,
                        &result.meta,
                        &result.version,
                        candidate.as_ref(),
                    )?,
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
            let dependency_state = view.node_state(dependency)?;
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
    result
}

fn staleness_for_result(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    result: &ResultMeta,
    result_version: &ResultVersion,
    candidate: Option<&CandidateRecord>,
) -> Result<Vec<StalenessReason>> {
    let mut reasons = Vec::new();
    let current = store.load_definition(id)?.version;
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
        if store.load_definition(&consumed.id)?.version != consumed.definition {
            reasons.push(StalenessReason::ConsumedDefinitionChanged {
                id: consumed.id.clone(),
            });
        }
        let current_result = store.load_result(&consumed.id)?;
        let current_version = current_result.as_ref().map(|loaded| loaded.version.clone());
        if current_version != consumed.result {
            reasons.push(StalenessReason::ConsumedResultChanged {
                id: consumed.id.clone(),
            });
        }
        let current_output = current_result.and_then(|loaded| loaded.meta.output);
        if current_output != consumed.output {
            reasons.push(StalenessReason::ConsumedOutputChanged {
                id: consumed.id.clone(),
            });
        }
    }
    let root = store.project_root();
    let observations = store.read_context_observations(id)?;
    let observed_context = observations
        .iter()
        .filter(|observation| observation.result == *result_version)
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
    version: &ResultVersion,
) -> Result<Option<CandidateRecord>> {
    let Some(artifact) = &result.output else {
        return Ok(None);
    };
    CandidateStore::new(store).for_result(id, version, artifact)
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
    let view = GraphView::new(store, vcs);
    let mut ready = Vec::new();
    for id in store.list_ids()? {
        if !view.node_state(&id)?.is_ready() {
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
