//! `llaundry` — a tiny CLI over a content-addressed, immutable node graph.
//!
//! See DESIGN.md for the model and the reasoning behind it.

mod git;
mod model;
mod store;
mod vcs;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

use model::{Author, Edge, Meta, NodeType, StatusEvent};
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
        /// Message for the output commit (defaults to the node's type and title).
        #[arg(long, short = 'm')]
        message: Option<String>,
        #[arg(long, value_enum, default_value = "machine")]
        author: Author,
    },

    /// Append a status event (open, in_progress, done, failed, blocked, ...).
    #[command(alias = "status")]
    SetStatus {
        id: String,
        status: String,
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
        } => {
            let store = Store::open(store)?;
            let body = read_body(body, file)?;
            let logical_id = format!("{}-{}", node_type.prefix(), Ulid::new());

            let mut edges = Vec::new();
            for dep in &depends_on {
                edges.push(make_edge(&store, dep, "depends_on")?);
            }
            for src in &derived_from {
                edges.push(make_edge(&store, src, "derived_from")?);
            }

            let meta = Meta {
                schema: 1,
                logical_id: logical_id.clone(),
                node_type,
                title,
                author,
                parent: None,
                output_commit: None,
                edges,
            };
            let hash = store.put_object(&meta, &body)?;
            store.set_ref(&logical_id, &hash)?;
            store.append_status(
                &logical_id,
                &StatusEvent {
                    at: now_millis(),
                    status: "open".into(),
                    author,
                    version: hash.clone(),
                },
            )?;
            println!("{logical_id}  {}", short(&hash));
        }

        Cmd::Link {
            from,
            to,
            rel,
            author,
        } => {
            let store = Store::open(store)?;
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
            println!("{id}  {}", short(&hash));
        }

        Cmd::Complete {
            id,
            outputs,
            message,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = git::GitVcs::new(store.project_root());
            let paths: Vec<String> = outputs
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let (hash, commit) = complete(&store, &vcs, &id, &paths, message, author)?;
            println!("{id}  {}  (output {})", short(&hash), short(&commit));
        }

        Cmd::SetStatus {
            id,
            status,
            author,
        } => {
            let store = Store::open(store)?;
            let version = store.get_ref(&id)?;
            store.append_status(
                &id,
                &StatusEvent {
                    at: now_millis(),
                    status: status.clone(),
                    author,
                    version,
                },
            )?;
            println!("{id}  -> {status}");
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
    paths: &[String],
    message: Option<String>,
    author: Author,
) -> Result<(String, String)> {
    let current = store.get_ref(id)?;
    let (mut meta, body) = store.get_object(&current)?;

    let message =
        message.unwrap_or_else(|| format!("{}: {}", meta.node_type.as_str(), meta.title));
    let commit = vcs.capture(paths, &message)?;

    meta.output_commit = Some(commit.clone());
    meta.parent = Some(current);
    meta.author = author;
    let hash = store.put_object(&meta, &body)?;
    store.set_ref(id, &hash)?;
    store.append_status(
        id,
        &StatusEvent {
            at: now_millis(),
            status: "done".into(),
            author,
            version: hash.clone(),
        },
    )?;

    vcs.commit_store(&store.store_name(), &format!("llaundry: complete {id}"))?;
    Ok((hash, commit))
}

/// Collect explicit reasons a node version is stale, if any.
///
/// Two independent sources of staleness:
///   * an edge whose target has moved past the pinned version (or vanished), and
///   * an output that has changed since the node's output capture (via `vcs`).
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
        let meta = Meta {
            schema: 1,
            logical_id: id.to_string(),
            node_type: NodeType::Task,
            title: "title".into(),
            author: Author::Human,
            parent: None,
            output_commit,
            edges,
        };
        let hash = store.put_object(&meta, "body").unwrap();
        store.set_ref(id, &hash).unwrap();
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

        let ev = |s: &str| StatusEvent {
            at: 0,
            status: s.into(),
            author: Author::Human,
            version: "abc".into(),
        };
        store.append_status("task-1", &ev("open")).unwrap();
        store.append_status("task-1", &ev("done")).unwrap();
        let log = store.status_log("task-1").unwrap();
        assert_eq!(log.events.len(), 2);
        assert_eq!(log.events.last().unwrap().status, "done");
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

        let (hash, commit) =
            complete(&store, &fake, "impl-1", &["src/x.rs".into()], None, Author::Machine).unwrap();

        assert_eq!(commit, "commit-abc");
        assert_eq!(store.get_ref("impl-1").unwrap(), hash, "ref advanced to new version");

        let (meta, _) = store.get_object(&hash).unwrap();
        assert_eq!(meta.output_commit.as_deref(), Some("commit-abc"));
        assert_eq!(meta.author, Author::Machine);
        assert_eq!(store.status_log("impl-1").unwrap().events.last().unwrap().status, "done");

        // The right paths were captured, and the store change was committed once.
        assert_eq!(fake.captured.borrow().as_slice(), &[vec!["src/x.rs".to_string()]]);
        assert_eq!(*fake.store_commits.borrow(), 1);

        // And the completed node now reports as stale once its output drifts.
        fake.drift_for.insert("commit-abc".into(), "M\tsrc/x.rs".into());
        assert!(!staleness(&store, &fake, &meta).is_empty());
    }
}
