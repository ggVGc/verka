//! `llaundry` — a tiny CLI over a content-addressed, immutable node graph.
//!
//! This binary is a thin shell: it parses arguments, opens the store, wires up the
//! real [`GitVcs`], and delegates every operation to the `llaundry` library. See
//! DESIGN.md for the model and the reasoning behind it.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use llaundry::ops::{self, NewNode};
use llaundry::{Author, GitVcs, NodeType, Status, Store};

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

    /// Find which node produced a given output commit.
    Origin {
        /// The output commit hash to trace back to its node.
        commit: String,
    },

    /// Show the output commit a node produced, if any.
    Outputs { id: String },
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
            let vcs = GitVcs::new(store.project_root());
            let (logical_id, hash) = ops::add(
                &store,
                &vcs,
                NewNode {
                    node_type,
                    title,
                    body: read_body(body, file)?,
                    author,
                    depends_on,
                    derived_from,
                    inputs: to_strings(&input),
                },
            )?;
            println!("{logical_id}  {}", ops::short(&hash));
        }

        Cmd::Link {
            from,
            to,
            rel,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::new(store.project_root());
            let hash = ops::link(&store, &vcs, &from, &to, &rel, author)?;
            println!("{from}  {}  (+{rel} -> {to})", ops::short(&hash));
        }

        Cmd::Edit {
            id,
            title,
            body,
            file,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::new(store.project_root());
            let new_body = if body.is_some() || file.is_some() {
                Some(read_body(body, file)?)
            } else {
                None
            };
            let hash = ops::edit(&store, &vcs, &id, title, new_body, author)?;
            println!("{id}  {}", ops::short(&hash));
        }

        Cmd::Complete {
            id,
            outputs,
            context,
            message,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::new(store.project_root());
            let (hash, commit) = ops::complete(
                &store,
                &vcs,
                &id,
                &to_strings(&outputs),
                &to_strings(&context),
                message,
                author,
            )?;
            println!("{id}  {}  (output {})", ops::short(&hash), ops::short(&commit));
        }

        Cmd::SetStatus {
            id,
            status,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::new(store.project_root());
            ops::set_status(&store, &vcs, &id, status, author)?;
            println!("{id}  -> {}", status.as_str());
        }

        Cmd::Show { id } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::new(store.project_root());
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
                println!("parent:  {}", ops::short(parent));
            }
            if !meta.edges.is_empty() {
                println!("edges:");
                for e in &meta.edges {
                    println!("  {:<12} -> {} @ {}", e.rel, e.to, ops::short(&e.pin));
                }
            }
            if !meta.inputs.is_empty() {
                println!("inputs:");
                for p in &meta.inputs {
                    println!("  {} @ {}", p.path, ops::short(&p.content));
                }
            }
            if !meta.context.is_empty() {
                println!("context:");
                for p in &meta.context {
                    println!("  {} @ {}", p.path, ops::short(&p.content));
                }
            }
            if let Some(commit) = &meta.output_commit {
                println!("output:  commit {}", ops::short(commit));
            }
            let reasons = ops::staleness(&store, &vcs, &meta);
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
                println!("{}  {} {}", ops::short(&hash), meta.author.as_str(), meta.title);
                match meta.parent {
                    Some(parent) => hash = parent,
                    None => break,
                }
            }
        }

        Cmd::Stale => {
            let store = Store::open(store)?;
            let vcs = GitVcs::new(store.project_root());
            let mut found = false;
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                let reasons = ops::staleness(&store, &vcs, &meta);
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
            let vcs = GitVcs::new(store.project_root());
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                if ops::is_ready(&store, &vcs, &meta) {
                    println!("{:<30} {}", id, meta.title);
                }
            }
        }

        Cmd::Blocked => {
            let store = Store::open(store)?;
            let vcs = GitVcs::new(store.project_root());
            let mut any = false;
            for id in store.list_refs()? {
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                let blockers = ops::blockers(&store, &vcs, &meta);
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

        Cmd::Origin { commit } => {
            let store = Store::open(store)?;
            match ops::producer(&store, &commit)? {
                Some((id, version)) => println!("{id}  {}", ops::short(&version)),
                None => println!("no node produced {}", ops::short(&commit)),
            }
        }

        Cmd::Outputs { id } => {
            let store = Store::open(store)?;
            let hash = store.get_ref(&id)?;
            let (meta, _) = store.get_object(&hash)?;
            match meta.output_commit {
                Some(commit) => println!("{commit}"),
                None => println!("{id} has produced no output"),
            }
        }
    }
    Ok(())
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

/// Convert CLI path arguments to project-root-relative strings.
fn to_strings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}
