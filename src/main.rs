//! `llaundry` — a tiny CLI over a content-addressed, immutable node graph.
//!
//! See DESIGN.md for the model and the reasoning behind it.

mod git;
mod model;
mod store;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

use model::{Author, Edge, Meta, NodeType, StatusEvent};
use store::Store;

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
            let base = store.project_root();
            let current = store.get_ref(&id)?;
            let (mut meta, body) = store.get_object(&current)?;

            // 1. Commit the produced files; the commit hash is the output hash.
            let paths: Vec<String> = outputs
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let message =
                message.unwrap_or_else(|| format!("{}: {}", meta.node_type.as_str(), meta.title));
            let commit = git::commit_paths(&base, &paths, &message)?;

            // 2. Record the commit on a new node version and mark it done.
            meta.output_commit = Some(commit.clone());
            meta.parent = Some(current);
            meta.author = author;
            let hash = store.put_object(&meta, &body)?;
            store.set_ref(&id, &hash)?;
            store.append_status(
                &id,
                &StatusEvent {
                    at: now_millis(),
                    status: "done".into(),
                    author,
                    version: hash.clone(),
                },
            )?;

            // 3. Commit the store change.
            git::commit_path(&base, &store.store_name(), &format!("llaundry: complete {id}"))?;

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
            let reasons = staleness(&store, &meta);
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
            let mut found = false;
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                let reasons = staleness(&store, &meta);
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

/// Collect explicit reasons a node version is stale, if any.
///
/// Two independent sources of staleness:
///   * an edge whose target has moved past the pinned version (or vanished), and
///   * an output file that has changed since the node's output commit (per git).
fn staleness(store: &Store, meta: &Meta) -> Vec<String> {
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
        match git::output_drift(&store.project_root(), commit) {
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
