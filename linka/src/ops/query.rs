//! Read-only graph queries derived by scanning the store rather than by
//! maintaining a second index.

use super::*;

/// The node whose work produced `commit`, if any — the inverse of the output
/// artifact on each result, derived by scanning rather than persisted as a
/// second index. Unique because each completion mints one commit for one node.
pub fn origin(store: &Store, commit: &str) -> Result<Option<NodeId>> {
    for id in store.list_ids()? {
        if let Some((result, _)) = store.read_result(&id)? {
            if result.output.as_ref().map(|a| a.id.as_str()) == Some(commit) {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}

/// A node's current output commit: what its recorded work produced. `None` if it
/// has no result or the work produced no files.
pub fn output_of(store: &Store, id: &NodeId) -> Result<Option<String>> {
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }
    Ok(store
        .read_result(id)?
        .and_then(|(result, _)| result.output.map(|artifact| artifact.id)))
}

/// Ids of nodes that name `id` in either dependency list.
pub fn dependents(store: &Store, id: &NodeId) -> Result<Vec<NodeId>> {
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }
    let mut out = Vec::new();
    for other in store.list_ids()? {
        if &other == id {
            continue;
        }
        let (meta, _) = store.read_node(&other)?;
        if meta
            .depends_on
            .iter()
            .chain(&meta.derived_from)
            .any(|d| d == id)
        {
            out.push(other);
        }
    }
    Ok(out)
}

/// Ids of ordinary nodes that verify `candidate`.
pub fn verifications_for(store: &Store, candidate: &CandidateId) -> Result<Vec<NodeId>> {
    CandidateStore::new(store).load(candidate)?;
    let mut out = Vec::new();
    for id in store.list_ids()? {
        let (meta, _) = store.read_node(&id)?;
        if meta.verifies.as_ref() == Some(candidate) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Reasons a node is not *settled* — done, not stale, and with every piece of
/// work derived from it (transitively, over reverse `depends_on` and
/// `derived_from` edges) also done and not stale. Empty means the whole branch
/// of work rooted at this node is finished and still valid.
///
/// This answers "is this actually finished?" for a node whose own `done` only
/// certifies its own unit of work — e.g. a task that closed at spec time while
/// its implementations were still open.
pub fn unsettled(store: &Store, vcs: &dyn Vcs, id: &NodeId) -> Result<Vec<String>> {
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }
    // Reverse adjacency over both edge kinds, built in one scan.
    let mut rev: std::collections::BTreeMap<NodeId, Vec<NodeId>> = Default::default();
    for other in store.list_ids()? {
        let (meta, _) = store.read_node(&other)?;
        for dep in meta.depends_on.iter().chain(&meta.derived_from) {
            rev.entry(dep.clone()).or_default().push(other.clone());
        }
    }

    let mut reasons = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([id.clone()]);
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        let state = node_state(store, vcs, &node)?;
        if !state.is_complete() {
            if state.is_awaiting_integration() {
                reasons.push(format!("{node}: awaiting candidate integration"));
            } else if state.outcome == RecordedOutcome::Succeeded {
                reasons.push(format!("{node}: done but stale"));
            } else {
                let outcome = match state.outcome {
                    RecordedOutcome::Open => "open",
                    RecordedOutcome::Failed => "failed",
                    RecordedOutcome::Succeeded => unreachable!(),
                    RecordedOutcome::Accepted => "accepted but stale",
                    RecordedOutcome::Rejected => "rejected but stale",
                    RecordedOutcome::Abandoned => "abandoned but stale",
                };
                reasons.push(format!("{node}: not done ({outcome})"));
            }
        }
        for dependent in rev.get(&node).into_iter().flatten() {
            queue.push_back(dependent.clone());
        }
    }
    Ok(reasons)
}
