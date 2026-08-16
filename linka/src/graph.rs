//! One memoized evaluation pass over the whole store.
//!
//! Derived state is a fold over records: nothing here writes, and nothing
//! caches a status beyond the lifetime of one [`Graph`]. Loading scans the
//! nodes and candidates once, evaluation walks `depends_on` iteratively with a
//! cache keyed by node id, and the version-control seam is wrapped in a
//! per-pass memo — so a diamond dependency evaluates its shared ancestor once,
//! deep chains cannot overflow the stack, and a hundred nodes sharing one
//! target branch cost one `git` call, not a hundred.
//!
//! A record that cannot be read, parsed, or reconciled makes *that* node
//! [`NodeState::Error`]. The rest of the graph stays queryable.

use anyhow::Result;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::model::{
    Author, Blocker, BlockerReason, Candidate, CandidateDecision, CandidateId, Currency,
    DefinitionVersion, IntegrationStatus, NodeId, NodeMeta, NodeState, ObservedContext, Outcome,
    ProjectPath, RecordedOutcome, ResultMeta, ResultVersion, StalenessReason, Unsettled,
    UnsettledReason, Workability,
};
use crate::store::{file_blob, Store};
use crate::vcs::{MemoizingVcs, Vcs};

/// Everything one node's own files say, read once.
struct NodeRecords {
    meta: NodeMeta,
    definition: DefinitionVersion,
    result: Option<(ResultMeta, ResultVersion)>,
    observed: Option<ObservedContext>,
}

pub struct Graph<'a> {
    store: &'a Store,
    vcs: MemoizingVcs<'a>,
    nodes: BTreeMap<NodeId, std::result::Result<NodeRecords, String>>,
    candidates: BTreeMap<CandidateId, Candidate>,
    /// Which candidates propose output for an exact node result.
    candidates_by_result: HashMap<(NodeId, ResultVersion), Vec<CandidateId>>,
    /// Which review nodes name each candidate.
    verifications: HashMap<CandidateId, Vec<NodeId>>,
    /// Nodes on a `depends_on` cycle, with the path to report.
    cycles: BTreeMap<NodeId, String>,
    /// Problems found while discovering records, for `check` to report.
    problems: Vec<String>,
    states: HashMap<NodeId, NodeState>,
    /// What the current verifications concluded, decided once per pass.
    decisions: RefCell<HashMap<CandidateId, std::result::Result<CandidateDecision, String>>>,
    unknown: NodeState,
}

impl<'a> Graph<'a> {
    /// Scan the store once and evaluate every node.
    pub fn load(store: &'a Store, vcs: &'a dyn Vcs) -> Result<Self> {
        let (ids, mut problems) = store.list_nodes()?;
        let (candidate_records, candidate_problems) = store.load_candidates()?;
        problems.extend(candidate_problems);

        let mut nodes = BTreeMap::new();
        for id in ids {
            nodes.insert(id.clone(), read_records(store, &id));
        }

        let mut candidates = BTreeMap::new();
        let mut candidates_by_result: HashMap<(NodeId, ResultVersion), Vec<CandidateId>> =
            HashMap::new();
        for candidate in candidate_records {
            candidates_by_result
                .entry((candidate.node.clone(), candidate.result.clone()))
                .or_default()
                .push(candidate.id.clone());
            candidates.insert(candidate.id.clone(), candidate);
        }

        let mut verifications: HashMap<CandidateId, Vec<NodeId>> = HashMap::new();
        for (id, records) in &nodes {
            if let Ok(records) = records {
                if let Some(candidate) = &records.meta.verifies {
                    verifications
                        .entry(candidate.clone())
                        .or_default()
                        .push(id.clone());
                }
            }
        }

        let cycles = dependency_cycles(&nodes);
        let mut graph = Self {
            store,
            vcs: MemoizingVcs::new(vcs),
            nodes,
            candidates,
            candidates_by_result,
            verifications,
            cycles,
            problems,
            states: HashMap::new(),
            decisions: RefCell::default(),
            unknown: NodeState::Error {
                message: "this store holds no such node".into(),
            },
        };
        graph.evaluate_all();
        Ok(graph)
    }

    /// The derived state of one node. An id the store does not hold is an
    /// error state like any other unreadable record, so a caller holding a
    /// stale id gets an answer rather than a failure.
    pub fn state(&self, id: &NodeId) -> &NodeState {
        self.states.get(id).unwrap_or(&self.unknown)
    }

    /// The derived state of one node, or `None` if this store has no such
    /// node — for callers that must tell "absent" from "unreadable".
    pub fn try_state(&self, id: &NodeId) -> Option<&NodeState> {
        self.states.get(id)
    }

    pub fn contains(&self, id: &NodeId) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &NodeId> {
        self.nodes.keys()
    }

    /// The node's definition, if its records could be read.
    pub fn meta(&self, id: &NodeId) -> Option<&NodeMeta> {
        self.nodes.get(id)?.as_ref().ok().map(|r| &r.meta)
    }

    /// The node's result, if it has one and its records could be read.
    pub fn result(&self, id: &NodeId) -> Option<&ResultMeta> {
        let records = self.nodes.get(id)?.as_ref().ok()?;
        records.result.as_ref().map(|(meta, _)| meta)
    }

    pub fn candidate(&self, id: &CandidateId) -> Option<&Candidate> {
        self.candidates.get(id)
    }

    pub fn candidates(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates.values()
    }

    /// The candidates proposing output for a node's current result.
    pub fn candidates_of(&self, id: &NodeId) -> Vec<&Candidate> {
        let Some(Ok(records)) = self.nodes.get(id) else {
            return Vec::new();
        };
        let Some((_, version)) = &records.result else {
            return Vec::new();
        };
        self.candidates_for_result(id, version)
    }

    /// Review nodes naming this candidate.
    pub fn verifications_of(&self, candidate: &CandidateId) -> &[NodeId] {
        self.verifications
            .get(candidate)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// What the current verifications concluded about a candidate; `Err` when
    /// two of them decided it differently.
    pub fn decision(
        &self,
        candidate: &CandidateId,
    ) -> std::result::Result<CandidateDecision, String> {
        self.candidate_decision(candidate)
    }

    /// Problems found while discovering records — bad directory names and
    /// unreadable candidate files, which listing reports rather than fails on.
    pub fn discovery_problems(&self) -> &[String] {
        &self.problems
    }

    /// Every `depends_on` cycle, reported once per node on it.
    pub fn cycle_reports(&self) -> Vec<String> {
        let mut reports: Vec<&str> = self.cycles.values().map(String::as_str).collect();
        reports.sort_unstable();
        reports.dedup();
        reports.into_iter().map(str::to_owned).collect()
    }

    // --- projections ----------------------------------------------------------

    /// Ready work, optionally restricted to what a given worker may take.
    pub fn ready(&self, worker: Option<Author>) -> Vec<&NodeId> {
        self.nodes
            .keys()
            .filter(|id| self.state(id).is_ready())
            .filter(
                |id| match (worker, self.meta(id).and_then(|meta| meta.assignee)) {
                    (Some(want), Some(has)) => want == has,
                    _ => true,
                },
            )
            .collect()
    }

    pub fn blocked(&self) -> Vec<(&NodeId, &[Blocker])> {
        self.nodes
            .keys()
            .map(|id| (id, self.state(id)))
            .filter(|(_, state)| state.workability() == Workability::Blocked)
            .map(|(id, state)| (id, state.blockers()))
            .collect()
    }

    pub fn stale(&self) -> Vec<(&NodeId, &[StalenessReason])> {
        self.nodes
            .keys()
            .map(|id| (id, self.state(id)))
            .filter(|(_, state)| state.currency() == Some(Currency::Stale))
            .map(|(id, state)| (id, state.staleness()))
            .collect()
    }

    /// Ids of nodes naming `id` in either edge list.
    pub fn dependents(&self, id: &NodeId) -> Vec<&NodeId> {
        self.nodes
            .iter()
            .filter(|(other, _)| *other != id)
            .filter(|(_, records)| {
                records.as_ref().is_ok_and(|records| {
                    records
                        .meta
                        .depends_on
                        .iter()
                        .chain(&records.meta.derived_from)
                        .any(|edge| edge == id)
                })
            })
            .map(|(other, _)| other)
            .collect()
    }

    /// The node whose work produced `commit`, derived by scanning rather than
    /// kept as a second index. Unique because each completion mints one commit
    /// for one node.
    pub fn origin(&self, commit: &str) -> Option<&NodeId> {
        self.nodes.iter().find_map(|(id, records)| {
            let result = &records.as_ref().ok()?.result.as_ref()?.0;
            (result.output.as_ref().map(|artifact| artifact.id.as_str()) == Some(commit))
                .then_some(id)
        })
    }

    /// Reasons this node's branch of work is not *settled*: it, or something
    /// derived from it (transitively, over reverse `depends_on` and
    /// `derived_from` edges), is not complete. Empty means the whole branch is
    /// finished and still valid.
    ///
    /// This answers "is this actually finished?" for a node whose own `done`
    /// only certifies its own unit of work — a task that closed at spec time
    /// while its implementations were still open, say.
    pub fn settled(&self, id: &NodeId) -> Vec<Unsettled> {
        let mut reverse: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
        for (other, records) in &self.nodes {
            let Ok(records) = records else { continue };
            for edge in records
                .meta
                .depends_on
                .iter()
                .chain(&records.meta.derived_from)
            {
                reverse.entry(edge).or_default().push(other);
            }
        }

        let mut reasons = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([id]);
        while let Some(node) = queue.pop_front() {
            if !seen.insert(node) {
                continue;
            }
            let state = self.state(node);
            let reason = match state.workability() {
                Workability::Complete => None,
                Workability::Error => Some(UnsettledReason::Error {
                    message: state.error().unwrap_or("unreadable").to_owned(),
                }),
                Workability::AwaitingIntegration => Some(UnsettledReason::AwaitingIntegration),
                Workability::Blocked => Some(UnsettledReason::Blocked {
                    blockers: state.blockers().to_vec(),
                }),
                Workability::Ready => Some(UnsettledReason::Open {
                    outcome: state.outcome(),
                    stale: state.currency() == Some(Currency::Stale),
                }),
            };
            if let Some(reason) = reason {
                reasons.push(Unsettled {
                    id: node.clone(),
                    reason,
                });
            }
            for dependent in reverse.get(node).into_iter().flatten() {
                queue.push_back(dependent);
            }
        }
        reasons
    }

    // --- evaluation -------------------------------------------------------------

    /// Evaluate every node, iteratively: a node is computed only once all its
    /// `depends_on` targets have been, and each is computed exactly once.
    fn evaluate_all(&mut self) {
        for (id, message) in &self.cycles {
            self.states.insert(
                id.clone(),
                NodeState::Error {
                    message: message.clone(),
                },
            );
        }
        let ids: Vec<NodeId> = self.nodes.keys().cloned().collect();
        for id in ids {
            if self.states.contains_key(&id) {
                continue;
            }
            let mut stack = vec![id];
            while let Some(top) = stack.last().cloned() {
                if self.states.contains_key(&top) {
                    stack.pop();
                    continue;
                }
                let dependencies: Vec<NodeId> = self
                    .nodes
                    .get(&top)
                    .and_then(|records| records.as_ref().ok())
                    .map(|records| records.meta.depends_on.clone())
                    .unwrap_or_default();
                let pending: Vec<NodeId> = dependencies
                    .into_iter()
                    .filter(|dep| self.nodes.contains_key(dep) && !self.states.contains_key(dep))
                    .collect();
                if pending.is_empty() {
                    let state = self.compute(&top);
                    self.states.insert(top, state);
                    stack.pop();
                } else {
                    stack.extend(pending);
                }
            }
        }
    }

    /// Derive one node's state. Every `depends_on` target already has a state.
    fn compute(&self, id: &NodeId) -> NodeState {
        let records = match self.nodes.get(id) {
            Some(Ok(records)) => records,
            Some(Err(message)) => {
                return NodeState::Error {
                    message: message.clone(),
                }
            }
            None => {
                return NodeState::Error {
                    message: "no such node".into(),
                }
            }
        };

        let (outcome, integration, staleness) = match self.evidence(id, records) {
            Ok(evidence) => evidence,
            Err(message) => return NodeState::Error { message },
        };
        let currency = if staleness.is_empty() {
            Currency::Current
        } else {
            Currency::Stale
        };

        let mut blockers = Vec::new();
        for dependency in &records.meta.depends_on {
            if !self.nodes.contains_key(dependency) {
                blockers.push(Blocker {
                    id: dependency.clone(),
                    reason: BlockerReason::Missing,
                });
                continue;
            }
            if let Some(reason) = blocker_reason(self.state(dependency)) {
                blockers.push(Blocker {
                    id: dependency.clone(),
                    reason,
                });
            }
        }

        NodeState::Known {
            outcome,
            currency,
            integration,
            staleness,
            blockers,
        }
    }

    /// The recorded outcome, integration status, and staleness of one node's
    /// own evidence. `Err` means the node's records are self-inconsistent.
    #[allow(clippy::type_complexity)]
    fn evidence(
        &self,
        id: &NodeId,
        records: &NodeRecords,
    ) -> std::result::Result<(RecordedOutcome, IntegrationStatus, Vec<StalenessReason>), String>
    {
        let Some((result, version)) = &records.result else {
            return Ok((
                RecordedOutcome::Open,
                IntegrationStatus::NotRequired,
                Vec::new(),
            ));
        };
        let verification = records.meta.is_verification();
        if !result.outcome.suits(verification) {
            return Err(if verification {
                format!("verification node `{id}` records a work outcome")
            } else {
                format!("ordinary node `{id}` records a verification outcome")
            });
        }
        let candidate = self.candidate_for_result(id, result, version)?;
        let integration = match candidate {
            None => IntegrationStatus::NotRequired,
            Some(candidate) => self.integration(candidate)?,
        };
        let staleness = self
            .staleness(records, result, candidate)
            .map_err(|error| format!("{error:#}"))?;
        Ok((
            RecordedOutcome::from(result.outcome),
            integration,
            staleness,
        ))
    }

    fn candidates_for_result(&self, id: &NodeId, version: &ResultVersion) -> Vec<&Candidate> {
        self.candidates_by_result
            .get(&(id.clone(), version.clone()))
            .into_iter()
            .flatten()
            .filter_map(|candidate| self.candidates.get(candidate))
            .collect()
    }

    /// The one candidate proposing this exact successful result's artifact.
    ///
    /// Only a successful work result can have one, which is also what keeps
    /// evaluation from looping: deciding a candidate consults verification
    /// nodes, whose own results are never `done`.
    fn candidate_for_result(
        &self,
        id: &NodeId,
        result: &ResultMeta,
        version: &ResultVersion,
    ) -> std::result::Result<Option<&Candidate>, String> {
        if result.outcome != Outcome::Done {
            return Ok(None);
        }
        let Some(artifact) = &result.output else {
            return Ok(None);
        };
        let mut matching = self
            .candidates_for_result(id, version)
            .into_iter()
            .filter(|candidate| candidate.artifact == *artifact);
        let first = matching.next();
        if matching.next().is_some() {
            return Err(format!(
                "node `{id}` result has more than one candidate for the same artifact"
            ));
        }
        Ok(first)
    }

    /// Integration derived from the candidate's decision and git ancestry.
    /// There is no case in which reading it fails: a target branch that moved
    /// simply is or is not an ancestor of the artifact.
    fn integration(&self, candidate: &Candidate) -> std::result::Result<IntegrationStatus, String> {
        Ok(match self.candidate_decision(&candidate.id)? {
            CandidateDecision::Pending => IntegrationStatus::Pending,
            CandidateDecision::Rejected => IntegrationStatus::Rejected,
            CandidateDecision::Accepted => {
                let target = self
                    .vcs
                    .ref_commit(&candidate.target_ref())
                    .map_err(|error| format!("{error:#}"))?;
                let published = match target {
                    Some(target) => self
                        .vcs
                        .is_ancestor(&candidate.artifact.id, &target)
                        .map_err(|error| format!("{error:#}"))?,
                    None => false,
                };
                if published {
                    IntegrationStatus::Published
                } else {
                    IntegrationStatus::Accepted
                }
            }
        })
    }

    /// The conclusion of whichever current, non-abandoned verification decided
    /// the candidate, computed once per pass.
    ///
    /// Every source node asks for its own candidate's decision, `check` asks
    /// again for each candidate, and the CLI asks a third time while
    /// rendering. Deciding means re-reading every naming review and
    /// recomputing its staleness, so the answer is cached exactly like a node
    /// state.
    fn candidate_decision(
        &self,
        candidate: &CandidateId,
    ) -> std::result::Result<CandidateDecision, String> {
        if let Some(decided) = self.decisions.borrow().get(candidate) {
            return decided.clone();
        }
        let decided = self.decide(candidate);
        self.decisions
            .borrow_mut()
            .insert(candidate.clone(), decided.clone());
        decided
    }

    /// Two current verifications disagreeing is a corrupt graph.
    fn decide(&self, candidate: &CandidateId) -> std::result::Result<CandidateDecision, String> {
        let mut decision = CandidateDecision::Pending;
        for verification in self.verifications_of(candidate) {
            let Some(Ok(records)) = self.nodes.get(verification) else {
                continue;
            };
            let Some((result, _)) = &records.result else {
                continue;
            };
            let concluded = match result.outcome {
                Outcome::Accepted => CandidateDecision::Accepted,
                Outcome::Rejected => CandidateDecision::Rejected,
                // Abandoned reaches no decision; a work outcome on a review
                // node is that node's own error, reported there.
                _ => continue,
            };
            // A stale review no longer decided *this* candidate's state of the
            // world, so it does not speak for it.
            if self.verification_is_stale(records) {
                continue;
            }
            match decision {
                CandidateDecision::Pending => decision = concluded,
                existing if existing == concluded => {}
                _ => {
                    return Err(format!(
                        "candidate `{candidate}` has current verifications that disagree"
                    ))
                }
            }
        }
        Ok(decision)
    }

    /// Whether a review node's own evidence has gone stale. Deliberately
    /// narrower than a full evaluation: a verification's result never has a
    /// candidate, so this cannot re-enter candidate evaluation.
    fn verification_is_stale(&self, records: &NodeRecords) -> bool {
        let Some((result, _)) = &records.result else {
            return false;
        };
        match self.staleness(records, result, None) {
            Ok(reasons) => !reasons.is_empty(),
            // Unreadable evidence cannot speak for a candidate either.
            Err(_) => true,
        }
    }

    fn staleness(
        &self,
        records: &NodeRecords,
        result: &ResultMeta,
        candidate: Option<&Candidate>,
    ) -> Result<Vec<StalenessReason>> {
        let mut reasons = Vec::new();
        if records.definition != result.definition {
            reasons.push(StalenessReason::DefinitionChanged {
                metadata: records.definition.metadata != result.definition.metadata,
                description: records.definition.description != result.definition.description,
            });
        }
        for consumed in &result.consumed {
            // A consumed node whose own records cannot be read can no longer
            // confirm the facts pinned against it.
            let Some(Ok(other)) = self.nodes.get(&consumed.id) else {
                reasons.push(StalenessReason::ConsumedNodeMissing {
                    id: consumed.id.clone(),
                });
                continue;
            };
            if other.definition != consumed.definition {
                reasons.push(StalenessReason::ConsumedDefinitionChanged {
                    id: consumed.id.clone(),
                });
            }
            let current_version = other.result.as_ref().map(|(_, version)| version.clone());
            if current_version != consumed.result {
                reasons.push(StalenessReason::ConsumedResultChanged {
                    id: consumed.id.clone(),
                });
            }
            let current_output = other
                .result
                .as_ref()
                .and_then(|(result, _)| result.output.clone());
            if current_output != consumed.output {
                reasons.push(StalenessReason::ConsumedOutputChanged {
                    id: consumed.id.clone(),
                });
            }
        }

        let observed = records
            .observed
            .iter()
            .filter(|observed| {
                records
                    .result
                    .as_ref()
                    .is_some_and(|(_, version)| observed.result == *version)
            })
            .flat_map(|observed| observed.pins.iter());
        for pin in result.context.iter().chain(observed) {
            match self.project_file_blob(&pin.path)? {
                Some(identity) if identity != pin.identity => {
                    reasons.push(StalenessReason::ContextChanged {
                        path: pin.path.clone(),
                    })
                }
                None => reasons.push(StalenessReason::ContextMissing {
                    path: pin.path.clone(),
                }),
                Some(_) => {}
            }
        }

        // A result backed by a candidate never drifts: its artifact commit is
        // immutable, and absence from the target branch is integration, not
        // staleness. Drift applies only to output applied directly.
        if let (Some(output), None) = (&result.output, candidate) {
            if let Some(detail) = self.vcs.drift(&output.id, None)? {
                reasons.push(StalenessReason::OutputDrifted {
                    artifact: output.id.clone(),
                    detail,
                });
            }
        }
        Ok(reasons)
    }

    fn project_file_blob(&self, path: &ProjectPath) -> Result<Option<String>> {
        project_file_blob(&self.store.project_root(), path)
    }
}

/// Why a `depends_on` target does not satisfy its dependent, or `None` when it
/// does. Only `done` work and an `accepted` review satisfy an edge, so a
/// rejected or abandoned review blocks even though it is itself complete.
fn blocker_reason(state: &NodeState) -> Option<BlockerReason> {
    let NodeState::Known {
        outcome, currency, ..
    } = state
    else {
        return Some(BlockerReason::Error);
    };
    // A rejected or abandoned review is complete — it will not be worked
    // again — but it never satisfies the edge that waited on it.
    let satisfies = state.is_complete()
        && matches!(
            outcome,
            RecordedOutcome::Succeeded | RecordedOutcome::Accepted
        );
    if satisfies {
        return None;
    }
    Some(match outcome {
        RecordedOutcome::Open => BlockerReason::Open,
        RecordedOutcome::Failed => BlockerReason::Failed,
        RecordedOutcome::Rejected => BlockerReason::Rejected,
        RecordedOutcome::Abandoned => BlockerReason::Abandoned,
        RecordedOutcome::Succeeded | RecordedOutcome::Accepted => {
            if state.is_awaiting_integration() {
                BlockerReason::AwaitingIntegration
            } else if *currency == Currency::Stale {
                BlockerReason::Stale
            } else {
                // Current, successful, and still not complete: its own
                // candidate was rejected, so the work is open again.
                BlockerReason::Open
            }
        }
    })
}

/// Read one node's own files. Anything unreadable, unparseable, or of an
/// unsupported schema becomes that node's error message.
fn read_records(store: &Store, id: &NodeId) -> std::result::Result<NodeRecords, String> {
    let (meta, _) = store.read_node(id).map_err(|error| format!("{error:#}"))?;
    let definition = store
        .node_version(id)
        .map_err(|error| format!("{error:#}"))?;
    let result = store
        .read_result(id)
        .map_err(|error| format!("{error:#}"))?
        .map(|(result, _)| {
            store
                .result_version(id)
                .map(|version| (result, version))
                .map_err(|error| format!("{error:#}"))
        })
        .transpose()?;
    let observed = store
        .read_observed_context(id)
        .map_err(|error| format!("{error:#}"))?;
    Ok(NodeRecords {
        meta,
        definition,
        result,
        observed,
    })
}

/// The blob id of a project file, refusing symlinks that resolve outside the
/// project root. `None` only when the file is proven absent.
pub(crate) fn project_file_blob(
    root: &std::path::Path,
    path: &ProjectPath,
) -> Result<Option<String>> {
    let candidate = root.join(path.as_str());
    match std::fs::canonicalize(&candidate) {
        Ok(resolved) => {
            let root = std::fs::canonicalize(root)?;
            if !resolved.starts_with(&root) {
                anyhow::bail!("project path `{path}` escapes the project root through a symlink");
            }
            file_blob(&resolved)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).map_err(|error| {
            anyhow::Error::from(error).context(format!("resolving project path `{path}`"))
        }),
    }
}

/// Every node on a `depends_on` cycle, mapped to the cycle path to report.
/// Iterative, so a pathological chain cannot overflow the stack.
fn dependency_cycles(
    nodes: &BTreeMap<NodeId, std::result::Result<NodeRecords, String>>,
) -> BTreeMap<NodeId, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    let mut marks: HashMap<&NodeId, Mark> = HashMap::new();
    let mut cycles = BTreeMap::new();

    for root in nodes.keys() {
        if marks.contains_key(root) {
            continue;
        }
        // Explicit DFS: (node, index of the next edge to follow).
        let mut stack: Vec<(&NodeId, usize)> = vec![(root, 0)];
        marks.insert(root, Mark::Visiting);
        while let Some((node, edge)) = stack.pop() {
            let edges = nodes
                .get(node)
                .and_then(|records| records.as_ref().ok())
                .map(|records| records.meta.depends_on.as_slice())
                .unwrap_or_default();
            let Some(next) = edges.get(edge) else {
                marks.insert(node, Mark::Done);
                continue;
            };
            stack.push((node, edge + 1));
            // Only walk into nodes the store actually holds; missing targets
            // are a blocker, reported separately.
            let Some((next, _)) = nodes.get_key_value(next) else {
                continue;
            };
            match marks.get(next) {
                Some(Mark::Done) => {}
                Some(Mark::Visiting) => {
                    let start = stack
                        .iter()
                        .position(|(node, _)| node == &next)
                        .unwrap_or(0);
                    let mut path: Vec<&str> = stack[start..]
                        .iter()
                        .map(|(node, _)| node.as_str())
                        .collect();
                    path.push(next.as_str());
                    let report = format!("dependency cycle: {}", path.join(" -> "));
                    for (node, _) in &stack[start..] {
                        cycles.insert((*node).clone(), report.clone());
                    }
                }
                None => {
                    marks.insert(next, Mark::Visiting);
                    stack.push((next, 0));
                }
            }
        }
    }
    cycles
}
