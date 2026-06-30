//! `llaundry` — a tiny CLI over a content-addressed, immutable node graph.
//!
//! See DESIGN.md for the model and the reasoning behind it.

mod git;
mod model;
mod store;
mod vcs;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

use model::{Author, Edge, Meta, NodeType, Pin, Status, StatusEvent};
use store::Store;
use vcs::Vcs;

#[derive(Parser)]
#[command(
    name = "llaundry",
    version,
    about = "A content-addressed, immutable graph of LLM-development nodes"
)]
struct Cli {
    /// Path to the store directory.
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry", global = true)]
    store: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new, empty store.
    Init,

    /// Add a new node. Prints its logical id.
    Add {
        #[arg(long = "type", value_enum, default_value = "task")]
        node_type: NodeType,
        #[arg(long)]
        title: String,
        /// Body text inline.
        #[arg(long)]
        body: Option<String>,
        /// Body text read from a file (mutually exclusive with --body).
        #[arg(long, conflicts_with = "body")]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
        /// Add a `depends_on` edge to another node (repeatable), by logical id.
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        /// Add a `derived_from` edge to another node (repeatable), by logical id.
        #[arg(long = "derived-from")]
        derived_from: Vec<String>,
        /// Declare an input file this node is allowed to use, pinned by its current
        /// content (repeatable). Changing it later invalidates the node.
        #[arg(long = "input", short = 'i')]
        input: Vec<PathBuf>,
    },

    /// Add a typed edge from one node to another.
    /// This is an edit, so it produces a new version of <from>.
    Link {
        /// Source node (the one that gains the edge).
        from: String,
        /// Target node.
        to: String,
        #[arg(long, default_value = "depends_on")]
        rel: String,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Edit a node, producing a new immutable version.
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long, conflicts_with = "body")]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Complete a node by committing the output files it produced.
    /// Commits the named files with git, stores that commit hash on a new version,
    /// commits the store change, and marks the node `done`.
    Complete {
        id: String,
        /// A produced file, relative to the project root (repeatable).
        #[arg(long = "output", short = 'o', required = true)]
        outputs: Vec<PathBuf>,
        /// A file that was actually used while working this node but was not a
        /// declared input — e.g. read by an agent's tool call (repeatable). Pinned
        /// by content, so a later change to it also invalidates the node.
        #[arg(long = "context", short = 'c')]
        context: Vec<PathBuf>,
        /// Message for the output commit (defaults to the node's type and title).
        #[arg(long, short = 'm')]
        message: Option<String>,
        #[arg(long, value_enum, default_value = "machine")]
        author: Author,
    },

    /// Append a status event.
    #[command(alias = "status")]
    SetStatus {
        id: String,
        #[arg(value_enum)]
        status: Status,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Show a node: its current version, edges, and status.
    Show { id: String },

    /// List every node with its current status.
    List,

    /// Show the version history of a node (newest first).
    Log { id: String },

    /// Report nodes whose edges point at outdated versions of their targets.
    Stale,

    /// List unfinished nodes whose dependencies are all satisfied (done, not stale).
    Ready,

    /// List nodes blocked by an unsatisfied dependency, with reasons.
    Blocked,
}

fn main() -> Result<()> {
    let Cli { store, cmd } = Cli::parse();
    match cmd {
        Cmd::Init => {
            Store::init(store.clone())?;
            println!("initialised llaundry store at {}", store.display());
        }

        Cmd::Add {
            node_type,
            title,
            body,
            file,
            author,
            depends_on,
            derived_from,
            input,
        } => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            require_clean(&vcs)?;
            let body = read_body(body, file)?;
            let logical_id = format!("{}-{}", node_type.prefix(), Ulid::new());

            let mut edges = Vec::new();
            for dep in &depends_on {
                edges.push(make_edge(&store, dep, "depends_on")?);
            }
            for src in &derived_from {
                edges.push(make_edge(&store, src, "derived_from")?);
            }
            let inputs = pin_files(&vcs, &to_strings(&input))?;

            let meta = Meta {
                schema: 1,
                logical_id: logical_id.clone(),
                node_type,
                title,
                author,
                parent: None,
                output_commit: None,
                edges,
                inputs,
                context: Vec::new(),
            };
            let hash = store.put_object(&meta, &body)?;
            store.set_ref(&logical_id, &hash)?;
            store.append_status(
                &logical_id,
                &StatusEvent {
                    at: now_millis(),
                    status: Status::Open,
                    author,
                    version: hash.clone(),
                },
            )?;
            vcs.commit_store(&store.store_name(), &format!("llaundry: add {logical_id}"))?;
            println!("{logical_id}  {}", short(&hash));
        }

        Cmd::Link {
            from,
            to,
            rel,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            require_clean(&vcs)?;
            let current = store.get_ref(&from)?;
            let (mut meta, body) = store.get_object(&current)?;
            let pin = store
                .get_ref(&to)
                .with_context(|| format!("cannot link to unknown node `{to}`"))?;
            meta.edges.push(Edge {
                to: to.clone(),
                rel: rel.clone(),
                pin,
            });
            meta.parent = Some(current);
            meta.author = author;
            let hash = store.put_object(&meta, &body)?;
            store.set_ref(&from, &hash)?;
            vcs.commit_store(&store.store_name(), &format!("llaundry: link {from} -> {to}"))?;
            println!("{from}  {}  (+{rel} -> {to})", short(&hash));
        }

        Cmd::Edit {
            id,
            title,
            body,
            file,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            require_clean(&vcs)?;
            let current = store.get_ref(&id)?;
            let (mut meta, old_body) = store.get_object(&current)?;
            if let Some(t) = title {
                meta.title = t;
            }
            let new_body = if body.is_some() || file.is_some() {
                read_body(body, file)?
            } else {
                old_body
            };
            meta.parent = Some(current);
            meta.author = author;
            let hash = store.put_object(&meta, &new_body)?;
            store.set_ref(&id, &hash)?;
            vcs.commit_store(&store.store_name(), &format!("llaundry: edit {id}"))?;
            println!("{id}  {}", short(&hash));
        }

        Cmd::Complete {
            id,
            outputs,
            context,
            message,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            let (hash, commit) = complete(
                &store,
                &vcs,
                &id,
                &to_strings(&outputs),
                &to_strings(&context),
                message,
                author,
            )?;
            println!("{id}  {}  (output {})", short(&hash), short(&commit));
        }

        Cmd::SetStatus {
            id,
            status,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            require_clean(&vcs)?;
            let version = store.get_ref(&id)?;
            store.append_status(
                &id,
                &StatusEvent {
                    at: now_millis(),
                    status,
                    author,
                    version,
                },
            )?;
            vcs.commit_store(&store.store_name(), &format!("llaundry: status {id} {}", status.as_str()))?;
            println!("{id}  -> {}", status.as_str());
        }

        Cmd::Show { id } => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            let hash = store.get_ref(&id)?;
            let (meta, body) = store.get_object(&hash)?;
            let log = store.status_log(&id)?;
            let status = log.events.last().map_or("(none)", |e| e.status.as_str());

            println!("id:      {}", meta.logical_id);
            println!("type:    {}", meta.node_type.as_str());
            println!("title:   {}", meta.title);
            println!("status:  {status}");
            println!("author:  {}", meta.author.as_str());
            println!("version: {hash}");
            if let Some(parent) = &meta.parent {
                println!("parent:  {}", short(parent));
            }
            if !meta.edges.is_empty() {
                println!("edges:");
                for e in &meta.edges {
                    println!("  {:<12} -> {} @ {}", e.rel, e.to, short(&e.pin));
                }
            }
            if !meta.inputs.is_empty() {
                println!("inputs:");
                for p in &meta.inputs {
                    println!("  {} @ {}", p.path, short(&p.content));
                }
            }
            if !meta.context.is_empty() {
                println!("context:");
                for p in &meta.context {
                    println!("  {} @ {}", p.path, short(&p.content));
                }
            }
            if let Some(commit) = &meta.output_commit {
                println!("output:  commit {}", short(commit));
            }
            let reasons = staleness(&store, &vcs, &meta);
            if !reasons.is_empty() {
                println!("stale:");
                for r in &reasons {
                    println!("  {r}");
                }
            }
            let body = body.trim_end();
            if !body.is_empty() {
                println!("\n{body}");
            }
        }

        Cmd::List => {
            let store = Store::open(store)?;
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                let log = store.status_log(&id)?;
                let status = log.events.last().map_or("-", |e| e.status.as_str());
                println!(
                    "{:<30} {:<12} {:<14} {}",
                    id,
                    status,
                    meta.node_type.as_str(),
                    meta.title
                );
            }
        }

        Cmd::Log { id } => {
            let store = Store::open(store)?;
            let mut hash = store.get_ref(&id)?;
            loop {
                let (meta, _) = store.get_object(&hash)?;
                println!(
                    "{}  {} {}",
                    short(&hash),
                    meta.author.as_str(),
                    meta.title
                );
                match meta.parent {
                    Some(parent) => hash = parent,
                    None => break,
                }
            }
        }

        Cmd::Stale => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            let mut found = false;
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                let reasons = staleness(&store, &vcs, &meta);
                if !reasons.is_empty() {
                    found = true;
                    println!("{id}:");
                    for r in &reasons {
                        println!("  {r}");
                    }
                }
            }
            if !found {
                println!("all nodes up to date");
            }
        }

        Cmd::Ready => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                if current_status(&store, &id) == Some(Status::Done) {
                    continue;
                }
                if blockers(&store, &vcs, &meta).is_empty() {
                    println!("{:<30} {}", id, meta.title);
                }
            }
        }

        Cmd::Blocked => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            let mut any = false;
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                let blockers = blockers(&store, &vcs, &meta);
                if !blockers.is_empty() {
                    any = true;
                    println!("{id}:");
                    for b in &blockers {
                        println!("  blocked by {b}");
                    }
                }
            }
            if !any {
                println!("nothing blocked");
            }
        }
    }
    Ok(())
}

/// Complete a node: capture the produced files via `vcs`, record the resulting
/// output id on a new immutable version, mark it `done`, and persist the store
/// change. Returns `(new version hash, output id)`.
///
/// Takes `&dyn Vcs` rather than touching git directly, so it is unit-testable with
/// an in-memory fake.
fn complete(
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
    let message =
        message.unwrap_or_else(|| format!("{}: {}", meta.node_type.as_str(), meta.title));
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

/// Collect explicit reasons a node version is stale, if any.
///
/// Independent sources of staleness:
///   * an edge whose target node has moved past the pinned version (or vanished),
///   * a declared input or recorded context file whose content has drifted, and
///   * an output that has changed since the node's output capture.
fn staleness(store: &Store, vcs: &dyn Vcs, meta: &Meta) -> Vec<String> {
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

    reasons
}

/// Enforce a clean working tree before a node operation, so the commit recorded
/// for the resulting state change fully represents the repository.
fn require_clean(vcs: &dyn Vcs) -> Result<()> {
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
/// used by `complete`, whose job is to commit exactly the produced outputs.
fn require_clean_except(vcs: &dyn Vcs, allowed: &[String]) -> Result<()> {
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

/// The current (latest) status of a node, or `None` if it has no status events.
fn current_status(store: &Store, id: &str) -> Option<Status> {
    store
        .status_log(id)
        .ok()
        .and_then(|log| log.events.last().map(|e| e.status))
}

/// Reasons a node's `depends_on` dependencies are unsatisfied — empty means ready.
///
/// "Blocked" is computed here, not stored: a dependency is satisfied only if its
/// target is `done` and not itself stale. Because it is derived from the graph, it
/// can never drift out of sync the way a manual `blocked` flag would.
fn blockers(store: &Store, vcs: &dyn Vcs, meta: &Meta) -> Vec<String> {
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

/// Convert CLI path arguments to project-root-relative strings.
fn to_strings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
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

fn read_body(body: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match (body, file) {
        (Some(b), _) => Ok(b),
        (None, Some(f)) => {
            std::fs::read_to_string(&f).with_context(|| format!("reading body from {}", f.display()))
        }
        (None, None) => Ok(String::new()),
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// First 12 characters of a hash, for compact display.
fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
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
        fake.drift_for
            .insert("commitX".into(), "M\tsrc/x.rs".into());
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
            pin: target,
        };
        put_node(&store, "task-dep", vec![edge], None);
        let (dep, _) = store.get_object(&store.get_ref("task-dep").unwrap()).unwrap();

        // Target has no status yet -> the dependent is blocked.
        let b = blockers(&store, &fake, &dep);
        assert_eq!(b.len(), 1);
        assert!(b[0].contains("not done"), "{b:?}");

        // Once the target is done (and not stale), the dependent is ready.
        let ev = StatusEvent {
            at: 0,
            status: Status::Done,
            author: Author::Human,
            version: "h".into(),
        };
        store.append_status("task-target", &ev).unwrap();
        assert!(blockers(&store, &fake, &dep).is_empty());
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
