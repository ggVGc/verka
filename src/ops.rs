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
    Author, BuiltAgainst, ContextPin, DefinitionVersion, DepKind, NodeMeta, Outcome, ResultMeta,
    ResultVersion, Status, WorkedBy,
};
use crate::store::{file_blob, Store};
use crate::vcs::Vcs;

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
        bail!("a node needs a description (its first line serves as the title)");
    }
    for dep in new.depends_on.iter().chain(&new.derived_from) {
        check_edge(store, dep)?;
    }
    let id = format!("node-{}", Ulid::new());
    let meta = NodeMeta {
        schema: 1,
        author: new.author,
        assignee: new.assignee,
        depends_on: new.depends_on,
        derived_from: new.derived_from,
    };
    store.write_node(&id, &meta, &new.description)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: add {id}"))?;
    Ok(id)
}

/// Add `to` to one of `from`'s dependency lists. A definition change: it moves
/// `from`'s version.
pub fn link(store: &Store, vcs: &dyn Vcs, from: &str, to: &str, kind: DepKind) -> Result<()> {
    if from == to {
        bail!("cannot link `{from}` to itself");
    }
    let (mut meta, description) = store.read_node(from)?;
    check_edge(store, to)?;
    let list = match kind {
        DepKind::DependsOn => &mut meta.depends_on,
        DepKind::DerivedFrom => &mut meta.derived_from,
    };
    if list.iter().any(|d| d == to) {
        bail!("{from} already has a {} link to {to}", kind.as_str());
    }
    list.push(to.to_string());
    store.write_node(from, &meta, &description)?;
    vcs.commit_store(
        &store.store_name(),
        &format!("llaundry: link {from} -> {to}"),
    )?;
    Ok(())
}

/// Edit a node's description. A definition change: it moves the node's
/// version, so a prior `done` no longer covers it and dependents' pins go stale.
pub fn edit(store: &Store, vcs: &dyn Vcs, id: &str, description: String) -> Result<()> {
    if description.trim().is_empty() {
        bail!("a node needs a description (its first line serves as the title)");
    }
    let (meta, _) = store.read_node(id)?;
    store.write_node(id, &meta, &description)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: edit {id}"))?;
    Ok(())
}

/// Complete a node's work: commit all produced files as one output commit, pin
/// what the work was built against (dependency versions and outputs, plus any
/// extra context files), and record it all in `result.toml` and `result.md`.
/// commit, or `None` when the work produced no files (graph-only work).
#[allow(clippy::too_many_arguments)] // mirrors the CLI/MCP surface one-to-one
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
    // The only uncommitted project changes allowed are the outputs we are about
    // to commit — completion is where output provenance is asserted.
    require_clean_except(vcs, outputs)?;
    let (meta, description) = store.read_node(id)?;

    // Pin everything the work saw, before committing anything.
    let context = pin_context(store, context)?;
    let built_against = pin_deps(store, &meta)?;

    let output_commit = if outputs.is_empty() {
        None
    } else {
        let message = message.unwrap_or_else(|| crate::model::title_of(&description).to_string());
        Some(vcs.capture(outputs, &message)?)
    };

    store.write_result(
        id,
        &ResultMeta {
            at: now_millis(),
            author,
            definition: store.node_version(id)?,
            outcome: Outcome::Done,
            output_commit: output_commit.clone(),
            worked_by: None,
            built_against,
            context,
        },
        notes,
    )?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: complete {id}"))?;
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
    let built_against = pin_deps(store, &meta)?;
    store.write_result(
        id,
        &ResultMeta {
            at: now_millis(),
            author,
            definition: store.node_version(id)?,
            outcome: Outcome::Done,
            output_commit: None,
            worked_by: None,
            built_against,
            context: Vec::new(),
        },
        notes,
    )?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: respond {id}"))?;
    Ok(())
}

/// Record that a node's work was attempted and failed. Like [`complete`] it pins
/// what the attempt was built against, so the failure is reproducible evidence.
/// It does not gate on project-tree cleanliness: a failed attempt may well have
/// left a mess, and recording the failure must not be blocked by it.
pub fn fail(store: &Store, vcs: &dyn Vcs, id: &str, notes: &str, author: Author) -> Result<()> {
    let (meta, _) = store.read_node(id)?;
    let built_against = pin_deps(store, &meta)?;
    store.write_result(
        id,
        &ResultMeta {
            at: now_millis(),
            author,
            definition: store.node_version(id)?,
            outcome: Outcome::Failed,
            output_commit: None,
            worked_by: None,
            built_against,
            context: Vec::new(),
        },
        notes,
    )?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: fail {id}"))?;
    Ok(())
}

/// Commit whatever of the node's streamed interaction log (`work.jsonl`) is not
/// yet in git. The log is written line by line *during* a session, dirtying only
/// the workbench repository, and each store commit the session makes already
/// sweeps the story-so-far in; this picks up the tail after the session ends.
/// A no-op when the log is fully committed.
pub fn commit_work_log(store: &Store, vcs: &dyn Vcs, id: &str) -> Result<()> {
    vcs.commit_store(&store.store_name(), &format!("llaundry: work log {id}"))?;
    Ok(())
}

/// Append *observed* context pins to a node's recorded result: files a work
/// session was seen reading (mined from its recorded transcript) that the
/// worker did not declare in `complete`. Turns input provenance from agent
/// discipline into a derived fact.
///
/// Skips paths already pinned, paths inside any node's output commit (those
/// are covered by `built_against` pins — context is for files no node
/// produced), and files that no longer exist. Returns how many pins were
/// added; a no-op when the node has no result yet (a paused unit's reads are
/// amended once it completes, from the full replayed log).
pub fn amend_context(store: &Store, vcs: &dyn Vcs, id: &str, reads: &[String]) -> Result<usize> {
    let Some((mut result, notes)) = store.read_result(id)? else {
        return Ok(0);
    };

    let mut node_outputs = std::collections::HashSet::new();
    for other in store.list_ids()? {
        if let Some(commit) = output_of(store, &other) {
            node_outputs.extend(vcs.files_in(&commit)?);
        }
    }

    let root = store.project_root();
    let mut pinned: std::collections::HashSet<String> =
        result.context.iter().map(|p| p.path.clone()).collect();
    let mut added = 0;
    for path in reads {
        if pinned.contains(path) || node_outputs.contains(path) {
            continue;
        }
        let Some(blob) = file_blob(&root.join(path)) else {
            continue;
        };
        pinned.insert(path.clone());
        result.context.push(ContextPin {
            path: path.clone(),
            blob,
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
        &format!("llaundry: observed context {id}"),
    )?;
    Ok(added)
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
    result.worked_by = Some(worked_by);
    store.write_result(id, &result, &notes)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: worked by {id}"))?;
    Ok(true)
}

/// A node's derived status.
///
/// `done` holds only while the result's definition version still matches:
/// editing the definition after completion reopens the node, because the
/// completion no longer certifies the current content.
pub fn current_status(store: &Store, id: &str) -> Status {
    match store.read_result(id) {
        Ok(Some((r, _))) => match r.outcome {
            Outcome::Failed => Status::Failed,
            Outcome::Done => {
                if store.node_version(id).ok().as_ref() == Some(&r.definition) {
                    Status::Done
                } else {
                    Status::Open
                }
            }
        },
        _ => Status::Open,
    }
}

/// Collect explicit reasons a node's recorded work is stale, if any. A node with
/// no result cannot be stale (there is no work to invalidate).
///
/// Independent sources of staleness:
///   * a pinned dependency whose definition has moved (or that vanished),
///   * a pinned dependency whose output commit has changed since,
///   * a pinned context file whose content has drifted,
///   * the node's own outputs changed since its output commit, and
///   * the definition edited after completion (which also reopens the node).
pub fn staleness(store: &Store, vcs: &dyn Vcs, id: &str) -> Vec<String> {
    let mut reasons = Vec::new();
    let Ok(Some((result, _))) = store.read_result(id) else {
        return reasons;
    };

    if let Ok(current) = store.node_version(id) {
        definition_drift(
            "definition changed since the work",
            &result.definition,
            &current,
            &mut reasons,
        );
    } else {
        reasons.push("definition missing or unreadable since the work".into());
    }

    for ba in &result.built_against {
        match store.node_version(&ba.id) {
            Ok(current) => definition_drift(
                &format!("dependency {}: definition moved", ba.id),
                &ba.definition,
                &current,
                &mut reasons,
            ),
            Err(_) => reasons.push(format!("dependency {}: missing", ba.id)),
        }
        if store.result_version(&ba.id).ok() != ba.result {
            reasons.push(format!(
                "dependency {}: result changed since it was consumed",
                ba.id
            ));
        }
        let current_output = output_of(store, &ba.id);
        if current_output != ba.output {
            reasons.push(format!(
                "dependency {}: output changed (built against {}, now {})",
                ba.id,
                ba.output.as_deref().map_or("none", short),
                current_output.as_deref().map_or("none", short)
            ));
        }
    }

    let root = store.project_root();
    for pin in &result.context {
        match file_blob(&root.join(&pin.path)) {
            Some(now) if now != pin.blob => reasons.push(format!(
                "context {}: content changed (pinned {}, now {})",
                pin.path,
                short(&pin.blob),
                short(&now)
            )),
            Some(_) => {}
            None => reasons.push(format!(
                "context {}: missing (pinned {})",
                pin.path,
                short(&pin.blob)
            )),
        }
    }

    if let Some(commit) = &result.output_commit {
        match vcs.drift(commit) {
            Ok(Some(drift)) => reasons.push(format!(
                "output changed since {}:\n      {}",
                short(commit),
                drift.replace('\n', "\n      ")
            )),
            Ok(None) => {}
            Err(e) => reasons.push(format!("output check failed ({}): {e}", short(commit))),
        }
    }

    reasons
}

/// Reasons a node's `depends_on` dependencies are unsatisfied — empty means the
/// node can be worked. A dependency is satisfied only if it is `done` (on its
/// current definition) and its own recorded work is not stale.
pub fn blockers(store: &Store, vcs: &dyn Vcs, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok((meta, _)) = store.read_node(id) else {
        return out;
    };
    for dep in &meta.depends_on {
        if !store.exists(dep) {
            out.push(format!("{dep}: missing"));
            continue;
        }
        match current_status(store, dep) {
            Status::Done => {
                if !staleness(store, vcs, dep).is_empty() {
                    out.push(format!("{dep}: stale"));
                }
            }
            other => out.push(format!("{dep}: not done ({})", other.as_str())),
        }
    }
    out
}

/// Whether a node is ready to be worked: not already done, and no unsatisfied
/// dependencies. (A failed node is ready again — its work can be retried.)
pub fn is_ready(store: &Store, vcs: &dyn Vcs, id: &str) -> bool {
    current_status(store, id) != Status::Done && blockers(store, vcs, id).is_empty()
}

/// The node whose work produced `commit`, if any — the inverse of the
/// `output_commit` on each result, derived by scanning rather than persisted as
/// a second index. Unique because each completion mints one commit for one node.
pub fn origin(store: &Store, commit: &str) -> Result<Option<String>> {
    for id in store.list_ids()? {
        if let Some((result, _)) = store.read_result(&id)? {
            if result.output_commit.as_deref() == Some(commit) {
                return Ok(Some(id));
            }
        }
    }
    Ok(None)
}

/// A node's current output commit: what its recorded work produced. `None` if it
/// has no result or the work produced no files.
pub fn output_of(store: &Store, id: &str) -> Option<String> {
    store
        .read_result(id)
        .ok()
        .flatten()
        .and_then(|(r, _)| r.output_commit)
}

/// Ids of nodes that name `id` in either dependency list.
pub fn dependents(store: &Store, id: &str) -> Result<Vec<String>> {
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
            .any(|d| d == id)
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
            rev.entry(dep.clone()).or_default().push(other.clone());
        }
    }

    let mut reasons = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([id.to_string()]);
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        match current_status(store, &node) {
            Status::Done => {
                if !staleness(store, vcs, &node).is_empty() {
                    reasons.push(format!("{node}: done but stale"));
                }
            }
            other => reasons.push(format!("{node}: not done ({})", other.as_str())),
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
        if let Err(e) = store.read_result(&id) {
            problems.push(format!("{id}: unreadable result ({e:#})"));
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
                if dep == &id {
                    problems.push(format!("{id}: {kind} refers to the node itself"));
                    continue;
                }
                if store.read_node(dep).is_err() {
                    problems.push(format!("{id}: {kind} target `{dep}` missing or unreadable"));
                }
            }
        }
        depends_on.insert(id, meta.depends_on);
    }

    problems.extend(find_cycles(&depends_on));
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

/// Validate that an edge target exists.
fn check_edge(store: &Store, to: &str) -> Result<()> {
    store
        .read_node(to)
        .with_context(|| format!("cannot link to unknown node `{to}`"))?;
    Ok(())
}

/// Pin the current version and output of every node in `meta`'s dependency lists.
fn pin_deps(store: &Store, meta: &NodeMeta) -> Result<Vec<BuiltAgainst>> {
    meta.depends_on
        .iter()
        .chain(&meta.derived_from)
        .map(|dep| {
            let definition = store
                .node_version(dep)
                .with_context(|| format!("cannot pin unknown dependency `{dep}`"))?;
            let result = store.result_version(dep).ok();
            Ok(BuiltAgainst {
                id: dep.clone(),
                definition,
                result,
                output: output_of(store, dep),
            })
        })
        .collect()
}

fn definition_drift(
    prefix: &str,
    pinned: &DefinitionVersion,
    current: &DefinitionVersion,
    reasons: &mut Vec<String>,
) {
    if pinned.metadata != current.metadata {
        reasons.push(format!(
            "{prefix}: node.toml changed (built against {}, now {})",
            short(&pinned.metadata),
            short(&current.metadata)
        ));
    }
    if pinned.description != current.description {
        reasons.push(format!(
            "{prefix}: description.md changed (built against {}, now {})",
            short(&pinned.description),
            short(&current.description)
        ));
    }
}

/// Pin each context path by its current content; errors if a file is missing.
fn pin_context(store: &Store, paths: &[String]) -> Result<Vec<ContextPin>> {
    let root = store.project_root();
    paths
        .iter()
        .map(|path| {
            let blob = file_blob(&root.join(path))
                .with_context(|| format!("cannot pin `{path}`: file not found"))?;
            Ok(ContextPin {
                path: path.clone(),
                blob,
                observed: false,
            })
        })
        .collect()
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
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
        let root = std::env::temp_dir().join(format!("llaundry-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::init(root.join(".llaundry")).unwrap();
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
        complete(store, vcs, id, &[], &[], None, "done", Author::Machine).unwrap();
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
            Author::Machine,
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
    fn amend_worker_stamps_only_this_sessions_result() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        let engine = || WorkedBy {
            backend: "claude-code".into(),
            model: Some("opus".into()),
        };

        // No result yet (a paused unit of work): nothing to stamp.
        assert!(!amend_worker(&store, &fake, &id, engine(), 0).unwrap());

        done(&store, &fake, &id);
        let at = store.read_result(&id).unwrap().unwrap().0.at;

        // A result older than the session is someone else's — left untouched.
        assert!(!amend_worker(&store, &fake, &id, engine(), at + 1).unwrap());
        assert!(store
            .read_result(&id)
            .unwrap()
            .unwrap()
            .0
            .worked_by
            .is_none());

        // This session's result gets the stamp, keeping the narrative.
        assert!(amend_worker(&store, &fake, &id, engine(), at).unwrap());
        let (result, notes) = store.read_result(&id).unwrap().unwrap();
        assert_eq!(notes, "done");
        let wb = result.worked_by.unwrap();
        assert_eq!(wb.backend, "claude-code");
        assert_eq!(wb.model.as_deref(), Some("opus"));

        // The stamp does not reopen the node or make it stale.
        assert_eq!(current_status(&store, &id), Status::Done);
        assert!(staleness(&store, &fake, &id).is_empty());
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
        assert!(staleness(&store, &fake, &id).is_empty());

        std::fs::write(root.join("read.txt"), "v2").unwrap();
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons.iter().any(|r| r.contains("read.txt")),
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
        assert_eq!(current_status(&store, &id), Status::Open);
        assert!(
            staleness(&store, &fake, &id).is_empty(),
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
            Author::Machine,
        )
        .unwrap();
        assert_eq!(commit.as_deref(), Some("commit-abc"));
        assert_eq!(current_status(&store, &id), Status::Done);
        assert_eq!(output_of(&store, &id).as_deref(), Some("commit-abc"));

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
            Author::Machine,
        )
        .unwrap();
        assert_eq!(commit, None);
        assert_eq!(current_status(&store, &id), Status::Done);
        assert!(fake.captured.borrow().is_empty(), "nothing captured");
    }

    #[test]
    fn editing_a_done_node_reopens_it() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        done(&store, &fake, &id);
        assert_eq!(current_status(&store, &id), Status::Done);

        edit(&store, &fake, &id, "revised".into()).unwrap();
        assert_eq!(current_status(&store, &id), Status::Open);
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("definition changed since the work")),
            "{reasons:?}"
        );
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

        assert_eq!(current_status(&store, &id), Status::Open);
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons.iter().any(|r| r.contains("node.toml changed")),
            "{reasons:?}"
        );
    }

    #[test]
    fn dependency_definition_move_makes_dependent_stale() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        done(&store, &fake, &a);
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        done(&store, &fake, &b);
        assert!(staleness(&store, &fake, &b).is_empty());

        edit(&store, &fake, &a, "revised".into()).unwrap();
        let reasons = staleness(&store, &fake, &b);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&a) && r.contains("definition moved")),
            "{reasons:?}"
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
            Author::Machine,
        )
        .unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        done(&store, &fake, &b);
        assert!(staleness(&store, &fake, &b).is_empty());

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
            Author::Machine,
        )
        .unwrap();
        let reasons = staleness(&store, &fake, &b);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&a) && r.contains("output changed")),
            "{reasons:?}"
        );
    }

    #[test]
    fn dependency_result_notes_change_makes_dependent_stale() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let answer = add(&store, &fake, new_node("answer", vec![])).unwrap();
        respond(&store, &fake, &answer, "use option A", Author::Human).unwrap();
        let consumer = add(&store, &fake, new_node("consumer", vec![answer.clone()])).unwrap();
        done(&store, &fake, &consumer);
        assert!(staleness(&store, &fake, &consumer).is_empty());

        respond(&store, &fake, &answer, "use option B", Author::Human).unwrap();
        let reasons = staleness(&store, &fake, &consumer);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&answer) && r.contains("result changed")),
            "{reasons:?}"
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
            Author::Machine,
        )
        .unwrap();
        assert!(staleness(&store, &fake, &id).is_empty());

        std::fs::write(store.project_root().join("helper.rs"), "v2").unwrap();
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("context helper.rs") && r.contains("content changed")),
            "{reasons:?}"
        );

        std::fs::remove_file(store.project_root().join("helper.rs")).unwrap();
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons
                .iter()
                .any(|r| r.contains("context helper.rs") && r.contains("missing")),
            "{reasons:?}"
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
            Author::Machine,
        )
        .unwrap();
        assert!(staleness(&store, &fake, &id).is_empty());

        fake.drift_for
            .insert("commit-x".into(), "M\tsrc/x.rs".into());
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons.iter().any(|r| r.contains("output changed since")),
            "{reasons:?}"
        );
    }

    #[test]
    fn blockers_follow_dependency_status() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();

        // A not done -> B blocked, not ready.
        let blocked = blockers(&store, &fake, &b);
        assert!(
            blocked.iter().any(|r| r.contains("not done")),
            "{blocked:?}"
        );
        assert!(!is_ready(&store, &fake, &b));

        // A done -> B ready.
        done(&store, &fake, &a);
        assert!(blockers(&store, &fake, &b).is_empty());
        assert!(is_ready(&store, &fake, &b));

        // A edited after done -> reopened -> B blocked again.
        edit(&store, &fake, &a, "revised".into()).unwrap();
        let blocked = blockers(&store, &fake, &b);
        assert!(
            blocked.iter().any(|r| r.contains("not done")),
            "{blocked:?}"
        );
    }

    #[test]
    fn failed_node_is_ready_to_retry_but_blocks_dependents() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();

        fail(&store, &fake, &a, "build broke", Author::Machine).unwrap();
        assert_eq!(current_status(&store, &a), Status::Failed);
        assert!(is_ready(&store, &fake, &a), "a failed node can be retried");
        assert!(!is_ready(&store, &fake, &b), "its dependents stay blocked");

        // Retry succeeds: the result is overwritten, B unblocks.
        done(&store, &fake, &a);
        assert_eq!(current_status(&store, &a), Status::Done);
        assert!(is_ready(&store, &fake, &b));
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
            Author::Machine,
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
        assert_eq!(meta.depends_on, vec![b]);
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
        meta.depends_on = vec![node.clone(), node.clone(), "node-gone".into()];
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
    fn check_finds_multi_node_cycles() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        // Close the loop sideways: a -> b (write-time link would allow a -> b;
        // the *cycle* is only visible to check).
        let (mut meta, body) = store.read_node(&a).unwrap();
        meta.depends_on = vec![b.clone()];
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
            Author::Machine,
        )
        .unwrap();
        assert!(unsettled(&store, &fake, &root).unwrap().is_empty());
        assert!(
            unsettled(&store, &fake, &imp).unwrap().is_empty(),
            "leaves settle too"
        );

        // Editing the sub-task reopens it and flags the branch again, twice over:
        // the sub-task is no longer done, and the impl is done-but-stale.
        edit(&store, &fake, &sub, "revised".into()).unwrap();
        let reasons = unsettled(&store, &fake, &root).unwrap();
        assert!(
            reasons.contains(&format!("{sub}: not done (open)")),
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

        assert_eq!(current_status(&store, &q), Status::Done);
        let (result, notes) = store.read_result(&q).unwrap().unwrap();
        assert_eq!(notes, "concept A");
        assert_eq!(result.author, Author::Human);
        assert_eq!(result.output_commit, None);
        assert_eq!(
            result.built_against.len(),
            1,
            "the answer pins its dependencies"
        );
        assert!(
            dirty.captured.borrow().is_empty(),
            "no output commit is minted"
        );

        // Editing the question afterwards invalidates the answer as usual.
        edit(&store, &dirty, &q, "Question: revised".into()).unwrap();
        assert_eq!(current_status(&store, &q), Status::Open);
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
        fail(&store, &dirty, &a, "broke", Author::Machine).unwrap();

        // Completion asserts output provenance: the undeclared write is refused,
        // and declaring it is what unblocks the completion.
        assert!(complete(&store, &dirty, &a, &[], &[], None, "", Author::Machine).is_err());
        complete(
            &store,
            &dirty,
            &a,
            &["src/x.rs".into()],
            &[],
            None,
            "",
            Author::Machine,
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
}
