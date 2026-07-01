//! Graph operations and derived queries.
//!
//! Every mutating operation ([`add`], [`link`], [`edit`], [`complete`],
//! [`set_status`]) requires a clean working tree and commits its own store change,
//! so each is its own git commit (§2.11). The derived queries ([`staleness`],
//! [`blockers`], [`is_ready`]) recompute from the graph and are never stored.
//!
//! All git interaction goes through `&dyn Vcs`, so the whole module is unit-testable
//! with an in-memory fake — no git binary, repository, or identity required.

use anyhow::{bail, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

use crate::model::{Author, Edge, Meta, NodeType, Pin, Status, StatusEvent};
use crate::store::Store;
use crate::vcs::Vcs;

/// Parameters for creating a node with [`add`].
pub struct NewNode {
    pub node_type: NodeType,
    pub title: String,
    pub body: String,
    pub author: Author,
    /// Logical ids to add `depends_on` edges to.
    pub depends_on: Vec<String>,
    /// Logical ids to add `derived_from` edges to.
    pub derived_from: Vec<String>,
    /// Paths to declare as pinned inputs.
    pub inputs: Vec<String>,
}

/// Create a new node. Returns `(logical_id, version hash)`.
pub fn add(store: &Store, vcs: &dyn Vcs, new: NewNode) -> Result<(String, String)> {
    require_clean(vcs)?;
    let logical_id = format!("{}-{}", new.node_type.prefix(), Ulid::new());

    let mut edges = Vec::new();
    for dep in &new.depends_on {
        edges.push(make_edge(store, dep, "depends_on")?);
    }
    for src in &new.derived_from {
        edges.push(make_edge(store, src, "derived_from")?);
    }
    let inputs = pin_files(vcs, &new.inputs)?;

    let meta = Meta {
        schema: 1,
        logical_id: logical_id.clone(),
        node_type: new.node_type,
        title: new.title,
        author: new.author,
        parent: None,
        output_commit: None,
        edges,
        inputs,
        context: Vec::new(),
    };
    let hash = store.put_object(&meta, &new.body)?;
    store.set_ref(&logical_id, &hash)?;
    store.append_status(
        &logical_id,
        &StatusEvent {
            at: now_millis(),
            status: Status::Open,
            author: new.author,
            version: hash.clone(),
        },
    )?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: add {logical_id}"))?;
    Ok((logical_id, hash))
}

/// Add a typed edge from `from` to `to` (a new version of `from`). Returns the new hash.
pub fn link(
    store: &Store,
    vcs: &dyn Vcs,
    from: &str,
    to: &str,
    rel: &str,
    author: Author,
) -> Result<String> {
    require_clean(vcs)?;
    let current = store.get_ref(from)?;
    let (mut meta, body) = store.get_object(&current)?;
    let pin = store
        .get_ref(to)
        .with_context(|| format!("cannot link to unknown node `{to}`"))?;
    meta.edges.push(Edge {
        to: to.to_string(),
        rel: rel.to_string(),
        pin,
    });
    meta.parent = Some(current);
    meta.author = author;
    let hash = store.put_object(&meta, &body)?;
    store.set_ref(from, &hash)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: link {from} -> {to}"))?;
    Ok(hash)
}

/// Edit a node's title and/or body, producing a new version. `body = None` keeps the
/// current body. Returns the new hash.
pub fn edit(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    title: Option<String>,
    body: Option<String>,
    author: Author,
) -> Result<String> {
    require_clean(vcs)?;
    let current = store.get_ref(id)?;
    let (mut meta, old_body) = store.get_object(&current)?;
    if let Some(t) = title {
        meta.title = t;
    }
    let new_body = body.unwrap_or(old_body);
    meta.parent = Some(current);
    meta.author = author;
    let hash = store.put_object(&meta, &new_body)?;
    store.set_ref(id, &hash)?;
    vcs.commit_store(&store.store_name(), &format!("llaundry: edit {id}"))?;
    Ok(hash)
}

/// Complete a node: capture the produced files via `vcs`, record the resulting
/// output id on a new immutable version, mark it `done`, and persist the store
/// change. Returns `(new version hash, output id)`.
pub fn complete(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    outputs: &[String],
    context: &[String],
    message: Option<String>,
    author: Author,
) -> Result<(String, String)> {
    // The only uncommitted changes allowed are the outputs we are about to commit.
    require_clean_except(vcs, outputs)?;

    let current = store.get_ref(id)?;
    let (mut meta, body) = store.get_object(&current)?;

    // Pin the context actually used (before committing outputs).
    let context = pin_files(vcs, context)?;
    let message = message.unwrap_or_else(|| format!("{}: {}", meta.node_type.as_str(), meta.title));
    let commit = vcs.capture(outputs, &message)?;

    meta.output_commit = Some(commit.clone());
    meta.context = context;
    meta.parent = Some(current);
    meta.author = author;
    let hash = store.put_object(&meta, &body)?;
    store.set_ref(id, &hash)?;
    store.append_status(
        id,
        &StatusEvent {
            at: now_millis(),
            status: Status::Done,
            author,
            version: hash.clone(),
        },
    )?;

    vcs.commit_store(&store.store_name(), &format!("llaundry: complete {id}"))?;
    Ok((hash, commit))
}

/// Append a status event and commit the store change.
pub fn set_status(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    status: Status,
    author: Author,
) -> Result<()> {
    require_clean(vcs)?;
    let version = store.get_ref(id)?;
    store.append_status(
        id,
        &StatusEvent {
            at: now_millis(),
            status,
            author,
            version,
        },
    )?;
    vcs.commit_store(
        &store.store_name(),
        &format!("llaundry: status {id} {}", status.as_str()),
    )?;
    Ok(())
}

/// Collect explicit reasons a node version is stale, if any.
///
/// Independent sources of staleness:
///   * an edge whose target node has moved past the pinned version (or vanished),
///   * a declared input or recorded context file whose content has drifted,
///   * an output that has changed since the node's output capture, and
///   * a `done` status set on a version the node has since moved past.
pub fn staleness(store: &Store, vcs: &dyn Vcs, meta: &Meta) -> Vec<String> {
    let mut reasons = Vec::new();

    for e in &meta.edges {
        match store.get_ref(&e.to) {
            Ok(current) if current != e.pin => reasons.push(format!(
                "{} -> {}: target moved (pinned {}, now {})",
                e.rel,
                e.to,
                short(&e.pin),
                short(&current)
            )),
            Ok(_) => {}
            Err(_) => reasons.push(format!("{} -> {}: target missing", e.rel, e.to)),
        }
    }

    pin_drift(vcs, "input", &meta.inputs, &mut reasons);
    pin_drift(vcs, "context", &meta.context, &mut reasons);

    if let Some(commit) = &meta.output_commit {
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

    // A `done` status certifies the specific version it was set on. If the node has
    // since been edited (a new version), the completion no longer covers the current
    // content, so it is stale. (`open`/`in_progress` don't certify content, so they
    // are not version-sensitive.)
    if let (Ok(current), Ok(log)) = (
        store.get_ref(&meta.logical_id),
        store.status_log(&meta.logical_id),
    ) {
        if let Some(last) = log.events.last() {
            if last.status == Status::Done && last.version != current {
                reasons.push(format!(
                    "done on an older version (completed {}, now {})",
                    short(&last.version),
                    short(&current)
                ));
            }
        }
    }

    reasons
}

/// Reasons a node's `depends_on` dependencies are unsatisfied — empty means ready.
///
/// "Blocked" is computed here, not stored: a dependency is satisfied only if its
/// target is `done` and not itself stale. Because it is derived from the graph, it
/// can never drift out of sync the way a manual `blocked` flag would.
pub fn blockers(store: &Store, vcs: &dyn Vcs, meta: &Meta) -> Vec<String> {
    let mut out = Vec::new();
    for e in &meta.edges {
        if e.rel != "depends_on" {
            continue;
        }
        let hash = match store.get_ref(&e.to) {
            Ok(h) => h,
            Err(_) => {
                out.push(format!("{}: missing", e.to));
                continue;
            }
        };
        match current_status(store, &e.to) {
            Some(Status::Done) => match store.get_object(&hash) {
                Ok((target, _)) if !staleness(store, vcs, &target).is_empty() => {
                    out.push(format!("{}: stale", e.to));
                }
                Ok(_) => {}
                Err(_) => out.push(format!("{}: unreadable", e.to)),
            },
            other => out.push(format!(
                "{}: not done ({})",
                e.to,
                other.map_or("no status", |s| s.as_str())
            )),
        }
    }
    out
}

/// Whether a node is ready to be worked: not already done, and no unsatisfied
/// dependencies.
pub fn is_ready(store: &Store, vcs: &dyn Vcs, meta: &Meta) -> bool {
    current_status(store, &meta.logical_id) != Some(Status::Done)
        && blockers(store, vcs, meta).is_empty()
}

/// The current (latest) status of a node, or `None` if it has no status events.
pub fn current_status(store: &Store, id: &str) -> Option<Status> {
    store
        .status_log(id)
        .ok()
        .and_then(|log| log.events.last().map(|e| e.status))
}

/// Enforce a clean working tree before a node operation, so the commit recorded for
/// the resulting state change fully represents the repository.
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

/// Resolve a target's current version hash and build an edge pinned to it.
fn make_edge(store: &Store, to: &str, rel: &str) -> Result<Edge> {
    let pin = store
        .get_ref(to)
        .with_context(|| format!("cannot link to unknown node `{to}`"))?;
    Ok(Edge {
        to: to.to_string(),
        rel: rel.to_string(),
        pin,
    })
}

/// Pin each path by its current content; errors if a file is missing.
fn pin_files(vcs: &dyn Vcs, paths: &[String]) -> Result<Vec<Pin>> {
    paths
        .iter()
        .map(|path| {
            let content = vcs
                .content_id(path)?
                .with_context(|| format!("cannot pin `{path}`: file not found"))?;
            Ok(Pin {
                path: path.clone(),
                content,
            })
        })
        .collect()
}

/// Append a reason for every pinned file whose content has drifted or vanished.
fn pin_drift(vcs: &dyn Vcs, kind: &str, pins: &[Pin], reasons: &mut Vec<String>) {
    for pin in pins {
        match vcs.content_id(&pin.path) {
            Ok(Some(now)) if now != pin.content => reasons.push(format!(
                "{kind} {}: content changed (pinned {}, now {})",
                pin.path,
                short(&pin.content),
                short(&now)
            )),
            Ok(Some(_)) => {}
            Ok(None) => reasons.push(format!(
                "{kind} {}: missing (pinned {})",
                pin.path,
                short(&pin.content)
            )),
            Err(e) => reasons.push(format!("{kind} {} check failed: {e}", pin.path)),
        }
    }
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

    /// Write a node version with a given logical id; returns its hash.
    fn put_node(store: &Store, id: &str, edges: Vec<Edge>, output_commit: Option<String>) -> String {
        put_meta(
            store,
            Meta {
                schema: 1,
                logical_id: id.to_string(),
                node_type: NodeType::Task,
                title: "title".into(),
                author: Author::Human,
                parent: None,
                output_commit,
                edges,
                inputs: vec![],
                context: vec![],
            },
        )
    }

    fn put_meta(store: &Store, meta: Meta) -> String {
        let id = meta.logical_id.clone();
        let hash = store.put_object(&meta, "body").unwrap();
        store.set_ref(&id, &hash).unwrap();
        hash
    }

    /// A `done` event asserted against version `v`.
    fn done_event(v: &str) -> StatusEvent {
        StatusEvent {
            at: 0,
            status: Status::Done,
            author: Author::Human,
            version: v.to_string(),
        }
    }

    #[test]
    fn objects_roundtrip_and_dedup() {
        let (_t, store) = temp_store();
        let meta = Meta {
            schema: 1,
            logical_id: "task-1".into(),
            node_type: NodeType::Task,
            title: "hello".into(),
            author: Author::Human,
            parent: None,
            output_commit: None,
            edges: vec![],
            inputs: vec![],
            context: vec![],
        };
        let h1 = store.put_object(&meta, "body").unwrap();
        let h2 = store.put_object(&meta, "body").unwrap();
        assert_eq!(h1, h2, "identical content hashes to the same object");
        assert_ne!(h1, store.put_object(&meta, "other body").unwrap());

        let (got, body) = store.get_object(&h1).unwrap();
        assert_eq!(got.title, "hello");
        assert_eq!(body, "body");
    }

    #[test]
    fn refs_and_status_log() {
        let (_t, store) = temp_store();
        store.set_ref("task-1", "abc").unwrap();
        assert_eq!(store.get_ref("task-1").unwrap(), "abc");
        assert!(store.list_refs().unwrap().contains(&"task-1".to_string()));

        let ev = |s: Status| StatusEvent {
            at: 0,
            status: s,
            author: Author::Human,
            version: "abc".into(),
        };
        store.append_status("task-1", &ev(Status::Open)).unwrap();
        store.append_status("task-1", &ev(Status::Done)).unwrap();
        let log = store.status_log("task-1").unwrap();
        assert_eq!(log.events.len(), 2);
        assert_eq!(log.events.last().unwrap().status, Status::Done);
    }

    #[test]
    fn edge_staleness_detects_moved_and_missing_targets() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();

        let target = put_node(&store, "task-target", vec![], None);
        let edge = Edge {
            to: "task-target".into(),
            rel: "depends_on".into(),
            pin: target,
        };
        put_node(&store, "task-dep", vec![edge], None);
        let (dep, _) = store.get_object(&store.get_ref("task-dep").unwrap()).unwrap();

        // Fresh: pin matches the target's current ref.
        assert!(staleness(&store, &fake, &dep).is_empty());

        // Move the target on: the dependent goes stale.
        store.set_ref("task-target", "newhash").unwrap();
        let reasons = staleness(&store, &fake, &dep);
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("target moved"), "{reasons:?}");

        // A vanished target is reported too.
        let edge = Edge {
            to: "task-gone".into(),
            rel: "depends_on".into(),
            pin: "x".into(),
        };
        put_node(&store, "task-orphan", vec![edge], None);
        let (orphan, _) = store.get_object(&store.get_ref("task-orphan").unwrap()).unwrap();
        assert!(staleness(&store, &fake, &orphan)[0].contains("target missing"));
    }

    #[test]
    fn output_staleness_uses_the_vcs() {
        let (_t, store) = temp_store();
        let hash = put_node(&store, "impl-1", vec![], Some("commitX".into()));
        let (meta, _) = store.get_object(&hash).unwrap();

        // No drift recorded -> not stale.
        let mut fake = FakeVcs::default();
        assert!(staleness(&store, &fake, &meta).is_empty());

        // Drift recorded for that output id -> stale, with the reason surfaced.
        fake.drift_for.insert("commitX".into(), "M\tsrc/x.rs".into());
        let reasons = staleness(&store, &fake, &meta);
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("output changed since"));
        assert!(reasons[0].contains("src/x.rs"));
    }

    #[test]
    fn complete_captures_output_and_marks_done() {
        let (_t, store) = temp_store();
        put_node(&store, "impl-1", vec![], None);
        let mut fake = FakeVcs {
            next_id: "commit-abc".into(),
            ..Default::default()
        };

        let (hash, commit) = complete(
            &store,
            &fake,
            "impl-1",
            &["src/x.rs".into()],
            &[],
            None,
            Author::Machine,
        )
        .unwrap();

        assert_eq!(commit, "commit-abc");
        assert_eq!(store.get_ref("impl-1").unwrap(), hash, "ref advanced to new version");

        let (meta, _) = store.get_object(&hash).unwrap();
        assert_eq!(meta.output_commit.as_deref(), Some("commit-abc"));
        assert_eq!(meta.author, Author::Machine);
        assert_eq!(current_status(&store, "impl-1"), Some(Status::Done));

        // The right paths were captured, and the store change was committed once.
        assert_eq!(fake.captured.borrow().as_slice(), &[vec!["src/x.rs".to_string()]]);
        assert_eq!(*fake.store_commits.borrow(), 1);

        // And the completed node now reports as stale once its output drifts.
        fake.drift_for.insert("commit-abc".into(), "M\tsrc/x.rs".into());
        assert!(!staleness(&store, &fake, &meta).is_empty());
    }

    #[test]
    fn input_and_context_staleness() {
        let (_t, store) = temp_store();
        let meta = Meta {
            schema: 1,
            logical_id: "task-1".into(),
            node_type: NodeType::Task,
            title: "t".into(),
            author: Author::Human,
            parent: None,
            output_commit: None,
            edges: vec![],
            inputs: vec![Pin {
                path: "src/a.rs".into(),
                content: "h1".into(),
            }],
            context: vec![Pin {
                path: "src/b.rs".into(),
                content: "h2".into(),
            }],
        };
        put_meta(&store, meta.clone());

        // Both pins match current content -> clean.
        let mut fake = FakeVcs::default();
        fake.content.insert("src/a.rs".into(), "h1".into());
        fake.content.insert("src/b.rs".into(), "h2".into());
        assert!(staleness(&store, &fake, &meta).is_empty());

        // A declared input changes -> stale, labelled "input".
        fake.content.insert("src/a.rs".into(), "h1-new".into());
        let r = staleness(&store, &fake, &meta);
        assert!(
            r.iter()
                .any(|s| s.contains("input src/a.rs") && s.contains("content changed")),
            "{r:?}"
        );

        // Recorded context goes missing -> stale, labelled "context".
        fake.content.remove("src/b.rs");
        let r = staleness(&store, &fake, &meta);
        assert!(
            r.iter()
                .any(|s| s.contains("context src/b.rs") && s.contains("missing")),
            "{r:?}"
        );
    }

    #[test]
    fn complete_pins_recorded_context() {
        let (_t, store) = temp_store();
        put_node(&store, "impl-1", vec![], None);
        let mut fake = FakeVcs {
            next_id: "commit-1".into(),
            ..Default::default()
        };
        fake.content.insert("src/read.rs".into(), "rh".into());

        let (hash, _) = complete(
            &store,
            &fake,
            "impl-1",
            &["src/out.rs".into()],
            &["src/read.rs".into()],
            None,
            Author::Machine,
        )
        .unwrap();

        let (meta, _) = store.get_object(&hash).unwrap();
        assert_eq!(meta.context.len(), 1);
        assert_eq!(meta.context[0].path, "src/read.rs");
        assert_eq!(meta.context[0].content, "rh");
    }

    #[test]
    fn blockers_follow_dependency_status() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();

        let target = put_node(&store, "task-target", vec![], None);
        let edge = Edge {
            to: "task-target".into(),
            rel: "depends_on".into(),
            pin: target.clone(),
        };
        put_node(&store, "task-dep", vec![edge], None);
        let (dep, _) = store.get_object(&store.get_ref("task-dep").unwrap()).unwrap();

        // Target has no status yet -> the dependent is blocked.
        let b = blockers(&store, &fake, &dep);
        assert_eq!(b.len(), 1);
        assert!(b[0].contains("not done"), "{b:?}");

        // Once the target is done on its current version, the dependent is ready.
        store
            .append_status("task-target", &done_event(&target))
            .unwrap();
        assert!(blockers(&store, &fake, &dep).is_empty());
    }

    #[test]
    fn done_status_only_covers_the_version_it_was_set_on() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();

        let v1 = put_node(&store, "task-1", vec![], None);
        store.append_status("task-1", &done_event(&v1)).unwrap();

        // Completed on the current version -> not stale.
        let (m1, _) = store.get_object(&v1).unwrap();
        assert!(staleness(&store, &fake, &m1).is_empty());

        // Edit the node: new version, ref moves; the done event still points at v1.
        let mut m2 = m1.clone();
        m2.title = "revised".into();
        m2.parent = Some(v1.clone());
        let v2 = store.put_object(&m2, "body").unwrap();
        store.set_ref("task-1", &v2).unwrap();
        let (m2, _) = store.get_object(&v2).unwrap();

        // The completion no longer covers the current version -> stale.
        let reasons = staleness(&store, &fake, &m2);
        assert!(
            reasons.iter().any(|r| r.contains("older version")),
            "{reasons:?}"
        );
    }

    #[test]
    fn done_on_an_older_version_blocks_dependents() {
        let (_t, store) = temp_store();
        let fake = FakeVcs::default();

        let tv1 = put_node(&store, "task-target", vec![], None);
        store.append_status("task-target", &done_event(&tv1)).unwrap();
        let edge = Edge {
            to: "task-target".into(),
            rel: "depends_on".into(),
            pin: tv1.clone(),
        };
        put_node(&store, "task-dep", vec![edge], None);
        let (dep, _) = store.get_object(&store.get_ref("task-dep").unwrap()).unwrap();

        // Target done on its current version -> dependent ready.
        assert!(blockers(&store, &fake, &dep).is_empty());

        // Edit the target: its `done` no longer applies -> dependent blocked again.
        let (mut tmeta, _) = store.get_object(&tv1).unwrap();
        tmeta.title = "revised".into();
        tmeta.parent = Some(tv1.clone());
        let tv2 = store.put_object(&tmeta, "body").unwrap();
        store.set_ref("task-target", &tv2).unwrap();
        assert!(!blockers(&store, &fake, &dep).is_empty());
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
