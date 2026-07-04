//! Graph operations and derived queries.
//!
//! Every mutating operation ([`add`], [`link`], [`edit`], [`complete`], [`fail`])
//! requires a clean working tree and commits its own store change, so each is its
//! own git commit. The derived queries ([`current_status`], [`staleness`],
//! [`blockers`], [`is_ready`]) recompute from the two files per node and are never
//! stored.
//!
//! All git interaction goes through `&dyn Vcs`, so the whole module is
//! unit-testable with an in-memory fake — no git binary, repository, or identity
//! required. (Blob hashing for versions and pins is computed locally.)

use anyhow::{bail, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

use crate::model::{
    Author, BuiltAgainst, ContextPin, DepKind, NodeMeta, Outcome, ResultMeta, Status,
};
use crate::store::{file_blob, Store};
use crate::vcs::Vcs;

/// Parameters for creating a node with [`add`].
pub struct NewNode {
    pub node_type: crate::model::NodeType,
    pub title: String,
    pub body: String,
    pub author: Author,
    /// Ids this node depends on (must exist).
    pub depends_on: Vec<String>,
    /// Ids this node is derived from (must exist).
    pub derived_from: Vec<String>,
}

/// Create a new node. Returns its id.
pub fn add(store: &Store, vcs: &dyn Vcs, new: NewNode) -> Result<String> {
    require_clean(vcs)?;
    for dep in new.depends_on.iter().chain(&new.derived_from) {
        if !store.exists(dep) {
            bail!("cannot link to unknown node `{dep}`");
        }
    }
    let id = format!("{}-{}", new.node_type.prefix(), Ulid::new());
    let meta = NodeMeta {
        schema: 1,
        node_type: new.node_type,
        title: new.title,
        author: new.author,
        depends_on: new.depends_on,
        derived_from: new.derived_from,
    };
    store.write_node(&id, &meta, &new.body)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: add {id}"))?;
    Ok(id)
}

/// Add `to` to one of `from`'s dependency lists. A definition change: it moves
/// `from`'s version.
pub fn link(store: &Store, vcs: &dyn Vcs, from: &str, to: &str, kind: DepKind) -> Result<()> {
    require_clean(vcs)?;
    if !store.exists(to) {
        bail!("cannot link to unknown node `{to}`");
    }
    let (mut meta, body) = store.read_node(from)?;
    let list = match kind {
        DepKind::DependsOn => &mut meta.depends_on,
        DepKind::DerivedFrom => &mut meta.derived_from,
    };
    if list.iter().any(|d| d == to) {
        bail!("{from} already has a {} link to {to}", kind.as_str());
    }
    list.push(to.to_string());
    store.write_node(from, &meta, &body)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: link {from} -> {to}"))?;
    Ok(())
}

/// Edit a node's title and/or body. A definition change: it moves the node's
/// version, so a prior `done` no longer covers it and dependents' pins go stale.
pub fn edit(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    title: Option<String>,
    body: Option<String>,
) -> Result<()> {
    require_clean(vcs)?;
    let (mut meta, old_body) = store.read_node(id)?;
    if let Some(t) = title {
        meta.title = t;
    }
    let body = body.unwrap_or(old_body);
    store.write_node(id, &meta, &body)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: edit {id}"))?;
    Ok(())
}

/// Complete a node's work: commit all produced files as one output commit, pin
/// what the work was built against (dependency versions and outputs, plus any
/// extra context files), and record it all in `result.md`. Returns the output
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
    // The only uncommitted changes allowed are the outputs we are about to commit.
    require_clean_except(vcs, outputs)?;
    let (meta, _) = store.read_node(id)?;

    // Pin everything the work saw, before committing anything.
    let context = pin_context(store, context)?;
    let built_against = pin_deps(store, &meta)?;

    let output_commit = if outputs.is_empty() {
        None
    } else {
        let message =
            message.unwrap_or_else(|| format!("{}: {}", meta.node_type.as_str(), meta.title));
        Some(vcs.capture(outputs, &message)?)
    };

    store.write_result(
        id,
        &ResultMeta {
            at: now_millis(),
            author,
            node_version: store.node_version(id)?,
            outcome: Outcome::Done,
            output_commit: output_commit.clone(),
            built_against,
            context,
        },
        notes,
    )?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: complete {id}"))?;
    Ok(output_commit)
}

/// Record that a node's work was attempted and failed. Like [`complete`] it pins
/// what the attempt was built against, so the failure is reproducible evidence.
pub fn fail(store: &Store, vcs: &dyn Vcs, id: &str, notes: &str, author: Author) -> Result<()> {
    require_clean(vcs)?;
    let (meta, _) = store.read_node(id)?;
    let built_against = pin_deps(store, &meta)?;
    store.write_result(
        id,
        &ResultMeta {
            at: now_millis(),
            author,
            node_version: store.node_version(id)?,
            outcome: Outcome::Failed,
            output_commit: None,
            built_against,
            context: Vec::new(),
        },
        notes,
    )?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: fail {id}"))?;
    Ok(())
}

/// A node's derived status.
///
/// `done` holds only while the result's `node_version` still matches `node.md`:
/// editing the definition after completion reopens the node, because the
/// completion no longer certifies the current content.
pub fn current_status(store: &Store, id: &str) -> Status {
    match store.read_result(id) {
        Ok(Some((r, _))) => match r.outcome {
            Outcome::Failed => Status::Failed,
            Outcome::Done => {
                if store.node_version(id).ok().as_deref() == Some(r.node_version.as_str()) {
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

    if store.node_version(id).ok().as_deref() != Some(result.node_version.as_str()) {
        reasons.push(format!(
            "definition changed since the work (result covers {}, node.md moved)",
            short(&result.node_version)
        ));
    }

    for ba in &result.built_against {
        match store.node_version(&ba.id) {
            Ok(current) if current != ba.pin => reasons.push(format!(
                "dependency {}: definition moved (built against {}, now {})",
                ba.id,
                short(&ba.pin),
                short(&current)
            )),
            Ok(_) => {}
            Err(_) => reasons.push(format!("dependency {}: missing", ba.id)),
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
        if meta.depends_on.iter().chain(&meta.derived_from).any(|d| d == id) {
            out.push(other);
        }
    }
    Ok(out)
}

/// Enforce a clean working tree before a store operation, so the commit recorded
/// for the resulting state change fully represents the repository.
pub fn require_clean(vcs: &dyn Vcs) -> Result<()> {
    let dirty = vcs.dirty_paths()?;
    if !dirty.is_empty() {
        bail!(
            "working tree is not clean; commit or stash first:\n  {}",
            dirty.join("\n  ")
        );
    }
    Ok(())
}

/// Like [`require_clean`], but tolerates uncommitted changes to `allowed` paths —
/// used by [`complete`], whose job is to commit exactly the produced outputs.
pub fn require_clean_except(vcs: &dyn Vcs, allowed: &[String]) -> Result<()> {
    let allowed: std::collections::HashSet<&str> = allowed.iter().map(String::as_str).collect();
    let stray: Vec<String> = vcs
        .dirty_paths()?
        .into_iter()
        .filter(|p| !allowed.contains(p.as_str()))
        .collect();
    if !stray.is_empty() {
        bail!(
            "uncommitted changes outside the declared outputs; declare or revert them:\n  {}",
            stray.join("\n  ")
        );
    }
    Ok(())
}

/// First 12 characters of a hash, for compact display.
pub fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

/// Pin the current version and output of every node in `meta`'s dependency lists.
fn pin_deps(store: &Store, meta: &NodeMeta) -> Result<Vec<BuiltAgainst>> {
    meta.depends_on
        .iter()
        .chain(&meta.derived_from)
        .map(|dep| {
            let pin = store
                .node_version(dep)
                .with_context(|| format!("cannot pin unknown dependency `{dep}`"))?;
            Ok(BuiltAgainst {
                id: dep.clone(),
                pin,
                output: output_of(store, dep),
            })
        })
        .collect()
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
    use crate::model::NodeType;
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

    fn new_node(title: &str, depends_on: Vec<String>) -> NewNode {
        NewNode {
            node_type: NodeType::Task,
            title: title.into(),
            body: "body".into(),
            author: Author::Human,
            depends_on,
            derived_from: vec![],
        }
    }

    fn done(store: &Store, vcs: &dyn Vcs, id: &str) {
        complete(store, vcs, id, &[], &[], None, "done", Author::Machine).unwrap();
    }

    #[test]
    fn add_validates_dependencies_and_starts_open() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();

        assert!(add(&store, &fake, new_node("a", vec!["task-nope".into()])).is_err());

        let id = add(&store, &fake, new_node("a", vec![])).unwrap();
        assert!(store.exists(&id));
        assert_eq!(current_status(&store, &id), Status::Open);
        assert!(staleness(&store, &fake, &id).is_empty(), "no result, nothing to invalidate");
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
        assert_eq!(result.node_version, store.node_version(&id).unwrap());
        assert_eq!(notes, "implemented it");

        // The right paths were captured; add + complete each committed the store.
        assert_eq!(fake.captured.borrow().as_slice(), &[vec!["src/x.rs".to_string()]]);
        assert_eq!(*fake.store_commits.borrow(), 2);
    }

    #[test]
    fn complete_without_outputs_makes_no_commit() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("planning", vec![])).unwrap();

        let commit =
            complete(&store, &fake, &id, &[], &[], None, "made sub-tasks", Author::Machine).unwrap();
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

        edit(&store, &fake, &id, Some("revised".into()), None).unwrap();
        assert_eq!(current_status(&store, &id), Status::Open);
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons.iter().any(|r| r.contains("definition changed since the work")),
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

        edit(&store, &fake, &a, Some("revised".into()), None).unwrap();
        let reasons = staleness(&store, &fake, &b);
        assert!(
            reasons.iter().any(|r| r.contains(&a) && r.contains("definition moved")),
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
        complete(&store, &fake, &a, &["src/a.rs".into()], &[], None, "", Author::Machine).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();
        done(&store, &fake, &b);
        assert!(staleness(&store, &fake, &b).is_empty());

        // A is re-worked and produces a new output commit -> B is stale.
        fake.next_id = "commit-2".into();
        complete(&store, &fake, &a, &["src/a.rs".into()], &[], None, "", Author::Machine).unwrap();
        let reasons = staleness(&store, &fake, &b);
        assert!(
            reasons.iter().any(|r| r.contains(&a) && r.contains("output changed")),
            "{reasons:?}"
        );
    }

    #[test]
    fn context_drift_makes_node_stale() {
        let (t, store) = temp_store();
        let fake = FakeVcs::default();
        let id = add(&store, &fake, new_node("a", vec![])).unwrap();

        std::fs::write(t.0.join("helper.rs"), "v1").unwrap();
        complete(&store, &fake, &id, &[], &["helper.rs".into()], None, "", Author::Machine)
            .unwrap();
        assert!(staleness(&store, &fake, &id).is_empty());

        std::fs::write(t.0.join("helper.rs"), "v2").unwrap();
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons.iter().any(|r| r.contains("context helper.rs") && r.contains("content changed")),
            "{reasons:?}"
        );

        std::fs::remove_file(t.0.join("helper.rs")).unwrap();
        let reasons = staleness(&store, &fake, &id);
        assert!(
            reasons.iter().any(|r| r.contains("context helper.rs") && r.contains("missing")),
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
        complete(&store, &fake, &id, &["src/x.rs".into()], &[], None, "", Author::Machine).unwrap();
        assert!(staleness(&store, &fake, &id).is_empty());

        fake.drift_for.insert("commit-x".into(), "M\tsrc/x.rs".into());
        let reasons = staleness(&store, &fake, &id);
        assert!(reasons.iter().any(|r| r.contains("output changed since")), "{reasons:?}");
    }

    #[test]
    fn blockers_follow_dependency_status() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();
        let a = add(&store, &fake, new_node("a", vec![])).unwrap();
        let b = add(&store, &fake, new_node("b", vec![a.clone()])).unwrap();

        // A not done -> B blocked, not ready.
        let blocked = blockers(&store, &fake, &b);
        assert!(blocked.iter().any(|r| r.contains("not done")), "{blocked:?}");
        assert!(!is_ready(&store, &fake, &b));

        // A done -> B ready.
        done(&store, &fake, &a);
        assert!(blockers(&store, &fake, &b).is_empty());
        assert!(is_ready(&store, &fake, &b));

        // A edited after done -> reopened -> B blocked again.
        edit(&store, &fake, &a, Some("revised".into()), None).unwrap();
        let blocked = blockers(&store, &fake, &b);
        assert!(blocked.iter().any(|r| r.contains("not done")), "{blocked:?}");
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
        complete(&store, &fake, &a, &["src/x.rs".into()], &[], None, "", Author::Machine).unwrap();

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

        assert!(link(&store, &fake, &a, "task-nope", DepKind::DependsOn).is_err());
        link(&store, &fake, &a, &b, DepKind::DependsOn).unwrap();
        assert!(link(&store, &fake, &a, &b, DepKind::DependsOn).is_err());

        let (meta, _) = store.read_node(&a).unwrap();
        assert_eq!(meta.depends_on, vec![b]);
    }

    #[test]
    fn require_clean_rejects_a_dirty_tree() {
        let dirty = FakeVcs {
            dirty: vec!["src/x.rs".into()],
            ..Default::default()
        };
        assert!(require_clean(&dirty).is_err());
        assert!(require_clean(&FakeVcs::default()).is_ok());
    }

    #[test]
    fn require_clean_except_allows_only_declared_outputs() {
        let outputs = vec!["src/out.rs".to_string()];
        let ok = FakeVcs {
            dirty: vec!["src/out.rs".into()],
            ..Default::default()
        };
        assert!(require_clean_except(&ok, &outputs).is_ok());

        let stray = FakeVcs {
            dirty: vec!["src/out.rs".into(), "src/other.rs".into()],
            ..Default::default()
        };
        assert!(require_clean_except(&stray, &outputs).is_err());
    }
}
