//! Integrity checking, fsck-style: the problems write-time validation cannot
//! see because they entered sideways (hand edits, merges, other writers).

use super::*;

/// Integrity-check the whole store, fsck-style: every problem that write-time
/// validation cannot see because it entered sideways (hand edits, git merges of
/// individually-valid branches, or unsupported writers). Returns explicit problem reports;
/// empty means the store is consistent. Read-only and git-free.
///
/// Checked per node: definition and result files parse; dependency lists hold no
/// duplicates or self-references; every edge target exists; and `depends_on`
/// contains no cycles (which would deadlock readiness — every node in the
/// cycle waiting on another).
pub fn check(store: &Store) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let repository = Pairing::load(store.root())?.map(|pairing| pairing.root_commit);
    let mut depends_on: std::collections::BTreeMap<NodeId, Vec<NodeId>> = Default::default();

    for id in store.list_ids()? {
        let meta = match store.read_node(&id) {
            Ok((meta, _)) => meta,
            Err(e) => {
                problems.push(format!("{id}: unreadable definition ({e:#})"));
                continue;
            }
        };
        if meta.schema != DEFINITION_SCHEMA {
            problems.push(format!(
                "{id}: unsupported definition schema {}",
                meta.schema
            ));
        }
        match store.read_result(&id) {
            Err(e) => problems.push(format!("{id}: unreadable result ({e:#})")),
            Ok(Some((result, _))) => {
                validate_result_semantics(
                    &id,
                    &meta,
                    &result,
                    repository.as_deref(),
                    &mut problems,
                );
                validate_verification_decision(store, &id, &meta, &result, &mut problems);
            }
            Ok(None) => {}
        }
        match store.read_context_observations(&id) {
            Err(error) => {
                problems.push(format!("{id}: unreadable context observation ({error:#})"))
            }
            Ok(observations) => {
                for observation in observations {
                    if observation.schema != OBSERVATION_SCHEMA {
                        problems.push(format!(
                            "{id}: unsupported context observation schema {}",
                            observation.schema
                        ));
                    }
                }
            }
        }
        if let Err(error) = store.list_node_attachments(&id) {
            problems.push(format!("{id}: unreadable attachment ({error:#})"));
        }
        for (kind, list) in [
            ("depends_on", &meta.depends_on),
            ("derived_from", &meta.derived_from),
        ] {
            let mut seen = std::collections::HashSet::new();
            for dep in list {
                if !seen.insert(dep.as_str()) {
                    problems.push(format!("{id}: duplicate {kind} entry `{dep}`"));
                }
                if *dep == id {
                    problems.push(format!("{id}: {kind} refers to the node itself"));
                    continue;
                }
                if store.read_node(dep).is_err() {
                    problems.push(format!("{id}: {kind} target `{dep}` missing or unreadable"));
                }
            }
        }
        if let Some(candidate_id) = &meta.verifies {
            match CandidateStore::new(store).load(candidate_id) {
                Err(error) => problems.push(format!(
                    "{id}: verifies candidate `{candidate_id}` missing or unreadable ({error:#})"
                )),
                Ok(candidate) => {
                    if !meta.derived_from.contains(&candidate.node) {
                        problems.push(format!(
                            "{id}: verifies candidate `{candidate_id}` but does not derive from its source node `{}`",
                            candidate.node
                        ));
                    }
                }
            }
        }
        depends_on.insert(id, meta.depends_on);
    }

    problems.extend(find_cycles(&depends_on));
    Ok(problems)
}

fn validate_result_semantics(
    id: &NodeId,
    meta: &NodeMeta,
    result: &ResultMeta,
    repository: Option<&str>,
    problems: &mut Vec<String>,
) {
    if result.schema != RESULT_SCHEMA {
        problems.push(format!("{id}: unsupported result schema {}", result.schema));
    }
    if !outcome_kind_matches(meta.verifies.is_some(), result.outcome) {
        if meta.verifies.is_some() {
            problems.push(format!("{id}: verification node has a work outcome"));
        } else {
            problems.push(format!("{id}: ordinary node has a verification outcome"));
        }
    }
    if meta.verifies.is_some() && result.output.is_some() {
        problems.push(format!("{id}: verification result declares project output"));
    }
    let mut seen = std::collections::HashSet::new();
    for pin in &result.consumed {
        if !seen.insert(pin.id.as_str()) {
            problems.push(format!("{id}: duplicate consumed-node pin `{}`", pin.id));
        }
        let required = meta.depends_on.contains(&pin.id);
        let lineage = meta.derived_from.contains(&pin.id);
        if !required && !lineage {
            problems.push(format!(
                "{id}: consumed pin `{}` has no declared edge",
                pin.id
            ));
        }
        if required
            && outcome_requires_full_pins(result.outcome)
            && (pin.result.is_none() || !pin.outcome.is_some_and(result_satisfies_dependency))
        {
            problems.push(format!(
                "{id}: successful result has no successful evidence for required dependency `{}`",
                pin.id
            ));
        }
        if let Some(output) = &pin.output {
            validate_artifact(id, output, repository, problems);
        }
    }
    if outcome_requires_full_pins(result.outcome) {
        for edge in meta.depends_on.iter().chain(&meta.derived_from) {
            if !result.consumed.iter().any(|pin| &pin.id == edge) {
                problems.push(format!(
                    "{id}: successful result is missing pin for `{edge}`"
                ));
            }
        }
    }
    let mut context = std::collections::HashSet::new();
    for pin in &result.context {
        if !context.insert(pin.path.as_str()) {
            problems.push(format!("{id}: duplicate context pin `{}`", pin.path));
        }
    }
    if let Some(output) = &result.output {
        validate_artifact(id, output, repository, problems);
    }
}

fn validate_verification_decision(
    store: &Store,
    id: &NodeId,
    meta: &NodeMeta,
    result: &ResultMeta,
    problems: &mut Vec<String>,
) {
    let Some(candidate_id) = &meta.verifies else {
        return;
    };
    let Ok(candidate) = CandidateStore::new(store).load(candidate_id) else {
        return;
    };
    let decided_by_this = match &candidate.state {
        CandidateState::Accepted { verification, .. }
        | CandidateState::Rejected { verification, .. } => verification == id,
        CandidateState::Pending => false,
    };
    let matches = match result.outcome {
        ResultOutcome::Verification(VerificationOutcome::Accepted) => matches!(
            &candidate.state,
            CandidateState::Accepted { verification, .. } if verification == id
        ),
        ResultOutcome::Verification(VerificationOutcome::Rejected) => matches!(
            &candidate.state,
            CandidateState::Rejected { verification, .. } if verification == id
        ),
        ResultOutcome::Verification(VerificationOutcome::Abandoned) => !decided_by_this,
        ResultOutcome::Work(_) => true,
    };
    if !matches {
        problems.push(format!(
            "{id}: verification outcome {} disagrees with candidate `{candidate_id}` decision",
            result.outcome.as_str()
        ));
    }
}

fn validate_artifact(
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
    if let Some(expected) = repository {
        if artifact.repository != expected {
            problems.push(format!("{id}: artifact belongs to a different repository"));
        }
    }
}

pub fn check_artifacts(store: &Store, vcs: &dyn Vcs) -> Result<Vec<String>> {
    let mut problems = check_workbench(store, vcs)?;
    for id in store.list_ids()? {
        if let Some((result, _)) = store.read_result(&id)? {
            if let Some(artifact) = &result.output {
                if artifact.scheme == "git-commit" {
                    let reference = format!("refs/linka/outputs/{id}");
                    match vcs.ref_commit(&reference)? {
                        None => problems.push(format!(
                            "{id}: output retention ref is missing for artifact {}",
                            artifact.id
                        )),
                        Some(retained) if retained != artifact.id => problems.push(format!(
                            "{id}: output retention ref points to {retained}, expected {}",
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
    }
    Ok(problems)
}

/// Check both the store's structure and whether its on-disk state is fully
/// recorded in workbench history. The latter catches interrupted or partial
/// mutations that can still leave individually valid files behind.
pub fn check_workbench(store: &Store, vcs: &dyn Vcs) -> Result<Vec<String>> {
    let mut problems = check(store)?;
    if let Err(error) = vcs.require_clean_store(&store.store_name()) {
        problems.push(format!("store has uncommitted changes: {error:#}"));
    }
    Ok(problems)
}

/// Report each `depends_on` cycle once, as an explicit `a -> b -> a` path.
fn find_cycles(graph: &std::collections::BTreeMap<NodeId, Vec<NodeId>>) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Visiting,
        Done,
    }
    fn visit(
        node: &NodeId,
        graph: &std::collections::BTreeMap<NodeId, Vec<NodeId>>,
        state: &mut std::collections::HashMap<NodeId, State>,
        stack: &mut Vec<NodeId>,
        out: &mut Vec<String>,
    ) {
        match state.get(node) {
            Some(State::Done) => return,
            Some(State::Visiting) => {
                // Back-edge: the cycle is the stack from the first occurrence on.
                let start = stack.iter().position(|n| n == node).unwrap_or(0);
                let mut path: Vec<&str> = stack[start..].iter().map(NodeId::as_str).collect();
                path.push(node.as_str());
                out.push(format!("dependency cycle: {}", path.join(" -> ")));
                return;
            }
            None => {}
        }
        state.insert(node.clone(), State::Visiting);
        stack.push(node.clone());
        for dep in graph.get(node).into_iter().flatten() {
            // Missing targets are reported separately; only follow known nodes.
            if graph.contains_key(dep) {
                visit(dep, graph, state, stack, out);
            }
        }
        stack.pop();
        state.insert(node.clone(), State::Done);
    }

    let mut state = std::collections::HashMap::new();
    let mut out = Vec::new();
    for node in graph.keys() {
        visit(node, graph, &mut state, &mut Vec::new(), &mut out);
    }
    out
}
