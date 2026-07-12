//! Graph operations and derived queries.
//!
//! Every mutating operation ([`add`], [`link`], [`edit`], [`complete`], [`fail`])
//! commits its own store change to the workbench repository, so each is its own
//! git commit there. The project repository is checked only where output
//! provenance is asserted: [`complete`] refuses undeclared dirty writes
//! ([`require_clean_except`]); pure graph edits never gate on project state.
//! The derived queries ([`current_status`], [`staleness`], [`blockers`],
//! [`is_ready`]) recompute from the node files and are never stored.
//!
//! All git interaction goes through `&dyn Vcs`, so the whole module is
//! unit-testable with an in-memory fake — no git binary, repository, or identity
//! required. (Blob hashing for versions and pins is computed locally.)

use anyhow::{bail, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

use crate::model::{
    ArtifactRef, Author, Blocker, BlockerReason, ConsumedNode, ContextPin, Currency,
    DefinitionVersion, DepKind, NodeMeta, NodeState, Outcome, RecordedOutcome, ResultMeta,
    ResultVersion, StalenessReason, Status,
};
use crate::pairing::Pairing;
use crate::store::{file_blob, Store};
use crate::vcs::Vcs;

pub struct InitializedWorkbench {
    pub store: Store,
    pub pairing: Pairing,
    pub created_workbench_repo: bool,
    pub created_project_repo: bool,
    pub created_project_root: bool,
}

/// Create a complete usable workbench. Frontends call this rather than
/// exposing the lower-level directory-only `Store::init` as initialization.
pub fn init_workbench(
    root: std::path::PathBuf,
    name: Option<String>,
) -> Result<InitializedWorkbench> {
    let store = Store::init(root)?;
    let created_workbench_repo = crate::git::ensure_repo(&store.workbench_root())?;
    let created_project_repo = crate::git::ensure_repo(&store.project_root())?;
    let created_project_root = crate::git::ensure_root_commit(&store.project_root())?;
    let vcs = crate::git::GitVcs::for_store(&store);
    let pairing = pair(&store, &vcs, name, false)?;
    Ok(InitializedWorkbench {
        store,
        pairing,
        created_workbench_repo,
        created_project_repo,
        created_project_root,
    })
}

/// Parameters for creating a node with [`add`].
pub struct NewNode {
    /// The definition prose (markdown). Its first line serves as the title.
    pub description: String,
    pub author: Author,
    /// Who the work is for (e.g. `human` for a question node); `None` = anyone.
    pub assignee: Option<Author>,
    /// Ids this node depends on (must exist).
    pub depends_on: Vec<String>,
    /// Ids this node is derived from (must exist).
    pub derived_from: Vec<String>,
}

/// Create a new node. Returns its id.
pub fn add(store: &Store, vcs: &dyn Vcs, new: NewNode) -> Result<String> {
    if new.description.trim().is_empty() {
        bail!("a node needs a description");
    }
    for dep in new.depends_on.iter().chain(&new.derived_from) {
        if !store.exists(dep) {
            bail!("unknown related node `{dep}`");
        }
    }
    let id = format!("node-{}", Ulid::new());
    let meta = NodeMeta {
        schema: 1,
        author: new.author,
        assignee: new.assignee,
        depends_on: new
            .depends_on
            .into_iter()
            .map(|id| id.parse())
            .collect::<std::result::Result<_, String>>()
            .map_err(anyhow::Error::msg)?,
        derived_from: new
            .derived_from
            .into_iter()
            .map(|id| id.parse())
            .collect::<std::result::Result<_, String>>()
            .map_err(anyhow::Error::msg)?,
        extensions: Default::default(),
    };
    store.write_node(&id, &meta, &new.description)?;
    vcs.commit_store(&store.store_name(), &format!("linka: add {id}"))?;
    Ok(id)
}

/// Add `to` to one of `from`'s dependency lists. A definition change: it moves
/// `from`'s version.
pub fn link(store: &Store, vcs: &dyn Vcs, from: &str, to: &str, kind: DepKind) -> Result<()> {
    if from == to {
        bail!("cannot link a node to itself");
    }
    if !store.exists(to) {
        bail!("unknown related node `{to}`");
    }
    let (mut meta, description) = store.read_node(from)?;
    let edges = match kind {
        DepKind::DependsOn => &mut meta.depends_on,
        DepKind::DerivedFrom => &mut meta.derived_from,
    };
    if edges.iter().any(|id| id.as_str() == to) {
        bail!("duplicate edge");
    }
    edges.push(to.parse().map_err(anyhow::Error::msg)?);
    store.write_node(from, &meta, &description)?;
    vcs.commit_store(&store.store_name(), &format!("linka: link {from} -> {to}"))?;
    Ok(())
}

/// Edit a node's description. A definition change: it moves the node's
/// version, so a prior `done` no longer covers it and dependents' pins go stale.
pub fn edit(store: &Store, vcs: &dyn Vcs, id: &str, description: String) -> Result<()> {
    if description.trim().is_empty() {
        bail!("a node needs a description");
    }
    let (meta, _) = store.read_node(id)?;
    store.write_node(id, &meta, &description)?;
    vcs.commit_store(&store.store_name(), &format!("linka: edit {id}"))?;
    Ok(())
}

/// Complete a node's work: commit all produced files as one output commit, pin
/// what the work was built against (dependency versions and outputs, plus any
/// extra context files), and record it all in `result.toml` and `result.md`.
/// Returns the output commit, or `None` when the work produced no files
/// (graph-only work).
#[allow(clippy::too_many_arguments)] // mirrors the CLI surface one-to-one
pub fn complete(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    outputs: &[String],
    context: &[String],
    message: Option<String>,
    notes: &str,
    author: Author,
) -> Result<Option<String>> {
    let outputs: Vec<String> = outputs
        .iter()
        .map(|path| {
            path.parse::<crate::model::ProjectPath>()
                .map(|path| path.to_string())
                .map_err(anyhow::Error::msg)
        })
        .collect::<Result<_>>()?;
    // The only uncommitted project changes allowed are the outputs we are about
    // to commit — completion is where output provenance is asserted.
    require_clean_except(vcs, &outputs)?;
    let (meta, description) = store.read_node(id)?;

    let input_commit = vcs.head_commit()?;

    // Pin everything the work saw, before committing anything.
    let context = pin_context(store, vcs, context)?;
    let consumed = pin_deps(store, &meta)?;

    let output_commit = if outputs.is_empty() {
        None
    } else {
        let message = message.unwrap_or_else(|| crate::model::title_of(&description).to_string());
        let mut commit_message = format!("{message}\n\nLinka-Node: {id}");
        if let Some(input) = &input_commit {
            commit_message.push_str(&format!("\nLinka-Input: {input}"));
        }
        let commit = vcs.capture(&outputs, &commit_message)?;
        vcs.retain_output(id, &commit)?;
        Some(commit)
    };

    let result = ResultMeta {
        schema: 1,
        at: now_millis(),
        author,
        definition: store.node_version(id)?,
        outcome: Outcome::Done,
        consumed,
        context,
        output: output_commit.as_deref().map(git_artifact),
        producer: None,
    };
    store.write_result(id, &result, notes)?;
    vcs.commit_store(&store.store_name(), &format!("linka: complete {id}"))?;
    Ok(output_commit)
}

/// Answer a node: record it done with the response as its result notes,
/// producing no output commit. Unlike [`complete`] this does not gate on
/// project-tree cleanliness: an answer asserts no output provenance, and a
/// question node is typically answered mid-work, while the tree is dirty with
/// whatever prompted the question. Dependency versions are still pinned, so
/// the answer participates in staleness like any other result.
pub fn respond(store: &Store, vcs: &dyn Vcs, id: &str, notes: &str, author: Author) -> Result<()> {
    if notes.trim().is_empty() {
        bail!("a response needs some text");
    }
    let (meta, _) = store.read_node(id)?;
    let consumed = pin_deps(store, &meta)?;
    let result = ResultMeta {
        schema: 1,
        at: now_millis(),
        author,
        definition: store.node_version(id)?,
        outcome: Outcome::Done,
        consumed,
        context: Vec::new(),
        output: None,
        producer: None,
    };
    store.write_result(id, &result, notes)?;
    vcs.commit_store(&store.store_name(), &format!("linka: respond {id}"))?;
    Ok(())
}

/// Record that a node's work was attempted and failed. Like [`complete`] it pins
/// what the attempt was built against, so the failure is reproducible evidence.
/// It does not gate on project-tree cleanliness: a failed attempt may well have
/// left a mess, and recording the failure must not be blocked by it.
pub fn fail(store: &Store, vcs: &dyn Vcs, id: &str, notes: &str, author: Author) -> Result<()> {
    let (meta, _) = store.read_node(id)?;
    let consumed = pin_deps(store, &meta)?;
    let result = ResultMeta {
        schema: 1,
        at: now_millis(),
        author,
        definition: store.node_version(id)?,
        outcome: Outcome::Failed,
        consumed,
        context: Vec::new(),
        output: None,
        producer: None,
    };
    store.write_result(id, &result, notes)?;
    vcs.commit_store(&store.store_name(), &format!("linka: fail {id}"))?;
    Ok(())
}

/// Commit whatever of the node's streamed interaction log (`work.jsonl`) is not
/// yet in git. The log is written line by line *during* a session, dirtying only
/// the workbench repository, and each store commit the session makes already
/// sweeps the story-so-far in; this picks up the tail after the session ends.
/// A no-op when the log is fully committed.
pub fn commit_work_log(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<()> {
    vcs.commit_store(&store.store_name(), &format!("linka: work log {id}"))?;
    Ok(())
}

/// Append *observed* context pins to a node's recorded result: files a work
/// session was seen reading (mined from its recorded transcript) that the
/// worker did not declare in `complete`. Turns input provenance from agent
/// discipline into a derived fact.
///
/// Skips paths already pinned, paths inside any node's output commit (those
/// are covered by `consumed` pins — context is for files no node produced),
/// and files that no longer exist. Returns how many pins were added; a no-op
/// when the node has no result yet (a paused unit's reads are amended once it
/// completes, from the full replayed log).
pub fn amend_context(store: &Store, vcs: &dyn Vcs, id: &str, reads: &[String]) -> Result<usize> {
    let Some((mut result, notes)) = store.read_result(id)? else {
        return Ok(0);
    };

    let mut node_outputs = std::collections::HashSet::new();
    for other in store.list_ids()? {
        if let Some(commit) = output_of(store, &other)? {
            node_outputs.extend(vcs.files_in(&commit)?);
        }
    }

    let root = store.project_root();
    let mut pinned: std::collections::HashSet<String> =
        result.context.iter().map(|p| p.path.to_string()).collect();
    let mut added = 0;
    for path in reads {
        if pinned.contains(path) || node_outputs.contains(path) {
            continue;
        }
        let project_path: crate::model::ProjectPath = path.parse().map_err(anyhow::Error::msg)?;
        let Some(blob) = vcs
            .file_blob(project_path.as_str())?
            .or(project_file_blob(&root, &project_path)?)
        else {
            continue;
        };
        pinned.insert(path.clone());
        result.context.push(ContextPin {
            path: path.parse().map_err(anyhow::Error::msg)?,
            identity: blob,
            observed: true,
        });
        added += 1;
    }
    if added == 0 {
        return Ok(0);
    }
    store.write_result(id, &result, &notes)?;
    vcs.commit_store(
        &store.store_name(),
        &format!("linka: observed context {id}"),
    )?;
    Ok(added)
}

/// Derive all graph state through one fallible evaluation.
pub fn node_state(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<NodeState> {
    let mut visiting = std::collections::HashSet::new();
    node_state_inner(store, vcs, id, &mut visiting)
}

fn node_state_inner(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    visiting: &mut std::collections::HashSet<String>,
) -> Result<NodeState> {
    let (meta, _) = store
        .read_node(id)
        .with_context(|| format!("reading definition for `{id}`"))?;
    if !visiting.insert(id.to_string()) {
        bail!("dependency cycle while deriving state at `{id}`");
    }
    let result = (|| {
        let result = store.read_result(id)?;
        let (outcome, staleness) = match result.as_ref() {
            None => (RecordedOutcome::Open, Vec::new()),
            Some((result, _)) => {
                let outcome = match result.outcome {
                    Outcome::Done => RecordedOutcome::Succeeded,
                    Outcome::Failed => RecordedOutcome::Failed,
                };
                (outcome, staleness_for_result(store, vcs, id, result)?)
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
                    id: dependency.to_string(),
                    reason: BlockerReason::Missing,
                });
                continue;
            }
            let dependency_state = node_state_inner(store, vcs, dependency, visiting)?;
            if !dependency_state.is_complete() {
                let reason = if dependency_state.currency == Currency::Stale {
                    BlockerReason::Stale
                } else {
                    match dependency_state.outcome {
                        RecordedOutcome::Open => BlockerReason::Open,
                        RecordedOutcome::Failed => BlockerReason::Failed,
                        RecordedOutcome::Succeeded => BlockerReason::Stale,
                    }
                };
                blockers.push(Blocker {
                    id: dependency.to_string(),
                    reason,
                });
            }
        }
        Ok(NodeState {
            outcome,
            currency,
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
    id: &str,
    result: &ResultMeta,
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
                id: consumed.id.to_string(),
            });
            continue;
        }
        if store.node_version(&consumed.id)? != consumed.definition {
            reasons.push(StalenessReason::ConsumedDefinitionChanged {
                id: consumed.id.to_string(),
            });
        }
        let current_result = store.read_result(&consumed.id)?;
        let current_version = current_result
            .is_some()
            .then(|| store.result_version(&consumed.id))
            .transpose()?;
        if current_version != consumed.result {
            reasons.push(StalenessReason::ConsumedResultChanged {
                id: consumed.id.to_string(),
            });
        }
        let current_output = current_result.and_then(|(r, _)| r.output);
        if current_output != consumed.output {
            reasons.push(StalenessReason::ConsumedOutputChanged {
                id: consumed.id.to_string(),
            });
        }
    }
    let root = store.project_root();
    for pin in &result.context {
        match project_file_blob(&root, &pin.path)? {
            Some(now) if now != pin.identity => reasons.push(StalenessReason::ContextChanged {
                path: pin.path.to_string(),
            }),
            None => reasons.push(StalenessReason::ContextMissing {
                path: pin.path.to_string(),
            }),
            _ => {}
        }
    }
    if let Some(output) = &result.output {
        if let Some(detail) = vcs.drift(&output.id)? {
            reasons.push(StalenessReason::OutputDrifted {
                artifact: output.id.clone(),
                detail,
            });
        }
    }
    Ok(reasons)
}

#[deprecated(note = "use node_state; Status cannot represent stale or evaluation errors")]
pub fn current_status(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<Status> {
    let state = node_state(store, vcs, id)?;
    Ok(match state.outcome {
        RecordedOutcome::Open => Status::Open,
        RecordedOutcome::Failed => Status::Failed,
        RecordedOutcome::Succeeded if state.currency == Currency::Current => Status::Done,
        RecordedOutcome::Succeeded => Status::Open,
    })
}

pub fn staleness(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<Vec<StalenessReason>> {
    Ok(node_state(store, vcs, id)?.staleness)
}

pub fn blockers(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<Vec<Blocker>> {
    Ok(node_state(store, vcs, id)?.blockers)
}

pub fn is_ready(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<bool> {
    Ok(node_state(store, vcs, id)?.is_ready())
}

pub fn ready_nodes(store: &Store, vcs: &dyn Vcs, worker: Option<Author>) -> Result<Vec<String>> {
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

pub fn first_ready_for(store: &Store, vcs: &dyn Vcs, worker: Author) -> Result<Option<String>> {
    Ok(ready_nodes(store, vcs, Some(worker))?.into_iter().next())
}

/// The node whose work produced `commit`, if any — the inverse of the output
/// artifact on each result, derived by scanning rather than persisted as a
/// second index. Unique because each completion mints one commit for one node.
pub fn origin(store: &Store, commit: &str) -> Result<Option<String>> {
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
pub fn output_of(store: &Store, id: &str) -> Result<Option<String>> {
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }
    Ok(store
        .read_result(id)?
        .and_then(|(result, _)| result.output.map(|artifact| artifact.id)))
}

/// Ids of nodes that name `id` in either dependency list.
pub fn dependents(store: &Store, id: &str) -> Result<Vec<String>> {
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }
    let mut out = Vec::new();
    for other in store.list_ids()? {
        if other == id {
            continue;
        }
        let (meta, _) = store.read_node(&other)?;
        if meta
            .depends_on
            .iter()
            .chain(&meta.derived_from)
            .any(|d| d.as_str() == id)
        {
            out.push(other);
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
pub fn unsettled(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<Vec<String>> {
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }
    // Reverse adjacency over both edge kinds, built in one scan.
    let mut rev: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for other in store.list_ids()? {
        let (meta, _) = store.read_node(&other)?;
        for dep in meta.depends_on.iter().chain(&meta.derived_from) {
            rev.entry(dep.to_string()).or_default().push(other.clone());
        }
    }

    let mut reasons = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([id.to_string()]);
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        let state = node_state(store, vcs, &node)?;
        if !state.is_complete() {
            if state.outcome == RecordedOutcome::Succeeded {
                reasons.push(format!("{node}: done but stale"));
            } else {
                let outcome = match state.outcome {
                    RecordedOutcome::Open => "open",
                    RecordedOutcome::Failed => "failed",
                    RecordedOutcome::Succeeded => unreachable!(),
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

/// Integrity-check the whole store, fsck-style: every problem that write-time
/// validation cannot see because it entered sideways (hand edits, git merges of
/// individually-valid branches, older tools). Returns explicit problem reports;
/// empty means the store is consistent. Read-only and git-free.
///
/// Checked per node: definition and result files parse; dependency lists hold no
/// duplicates or self-references; every edge target exists; and `depends_on`
/// contains no cycles (which would deadlock readiness — every node in the
/// cycle waiting on another).
pub fn check(store: &Store) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut depends_on: std::collections::BTreeMap<String, Vec<String>> = Default::default();

    for id in store.list_ids()? {
        let meta = match store.read_node(&id) {
            Ok((meta, _)) => meta,
            Err(e) => {
                problems.push(format!("{id}: unreadable definition ({e:#})"));
                continue;
            }
        };
        if meta.schema != 1 {
            problems.push(format!(
                "{id}: unsupported definition schema {}",
                meta.schema
            ));
        }
        match store.read_result(&id) {
            Err(e) => problems.push(format!("{id}: unreadable result ({e:#})")),
            Ok(Some((result, _))) => validate_result_semantics(&id, &meta, &result, &mut problems),
            Ok(None) => {}
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
                if dep.as_str() == id {
                    problems.push(format!("{id}: {kind} refers to the node itself"));
                    continue;
                }
                if store.read_node(dep).is_err() {
                    problems.push(format!("{id}: {kind} target `{dep}` missing or unreadable"));
                }
            }
        }
        depends_on.insert(id, meta.depends_on.into_iter().map(Into::into).collect());
    }

    problems.extend(find_cycles(&depends_on));
    Ok(problems)
}

fn validate_result_semantics(
    id: &str,
    meta: &NodeMeta,
    result: &ResultMeta,
    problems: &mut Vec<String>,
) {
    if result.schema != 1 {
        problems.push(format!("{id}: unsupported result schema {}", result.schema));
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
            && result.outcome == Outcome::Done
            && (pin.result.is_none() || pin.outcome != Some(Outcome::Done))
        {
            problems.push(format!(
                "{id}: successful result has no successful evidence for required dependency `{}`",
                pin.id
            ));
        }
        if let Some(output) = &pin.output {
            validate_artifact(id, output, problems);
        }
    }
    if result.outcome == Outcome::Done {
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
        validate_artifact(id, output, problems);
    }
}

fn validate_artifact(id: &str, artifact: &ArtifactRef, problems: &mut Vec<String>) {
    if artifact.scheme != "git-commit" {
        problems.push(format!(
            "{id}: unsupported artifact scheme `{}`",
            artifact.scheme
        ));
    }
    if !artifact.repository.is_empty()
        && (artifact.repository.len() != 40
            || !artifact
                .repository
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()))
    {
        problems.push(format!(
            "{id}: invalid artifact repository identity `{}`",
            artifact.repository
        ));
    }
}

pub fn check_artifacts(store: &Store, vcs: &dyn Vcs) -> Result<Vec<String>> {
    let mut problems = check(store)?;
    for id in store.list_ids()? {
        if let Some((result, _)) = store.read_result(&id)? {
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

/// Report each `depends_on` cycle once, as an explicit `a -> b -> a` path.
fn find_cycles(graph: &std::collections::BTreeMap<String, Vec<String>>) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Visiting,
        Done,
    }
    fn visit(
        node: &str,
        graph: &std::collections::BTreeMap<String, Vec<String>>,
        state: &mut std::collections::HashMap<String, State>,
        stack: &mut Vec<String>,
        out: &mut Vec<String>,
    ) {
        match state.get(node) {
            Some(State::Done) => return,
            Some(State::Visiting) => {
                // Back-edge: the cycle is the stack from the first occurrence on.
                let start = stack.iter().position(|n| n == node).unwrap_or(0);
                let mut path: Vec<&str> = stack[start..].iter().map(String::as_str).collect();
                path.push(node);
                out.push(format!("dependency cycle: {}", path.join(" -> ")));
                return;
            }
            None => {}
        }
        state.insert(node.to_string(), State::Visiting);
        stack.push(node.to_string());
        for dep in graph.get(node).into_iter().flatten() {
            // Missing targets are reported separately; only follow known nodes.
            if graph.contains_key(dep) {
                visit(dep, graph, state, stack, out);
            }
        }
        stack.pop();
        state.insert(node.to_string(), State::Done);
    }

    let mut state = std::collections::HashMap::new();
    let mut out = Vec::new();
    for node in graph.keys() {
        visit(node, graph, &mut state, &mut Vec::new(), &mut out);
    }
    out
}

/// Record which project repository this store describes, keyed by the
/// project's root commit (`pairing.toml` in the store, committed to the
/// workbench repository like any other store change). Idempotent when the
/// recorded root already matches. A mismatch is the error this exists to
/// catch — the wrong project sitting in the workbench, or a rewritten
/// history — and needs `force` to overwrite deliberately.
///
/// Two purely informational fields ride along for human readers, never
/// checked by anything: `name`, given by the caller, and the project's
/// `origin` remote URL, observed here. On a same-root re-pair they are
/// refreshed (a given name wins; a currently-present remote wins) without
/// touching the identity or its timestamp.
pub fn pair(store: &Store, vcs: &dyn Vcs, name: Option<String>, force: bool) -> Result<Pairing> {
    let Some(root) = vcs.root_commit()? else {
        bail!("the project repository has no commits yet — nothing to pair to");
    };
    let remote = vcs.remote_url()?;
    if let Some(existing) = Pairing::load(store.root())? {
        if existing.root_commit == root {
            let updated = Pairing {
                name: name.or_else(|| existing.name.clone()),
                remote: remote.or_else(|| existing.remote.clone()),
                ..existing.clone()
            };
            if updated.name == existing.name && updated.remote == existing.remote {
                return Ok(existing);
            }
            updated.save(store.root())?;
            vcs.commit_store(&store.store_name(), "linka: pair project (update info)")?;
            return Ok(updated);
        }
        if !force {
            bail!(
                "store is paired to project root {} but the project's root is {} — \
                 wrong project in the workbench, or a rewritten history \
                 (re-pair with --force if this is intentional)",
                short(&existing.root_commit),
                short(&root)
            );
        }
    }
    let pairing = Pairing {
        schema: 1,
        root_commit: root,
        paired_at: now_millis(),
        name,
        remote,
    };
    pairing.save(store.root())?;
    vcs.commit_store(&store.store_name(), "linka: pair project")?;
    Ok(pairing)
}

/// Verify the store↔project pairing. Read-only and manual — nothing calls it
/// implicitly. Returns the recorded pairing (`None` means the store is not
/// paired, which is a notice, not a problem — stores predating pairing
/// exist) and the list of problems found. Only the root commit is checked;
/// the pairing's name and remote are information for the caller to display.
///
/// The default check is one comparison: the project's actual root commit
/// against the recorded one. With `deep`, every hash the store points at —
/// each result's output commit and every consumed output pin — is also
/// checked to exist in the project repository, catching partial history
/// rewrites that leave the root intact but orphan recorded outputs.
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
            short(&pairing.root_commit)
        )),
        Some(actual) if actual != pairing.root_commit => problems.push(format!(
            "project root commit is {} but the store is paired to {} — \
             wrong project in the workbench, or a rewritten history \
             (`linka pair --force` re-pairs deliberately)",
            short(&actual),
            short(&pairing.root_commit)
        )),
        Some(_) => {}
    }
    if deep {
        for id in store.list_ids()? {
            let Some((result, _)) = store.read_result(&id)? else {
                continue;
            };
            if let Some(output) = &result.output {
                if !vcs.commit_exists(&output.id)? {
                    problems.push(format!(
                        "{id}: output commit {} does not exist in the project repository",
                        short(&output.id)
                    ));
                }
            }
            for consumed in &result.consumed {
                if let Some(output) = &consumed.output {
                    if !vcs.commit_exists(&output.id)? {
                        problems.push(format!(
                            "{id}: built-against output {} (of {}) does not exist in the project repository",
                            short(&output.id),
                            consumed.id
                        ));
                    }
                }
            }
        }
    }
    Ok((Some(pairing), problems))
}

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

/// Pin the current version and output of every node in `meta`'s dependency lists.
fn pin_deps(store: &Store, meta: &NodeMeta) -> Result<Vec<ConsumedNode>> {
    meta.depends_on
        .iter()
        .chain(&meta.derived_from)
        .map(|dep| {
            let definition = store
                .node_version(dep)
                .with_context(|| format!("cannot pin unknown dependency `{dep}`"))?;
            let result = store
                .read_result(dep)?
                .is_some()
                .then(|| store.result_version(dep))
                .transpose()?;
            Ok(ConsumedNode {
                id: dep.clone(),
                definition,
                result,
                outcome: store.read_result(dep)?.map(|(result, _)| result.outcome),
                output: output_of(store, dep)?.as_deref().map(git_artifact),
            })
        })
        .collect()
}

/// Pin each context path by its current content; errors if a file is missing.
fn pin_context(store: &Store, vcs: &dyn Vcs, paths: &[String]) -> Result<Vec<ContextPin>> {
    let root = store.project_root();
    paths
        .iter()
        .map(|path| {
            let path: crate::model::ProjectPath = path.parse().map_err(anyhow::Error::msg)?;
            let blob = vcs
                .file_blob(path.as_str())?
                .or(project_file_blob(&root, &path)?)
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

fn git_artifact(commit: &str) -> ArtifactRef {
    ArtifactRef {
        scheme: "git-commit".into(),
        repository: String::new(),
        id: commit.into(),
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::vcs::FakeVcs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A temp directory removed on drop, so tests are self-contained.
    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fresh, initialised store under a unique temp directory.
    fn temp_store() -> (TempDir, Store) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("linka-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::init(root.join(".linka")).unwrap();
        (TempDir(root), store)
    }

    fn new_node(description: &str, depends_on: Vec<String>) -> NewNode {
        NewNode {
            description: description.into(),
            author: Author::Human,
            assignee: None,
            depends_on,
            derived_from: vec![],
        }
    }

    fn done(store: &Store, vcs: &dyn Vcs, id: &str) {
        complete(store, vcs, id, &[], &[], None, "done", Author::Human).unwrap();
    }

    #[test]
    fn output_and_dependent_queries_reject_unknown_nodes() {
        let (_t, store) = temp_store();
        assert!(output_of(&store, "missing").is_err());
        assert!(dependents(&store, "missing").is_err());
    }

    #[test]
    fn amend_context_pins_observed_reads_only() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            next_id: "c1".into(),
            ..Default::default()
        };
        let root = store.project_root();
        std::fs::write(root.join("declared.txt"), "d").unwrap();
        std::fs::write(root.join("read.txt"), "r").unwrap();
        std::fs::write(root.join("out.txt"), "o").unwrap();

        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        complete(
            &store,
            &fake,
            &id,
            &["out.txt".into()],
            &["declared.txt".into()],
            None,
            "done",
            Author::Human,
        )
        .unwrap();

        // One genuinely new read gets pinned; a declared pin, a node output, a
        // missing file, and a duplicate do not.
        let reads: Vec<String> = [
            "read.txt",
            "declared.txt",
            "out.txt",
            "missing.txt",
            "read.txt",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(amend_context(&store, &fake, &id, &reads).unwrap(), 1);

        let (result, notes) = store.read_result(&id).unwrap().unwrap();
        assert_eq!(notes, "done", "amending keeps the narrative");
        let pin = result
            .context
            .iter()
            .find(|p| p.path == "read.txt")
            .unwrap();
        assert!(pin.observed);
        let declared = result
            .context
            .iter()
            .find(|p| p.path == "declared.txt")
            .unwrap();
        assert!(!declared.observed);
        assert!(!result
            .context
            .iter()
            .any(|p| p.path == "out.txt" || p.path == "missing.txt"));

        // Re-running with the same reads adds nothing.
        assert_eq!(amend_context(&store, &fake, &id, &reads).unwrap(), 0);
    }

    #[test]
    fn amend_context_is_a_no_op_without_a_result() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        let commits_before = *fake.store_commits.borrow();
        assert_eq!(
            amend_context(&store, &fake, &id, &["x.txt".into()]).unwrap(),
            0
        );
        assert_eq!(*fake.store_commits.borrow(), commits_before);
    }

    #[test]
    fn observed_pins_participate_in_staleness() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let root = store.project_root();
        std::fs::write(root.join("read.txt"), "v1").unwrap();

        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        done(&store, &fake, &id);
        assert_eq!(
            amend_context(&store, &fake, &id, &["read.txt".into()]).unwrap(),
            1
        );
        assert!(staleness(&store, &fake, &id).unwrap().is_empty());

        std::fs::write(root.join("read.txt"), "v2").unwrap();
        let reasons = staleness(&store, &fake, &id).unwrap();
        assert!(
            reasons
                .iter()
                .any(|r| format!("{r:?}").contains("read.txt")),
            "{reasons:?}"
        );
    }

    #[test]
    fn add_validates_dependencies_and_starts_open() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();

        assert!(add(&store, &fake, new_node("a", vec!["node-nope".into()])).is_err());

        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        assert!(store.exists(&id));
        assert_eq!(current_status(&store, &fake, &id).unwrap(), Status::Open);
        assert!(
            staleness(&store, &fake, &id).unwrap().is_empty(),
            "no result, nothing to invalidate"
        );
    }

    #[test]
    fn complete_records_result_and_output_commit() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            next_id: "commit-abc".into(),
            ..Default::default()
        };
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();

        let commit = complete(
            &store,
            &fake,
            &id,
            &["src/x.rs".into()],
            &[],
            None,
            "implemented it",
            Author::Human,
        )
        .unwrap();
        assert_eq!(commit.as_deref(), Some("commit-abc"));
        assert_eq!(current_status(&store, &fake, &id).unwrap(), Status::Done);
        assert_eq!(
            output_of(&store, &id).unwrap().as_deref(),
            Some("commit-abc")
        );

        let (result, notes) = store.read_result(&id).unwrap().unwrap();
        assert_eq!(result.definition, store.node_version(&id).unwrap());
        assert_eq!(notes, "implemented it");

        // The right paths were captured; add + complete each committed the store.
        assert_eq!(
            fake.captured.borrow().as_slice(),
            &[vec!["src/x.rs".to_string()]]
        );
        assert_eq!(*fake.store_commits.borrow(), 2);
    }

    #[test]
    fn complete_without_outputs_makes_no_commit() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("planning", vec![])).unwrap();

        let commit = complete(
            &store,
            &fake,
            &id,
            &[],
            &[],
            None,
            "made sub-tasks",
            Author::Human,
        )
        .unwrap();
        assert_eq!(commit, None);
        assert_eq!(current_status(&store, &fake, &id).unwrap(), Status::Done);
        assert!(fake.captured.borrow().is_empty(), "nothing captured");
    }

    #[test]
    fn editing_a_done_node_reopens_it() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        done(&store, &fake, &id);
        assert_eq!(current_status(&store, &fake, &id).unwrap(), Status::Done);

        edit(&store, &fake, &id, "revised".into()).unwrap();
        assert_eq!(current_status(&store, &fake, &id).unwrap(), Status::Open);
        let reasons = staleness(&store, &fake, &id).unwrap();
        assert!(matches!(
            reasons.as_slice(),
            [StalenessReason::DefinitionChanged {
                description: true,
                ..
            }]
        ));
        assert!(node_state(&store, &fake, &id).unwrap().is_ready());
    }

    #[test]
    fn editing_node_metadata_reopens_it() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        done(&store, &fake, &id);

        let (mut meta, description) = store.read_node(&id).unwrap();
        meta.assignee = Some(Author::Human);
        store.write_node(&id, &meta, &description).unwrap();

        assert_eq!(current_status(&store, &fake, &id).unwrap(), Status::Open);
        let reasons = staleness(&store, &fake, &id).unwrap();
        assert!(matches!(
            reasons.as_slice(),
            [StalenessReason::DefinitionChanged { metadata: true, .. }]
        ));
    }

    #[test]
    fn dependency_definition_move_makes_dependent_stale() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        done(&store, &fake, &a);
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        done(&store, &fake, &b);
        assert!(staleness(&store, &fake, &b).unwrap().is_empty());

        edit(&store, &fake, &a, "revised".into()).unwrap();
        let reasons = staleness(&store, &fake, &b).unwrap();
        assert_eq!(
            reasons,
            vec![StalenessReason::ConsumedDefinitionChanged { id: a }]
        );
    }

    #[test]
    fn dependency_output_change_makes_dependent_stale() {
        let (_t, store) = temp_store();
        let mut fake = FakeVcs {
            next_id: "commit-1".into(),
            ..Default::default()
        };
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        complete(
            &store,
            &fake,
            &a,
            &["src/a.rs".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        done(&store, &fake, &b);
        assert!(staleness(&store, &fake, &b).unwrap().is_empty());

        // A is re-worked and produces a new output commit -> B is stale.
        fake.next_id = "commit-2".into();
        complete(
            &store,
            &fake,
            &a,
            &["src/a.rs".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        let reasons = staleness(&store, &fake, &b).unwrap();
        assert!(reasons.contains(&StalenessReason::ConsumedOutputChanged { id: a }));
        assert!(node_state(&store, &fake, &b).unwrap().is_ready());
    }

    #[test]
    fn dependency_result_notes_change_makes_dependent_stale() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let answer = add(&store, &fake, new_node("answer", vec![])).unwrap();
        respond(&store, &fake, &answer, "use option A", Author::Human).unwrap();
        let consumer = add(&store, &fake, new_node("consumer", vec![answer.clone()])).unwrap();
        done(&store, &fake, &consumer);
        assert!(staleness(&store, &fake, &consumer).unwrap().is_empty());

        respond(&store, &fake, &answer, "use option B", Author::Human).unwrap();
        let reasons = staleness(&store, &fake, &consumer).unwrap();
        assert_eq!(
            reasons,
            vec![StalenessReason::ConsumedResultChanged { id: answer }]
        );
        assert!(node_state(&store, &fake, &consumer).unwrap().is_ready());
        let state = node_state(&store, &fake, &consumer).unwrap();
        assert_eq!(state.currency, Currency::Stale);
        assert!(
            !state.is_complete(),
            "changed consumed evidence invalidates success"
        );
    }

    #[test]
    fn context_drift_makes_node_stale() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();

        std::fs::write(store.project_root().join("helper.rs"), "v1").unwrap();
        complete(
            &store,
            &fake,
            &id,
            &[],
            &["helper.rs".into()],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        assert!(staleness(&store, &fake, &id).unwrap().is_empty());

        std::fs::write(store.project_root().join("helper.rs"), "v2").unwrap();
        let reasons = staleness(&store, &fake, &id).unwrap();
        assert_eq!(
            reasons,
            vec![StalenessReason::ContextChanged {
                path: "helper.rs".into()
            }]
        );
        assert!(node_state(&store, &fake, &id).unwrap().is_ready());

        std::fs::remove_file(store.project_root().join("helper.rs")).unwrap();
        let reasons = staleness(&store, &fake, &id).unwrap();
        assert_eq!(
            reasons,
            vec![StalenessReason::ContextMissing {
                path: "helper.rs".into()
            }]
        );
    }

    #[test]
    fn own_output_drift_uses_the_vcs() {
        let (_t, store) = temp_store();
        let mut fake = FakeVcs {
            next_id: "commit-x".into(),
            ..Default::default()
        };
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        complete(
            &store,
            &fake,
            &id,
            &["src/x.rs".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        assert!(staleness(&store, &fake, &id).unwrap().is_empty());

        fake.drift_for
            .insert("commit-x".into(), "M\tsrc/x.rs".into());
        let reasons = staleness(&store, &fake, &id).unwrap();
        assert!(matches!(
            reasons.as_slice(),
            [StalenessReason::OutputDrifted { .. }]
        ));
        assert!(node_state(&store, &fake, &id).unwrap().is_ready());
    }

    #[test]
    fn state_errors_are_not_converted_to_graph_facts() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let malformed = add(&store, &fake, new_node("malformed", vec![])).unwrap();
        std::fs::write(store.node_dir(&malformed).join("node.toml"), "not = [toml").unwrap();
        assert!(node_state(&store, &fake, &malformed).is_err());
        assert!(is_ready(&store, &fake, &malformed).is_err());

        let bad_result = add(&store, &fake, new_node("bad result", vec![])).unwrap();
        std::fs::write(
            store.node_dir(&bad_result).join("result.toml"),
            "outcome = ???",
        )
        .unwrap();
        assert!(node_state(&store, &fake, &bad_result).is_err());

        let target = add(&store, &fake, new_node("target", vec![])).unwrap();
        let consumer = add(&store, &fake, new_node("consumer", vec![target.clone()])).unwrap();
        std::fs::remove_dir_all(store.node_dir(&target)).unwrap();
        assert_eq!(
            node_state(&store, &fake, &consumer).unwrap().blockers,
            vec![Blocker {
                id: target.clone(),
                reason: BlockerReason::Missing,
            }]
        );
        std::fs::create_dir_all(store.node_dir(&target)).unwrap();
        std::fs::write(store.node_dir(&target).join("node.toml"), "not = [toml").unwrap();
        std::fs::write(store.node_dir(&target).join("description.md"), "target").unwrap();
        assert!(node_state(&store, &fake, &consumer).is_err());
    }

    #[test]
    fn context_and_artifact_inspection_failures_are_errors() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let context_node = add(&store, &fake, new_node("context", vec![])).unwrap();
        std::fs::write(store.project_root().join("input"), "content").unwrap();
        complete(
            &store,
            &fake,
            &context_node,
            &[],
            &["input".into()],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        std::fs::remove_file(store.project_root().join("input")).unwrap();
        std::fs::create_dir(store.project_root().join("input")).unwrap();
        assert!(node_state(&store, &fake, &context_node).is_err());

        let failing_vcs = FakeVcs {
            next_id: "output".into(),
            drift_error: Some("artifact backend unavailable".into()),
            ..Default::default()
        };
        let output_node = add(&store, &failing_vcs, new_node("output", vec![])).unwrap();
        complete(
            &store,
            &failing_vcs,
            &output_node,
            &["out".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        let error = node_state(&store, &failing_vcs, &output_node).unwrap_err();
        assert!(format!("{error:#}").contains("artifact backend unavailable"));
    }

    #[cfg(unix)]
    #[test]
    fn context_symlink_cannot_escape_project_root() {
        use std::os::unix::fs::symlink;
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("context", vec![])).unwrap();
        let outside = store.workbench_root().join("outside-secret");
        std::fs::write(&outside, "secret").unwrap();
        symlink(&outside, store.project_root().join("escape")).unwrap();
        let error = complete(
            &store,
            &fake,
            &id,
            &[],
            &["escape".into()],
            None,
            "",
            Author::Human,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("escapes the project root"));
    }

    #[test]
    fn blockers_follow_dependency_status() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();

        // A not done -> B blocked, not ready.
        let blocked = blockers(&store, &fake, &b).unwrap();
        assert_eq!(
            blocked,
            vec![Blocker {
                id: a.clone(),
                reason: BlockerReason::Open
            }]
        );
        assert!(!is_ready(&store, &fake, &b).unwrap());

        // A done -> B ready.
        done(&store, &fake, &a);
        assert!(blockers(&store, &fake, &b).unwrap().is_empty());
        assert!(is_ready(&store, &fake, &b).unwrap());

        // A edited after done -> reopened -> B blocked again.
        edit(&store, &fake, &a, "revised".into()).unwrap();
        let blocked = blockers(&store, &fake, &b).unwrap();
        assert_eq!(
            blocked,
            vec![Blocker {
                id: a,
                reason: BlockerReason::Stale
            }]
        );
    }

    #[test]
    fn failed_node_is_ready_to_retry_but_blocks_dependents() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();

        fail(&store, &fake, &a, "build broke", Author::Human).unwrap();
        assert_eq!(current_status(&store, &fake, &a).unwrap(), Status::Failed);
        assert!(
            is_ready(&store, &fake, &a).unwrap(),
            "a failed node can be retried"
        );
        assert!(
            !is_ready(&store, &fake, &b).unwrap(),
            "its dependents stay blocked"
        );

        // Retry succeeds: the result is overwritten, B unblocks.
        done(&store, &fake, &a);
        assert_eq!(current_status(&store, &fake, &a).unwrap(), Status::Done);
        assert!(is_ready(&store, &fake, &b).unwrap());
    }

    #[test]
    fn stale_node_with_incomplete_dependency_is_blocked_and_blocks_dependents() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
        done(&store, &fake, &dependency);
        let stale = add(
            &store,
            &fake,
            new_node("consumer", vec![dependency.clone()]),
        )
        .unwrap();
        done(&store, &fake, &stale);
        let dependent = add(&store, &fake, new_node("dependent", vec![stale.clone()])).unwrap();

        edit(&store, &fake, &dependency, "changed dependency".into()).unwrap();

        let stale_state = node_state(&store, &fake, &stale).unwrap();
        assert_eq!(stale_state.currency, Currency::Stale);
        assert!(stale_state.is_blocked());
        assert_eq!(
            stale_state.blockers,
            vec![Blocker {
                id: dependency,
                reason: BlockerReason::Stale,
            }]
        );
        let dependent_state = node_state(&store, &fake, &dependent).unwrap();
        assert!(dependent_state.is_blocked());
        assert_eq!(
            dependent_state.blockers,
            vec![Blocker {
                id: stale,
                reason: BlockerReason::Stale,
            }]
        );
    }

    #[test]
    fn origin_maps_a_commit_back_to_its_node() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            next_id: "commit-xyz".into(),
            ..Default::default()
        };
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        add(&store, &fake, new_node("other", vec![])).unwrap();
        complete(
            &store,
            &fake,
            &a,
            &["src/x.rs".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();

        assert_eq!(origin(&store, "commit-xyz").unwrap(), Some(a));
        assert_eq!(origin(&store, "no-such-commit").unwrap(), None);
    }

    #[test]
    fn dependents_scans_both_lists() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        let mut c = new_node("c", vec![]);
        c.derived_from = vec![a.clone()];
        let c = add(&store, &fake, c).unwrap();
        add(&store, &fake, new_node("unrelated", vec![])).unwrap();

        let mut deps = dependents(&store, &a).unwrap();
        deps.sort();
        let mut expected = vec![b, c];
        expected.sort();
        assert_eq!(deps, expected);
    }

    #[test]
    fn link_rejects_unknown_and_duplicate_targets() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![])).unwrap();

        assert!(link(&store, &fake, &a, "node-nope", DepKind::DependsOn).is_err());
        link(&store, &fake, &a, &b, DepKind::DependsOn).unwrap();
        assert!(link(&store, &fake, &a, &b, DepKind::DependsOn).is_err());

        let (meta, _) = store.read_node(&a).unwrap();
        assert_eq!(meta.depends_on, vec![b.parse().unwrap()]);
    }

    #[test]
    fn check_reports_sideways_damage() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();

        // A healthy little graph passes.
        let node = add(&store, &fake, new_node("a", vec![])).unwrap();
        let dep = add(&store, &fake, new_node("b", vec![node.clone()])).unwrap();
        assert!(check(&store).unwrap().is_empty());

        // Damage entered "sideways" (direct writes, as a hand edit or merge would):
        // give `node` a self-reference, a duplicate, and a missing target.
        let (mut meta, body) = store.read_node(&node).unwrap();
        meta.depends_on = vec![
            node.parse().unwrap(),
            node.parse().unwrap(),
            "node-gone".parse().unwrap(),
        ];
        store.write_node(&node, &meta, &body).unwrap();

        let problems = check(&store).unwrap();
        let all = problems.join("\n");
        assert!(all.contains("refers to the node itself"), "{all}");
        assert!(all.contains("duplicate depends_on entry"), "{all}");
        assert!(all.contains("missing or unreadable"), "{all}");
        assert!(
            all.contains(&format!("dependency cycle: {node} -> {node}")),
            "{all}"
        );

        // An unparseable file is reported, not a crash.
        std::fs::write(store.node_dir(&dep).join("node.toml"), "not = valid = toml").unwrap();
        let problems = check(&store).unwrap();
        assert!(
            problems.iter().any(|p| p.contains("unreadable definition")),
            "{problems:?}"
        );
    }

    #[test]
    fn check_rejects_semantically_impossible_results() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let dependency = add(&store, &fake, new_node("dependency", vec![])).unwrap();
        done(&store, &fake, &dependency);
        let consumer = add(&store, &fake, new_node("consumer", vec![dependency])).unwrap();
        done(&store, &fake, &consumer);

        let (mut result, notes) = store.read_result(&consumer).unwrap().unwrap();
        result.schema = 99;
        result.consumed[0].outcome = None;
        result.consumed.push(result.consumed[0].clone());
        result.consumed.push(ConsumedNode {
            id: "undeclared".parse().unwrap(),
            definition: result.definition.clone(),
            result: None,
            outcome: None,
            output: Some(ArtifactRef {
                scheme: "unknown".into(),
                repository: String::new(),
                id: "artifact".into(),
            }),
        });
        result.context.push(crate::model::ContextPin {
            path: "input".parse().unwrap(),
            identity: "one".into(),
            observed: false,
        });
        result.context.push(crate::model::ContextPin {
            path: "input".parse().unwrap(),
            identity: "two".into(),
            observed: false,
        });
        store.write_result(&consumer, &result, &notes).unwrap();

        let problems = check(&store).unwrap().join("\n");
        assert!(problems.contains("unsupported result schema"));
        assert!(problems.contains("duplicate consumed-node pin"));
        assert!(problems.contains("no declared edge"));
        assert!(problems.contains("no successful evidence"));
        assert!(problems.contains("duplicate context pin"));
        assert!(problems.contains("unsupported artifact scheme"));
    }

    #[test]
    fn check_finds_multi_node_cycles() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        // Close the loop sideways: a -> b (write-time link would allow a -> b;
        // the *cycle* is only visible to check).
        let (mut meta, body) = store.read_node(&a).unwrap();
        meta.depends_on = vec![b.parse().unwrap()];
        store.write_node(&a, &meta, &body).unwrap();

        let problems = check(&store).unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("dependency cycle: "),
            "{problems:?}"
        );
        assert!(problems[0].contains(&a) && problems[0].contains(&b));
    }

    #[test]
    fn settled_requires_the_whole_derived_branch_to_be_done_and_fresh() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            next_id: "commit-1".into(),
            ..Default::default()
        };
        // Root -> sub-task (derived) -> implementation (depends on the sub-task).
        let root = add(&store, &fake, new_node("idea", vec![])).unwrap();
        let mut sub = new_node("sub", vec![]);
        sub.derived_from = vec![root.clone()];
        let sub = add(&store, &fake, sub).unwrap();
        let imp = add(&store, &fake, new_node("impl", vec![sub.clone()])).unwrap();

        // Root done (spawned the sub-task), sub done (spec settled), impl open:
        // root is done, but not settled — the derived branch is unfinished.
        done(&store, &fake, &root);
        done(&store, &fake, &sub);
        let reasons = unsettled(&store, &fake, &root).unwrap();
        assert_eq!(reasons, vec![format!("{imp}: not done (open)")]);

        // Implementation lands: the whole branch is settled.
        complete(
            &store,
            &fake,
            &imp,
            &["src/x.rs".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        assert!(unsettled(&store, &fake, &root).unwrap().is_empty());
        assert!(
            unsettled(&store, &fake, &imp).unwrap().is_empty(),
            "leaves settle too"
        );

        // Editing the sub-task makes both its success and the implementation
        // that consumed it stale.
        edit(&store, &fake, &sub, "revised".into()).unwrap();
        let reasons = unsettled(&store, &fake, &root).unwrap();
        assert!(
            reasons.contains(&format!("{sub}: done but stale")),
            "{reasons:?}"
        );
        assert!(
            reasons.contains(&format!("{imp}: done but stale")),
            "{reasons:?}"
        );
    }

    #[test]
    fn link_rejects_self_reference() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        assert!(link(&store, &fake, &a, &a, DepKind::DependsOn).is_err());
    }

    #[test]
    fn assignee_round_trips_through_add() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let mut question = new_node("Question: which auth scheme?", vec![]);
        question.author = Author::Machine;
        question.assignee = Some(Author::Human);
        let q = add(&store, &fake, question).unwrap();

        let (meta, _) = store.read_node(&q).unwrap();
        assert_eq!(meta.assignee, Some(Author::Human));

        // Unassigned nodes stay unassigned (and omit the key on disk).
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let (meta, _) = store.read_node(&a).unwrap();
        assert_eq!(meta.assignee, None);
        let text = std::fs::read_to_string(store.node_dir(&a).join("node.toml")).unwrap();
        assert!(!text.contains("assignee"), "{text}");
    }

    #[test]
    fn respond_completes_despite_a_dirty_tree_and_pins_dependencies() {
        let (_t, store) = temp_store();
        // Groundwork lands on a clean tree; then the tree goes dirty with
        // whatever prompted the question — the normal state when a question
        // node is answered.
        let mut dirty = FakeVcs::default();
        let dep = add(&store, &dirty, new_node("groundwork", vec![])).unwrap();
        done(&store, &dirty, &dep);
        dirty.dirty.push("PROPOSAL.md".into());
        let mut question = new_node("Question: which concept?", vec![dep.clone()]);
        question.author = Author::Machine;
        question.assignee = Some(Author::Human);
        let q = add(&store, &dirty, question).unwrap();

        assert!(
            respond(&store, &dirty, &q, "  ", Author::Human).is_err(),
            "needs text"
        );
        respond(&store, &dirty, &q, "concept A", Author::Human).unwrap();

        assert_eq!(current_status(&store, &dirty, &q).unwrap(), Status::Done);
        let (result, notes) = store.read_result(&q).unwrap().unwrap();
        assert_eq!(notes, "concept A");
        assert_eq!(result.author, Author::Human);
        assert_eq!(result.output, None);
        assert_eq!(result.consumed.len(), 1, "the answer pins its dependencies");
        assert!(
            dirty.captured.borrow().is_empty(),
            "no output commit is minted"
        );

        // Editing the question afterwards invalidates the answer as usual.
        edit(&store, &dirty, &q, "Question: revised".into()).unwrap();
        assert_eq!(current_status(&store, &dirty, &q).unwrap(), Status::Open);
    }

    #[test]
    fn a_dirty_project_tree_blocks_only_completion() {
        let (_t, store) = temp_store();
        // The project tree is mid-hack: uncommitted changes unrelated to any node.
        let dirty = FakeVcs {
            dirty: vec!["src/x.rs".into()],
            ..Default::default()
        };

        // Pure graph edits never gate on project state.
        let a = add(&store, &dirty, new_node("a", vec![])).unwrap();
        let b = add(&store, &dirty, new_node("b", vec![])).unwrap();
        link(&store, &dirty, &b, &a, DepKind::DependsOn).unwrap();
        edit(&store, &dirty, &a, "revised".into()).unwrap();
        commit_work_log(&store, &dirty, &a).unwrap();

        // A failed attempt may have left the mess; recording it must not block.
        fail(&store, &dirty, &a, "broke", Author::Human).unwrap();

        // Completion asserts output provenance: the undeclared write is refused,
        // and declaring it is what unblocks the completion.
        assert!(complete(&store, &dirty, &a, &[], &[], None, "", Author::Human).is_err());
        complete(
            &store,
            &dirty,
            &a,
            &["src/x.rs".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();
    }

    #[test]
    fn require_clean_except_allows_exactly_the_declared_outputs() {
        let outputs = vec!["src/out.rs".to_string()];
        let ok = FakeVcs {
            dirty: vec!["src/out.rs".into()],
            ..Default::default()
        };
        assert!(require_clean_except(&ok, &outputs).is_ok());
        assert!(require_clean_except(&FakeVcs::default(), &outputs).is_ok());

        let stray = FakeVcs {
            dirty: vec!["src/out.rs".into(), "src/other.rs".into()],
            ..Default::default()
        };
        assert!(require_clean_except(&stray, &outputs).is_err());
    }

    #[test]
    fn pair_records_the_root_and_is_idempotent() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            root: Some("root-1".into()),
            ..Default::default()
        };

        let pairing = pair(&store, &fake, None, false).unwrap();
        assert_eq!(pairing.root_commit, "root-1");
        assert_eq!(*fake.store_commits.borrow(), 1);

        // Same root again: no re-write, no extra store commit.
        let again = pair(&store, &fake, None, false).unwrap();
        assert_eq!(again.root_commit, "root-1");
        assert_eq!(*fake.store_commits.borrow(), 1);
    }

    #[test]
    fn pair_records_and_refreshes_the_informational_fields() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            root: Some("root-1".into()),
            remote: Some("git@host:me/p.git".into()),
            ..Default::default()
        };

        // Name from the caller, remote observed from the repo.
        let pairing = pair(&store, &fake, Some("splurt".into()), false).unwrap();
        assert_eq!(pairing.name.as_deref(), Some("splurt"));
        assert_eq!(pairing.remote.as_deref(), Some("git@host:me/p.git"));
        let at = pairing.paired_at;

        // Same root, new name: the info updates without touching the identity
        // or its timestamp; one extra store commit records it.
        let renamed = pair(&store, &fake, Some("splurt-2".into()), false).unwrap();
        assert_eq!(renamed.name.as_deref(), Some("splurt-2"));
        assert_eq!(renamed.paired_at, at);
        assert_eq!(*fake.store_commits.borrow(), 2);

        // A repo whose remote vanished keeps the last-known one; nothing to
        // update, no commit.
        let no_remote = FakeVcs {
            root: Some("root-1".into()),
            ..Default::default()
        };
        let kept = pair(&store, &no_remote, None, false).unwrap();
        assert_eq!(kept.remote.as_deref(), Some("git@host:me/p.git"));
        assert_eq!(kept.name.as_deref(), Some("splurt-2"));
        assert_eq!(*no_remote.store_commits.borrow(), 0);
    }

    #[test]
    fn pair_refuses_an_empty_project_and_a_different_root_without_force() {
        let (_t, store) = temp_store();
        assert!(
            pair(&store, &FakeVcs::default(), None, false).is_err(),
            "no commits"
        );

        let first = FakeVcs {
            root: Some("root-1".into()),
            ..Default::default()
        };
        pair(&store, &first, None, false).unwrap();

        let other = FakeVcs {
            root: Some("root-2".into()),
            ..Default::default()
        };
        assert!(
            pair(&store, &other, None, false).is_err(),
            "mismatched root"
        );
        // A deliberate re-pair (history rewrite) goes through with force.
        assert_eq!(
            pair(&store, &other, None, true).unwrap().root_commit,
            "root-2"
        );
    }

    #[test]
    fn verify_pairing_reports_unpaired_matching_and_mismatched() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            root: Some("root-1".into()),
            ..Default::default()
        };

        // Unpaired: no problems, no recorded pairing.
        let (recorded, problems) = verify_pairing(&store, &fake, false).unwrap();
        assert!(recorded.is_none());
        assert!(problems.is_empty());

        pair(&store, &fake, None, false).unwrap();
        let (recorded, problems) = verify_pairing(&store, &fake, false).unwrap();
        assert_eq!(recorded.unwrap().root_commit, "root-1");
        assert!(problems.is_empty());

        let moved = FakeVcs {
            root: Some("root-2".into()),
            ..Default::default()
        };
        let (_, problems) = verify_pairing(&store, &moved, false).unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("root-2"), "{}", problems[0]);

        let empty = FakeVcs::default();
        let (_, problems) = verify_pairing(&store, &empty, false).unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("no commits"), "{}", problems[0]);
    }

    #[test]
    fn deep_verify_finds_orphaned_output_commits() {
        let (_t, store) = temp_store();
        let fake = FakeVcs {
            root: Some("root-1".into()),
            next_id: "commit-1".into(),
            ..Default::default()
        };
        pair(&store, &fake, None, false).unwrap();

        // A completes with an output commit; B is built against it.
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        std::fs::write(store.project_root().join("out.rs"), "x").unwrap();
        complete(
            &store,
            &fake,
            &a,
            &["out.rs".into()],
            &[],
            None,
            "",
            Author::Human,
        )
        .unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        complete(&store, &fake, &b, &[], &[], None, "", Author::Human).unwrap();

        // The commit exists: deep verify is clean.
        let (_, problems) = verify_pairing(&store, &fake, true).unwrap();
        assert!(problems.is_empty(), "{problems:?}");

        // A history rewrite drops the commit: both the output and the
        // built-against pin are reported.
        fake.commits.borrow_mut().clear();
        let (_, problems) = verify_pairing(&store, &fake, true).unwrap();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems
            .iter()
            .any(|p| p.starts_with(&a) && p.contains("output commit")));
        assert!(problems
            .iter()
            .any(|p| p.starts_with(&b) && p.contains("built-against")));
    }
}
