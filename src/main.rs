//! `llaundry` — a tiny CLI over a git-versioned node graph.
//!
//! This binary is a thin shell: it parses arguments, opens the store, wires up the
//! real [`GitVcs`], and delegates every operation to the `llaundry` library. See
//! DESIGN.md for the model and the reasoning behind it.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io;
use std::path::PathBuf;

use llaundry::ops::{self, NewNode};
use llaundry::{Author, DepKind, GitVcs, Store};

#[derive(Parser)]
#[command(
    name = "llaundry",
    version,
    about = "A git-versioned graph of LLM-development nodes"
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

    /// Add a new node. Prints its id.
    Add {
        /// The node's description (markdown); its first line serves as the title.
        #[arg(long, required_unless_present = "file")]
        description: Option<String>,
        /// Description read from a file (mutually exclusive with --description).
        #[arg(long, conflicts_with = "description")]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
        /// Who the work is for (e.g. `human` for a question node). Unset means
        /// anyone may work it.
        #[arg(long, value_enum)]
        assignee: Option<Author>,
        /// Another node this one depends on (repeatable), by id.
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        /// Another node this one is derived from (repeatable), by id.
        #[arg(long = "derived-from")]
        derived_from: Vec<String>,
    },

    /// Add <to> to one of <from>'s dependency lists (a definition change).
    Link {
        /// Source node (the one that gains the dependency).
        from: String,
        /// Target node.
        to: String,
        #[arg(long, value_enum, default_value = "depends-on")]
        rel: DepKind,
    },

    /// Edit a node's description (a definition change: reopens a done node
    /// and makes dependents' pins stale).
    Edit {
        id: String,
        /// The new description; its first line serves as the title.
        #[arg(long, required_unless_present = "file")]
        description: Option<String>,
        #[arg(long, conflicts_with = "description")]
        file: Option<PathBuf>,
    },

    /// Record a node's work as done: commit the produced files as one output
    /// commit, pin what the work was built against, and write result.md.
    Complete {
        id: String,
        /// A produced file, relative to the project root (repeatable). May be
        /// omitted entirely for graph-only work that produces no files.
        #[arg(long = "output", short = 'o')]
        outputs: Vec<PathBuf>,
        /// A consumed file that is not any node's output (repeatable). Pinned by
        /// content, so a later change to it flags this node.
        #[arg(long = "context", short = 'c')]
        context: Vec<PathBuf>,
        /// Message for the output commit (defaults to the first line of the
        /// node's description).
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// Narrative of what happened during the work (the body of result.md).
        #[arg(long)]
        notes: Option<String>,
        /// Read the notes from a file instead.
        #[arg(long, conflicts_with = "notes")]
        notes_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "machine")]
        author: Author,
    },

    /// Record a node's work as failed, with notes on what went wrong.
    Fail {
        id: String,
        #[arg(long)]
        notes: Option<String>,
        /// Read the notes from a file instead.
        #[arg(long, conflicts_with = "notes")]
        notes_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "machine")]
        author: Author,
    },

    /// Show a node: definition, derived status, result, and staleness reasons.
    Show { id: String },

    /// List every node with its derived status.
    List,

    /// Show a node's git history (every definition and result change).
    Log { id: String },

    /// Report nodes whose recorded work has been invalidated, with reasons.
    Stale,

    /// List unfinished nodes whose dependencies are all satisfied (done, not stale).
    Ready {
        /// Only nodes assigned to this worker kind (e.g. `human`: the inbox of
        /// pending questions). Unassigned nodes match either.
        #[arg(long = "for", value_enum)]
        assignee: Option<Author>,
    },

    /// List nodes blocked by an unsatisfied dependency, with reasons.
    Blocked,

    /// Find which node's work produced a given output commit.
    Origin {
        /// The output commit hash to trace back to its node.
        commit: String,
    },

    /// Show the output commit a node produced, if any.
    Outputs { id: String },

    /// List the nodes that depend on (or derive from) a node.
    Dependents { id: String },

    /// Integrity-check the store (fsck): parse errors, missing edge targets,
    /// duplicates, self-references, and dependency cycles. Exits non-zero if
    /// problems are found.
    Check,

    /// Check whether a node is settled: done, not stale, and all work derived
    /// from it (transitively) also done and not stale. Exits non-zero if not.
    Settled { id: String },
}

fn main() -> Result<()> {
    let Cli { store, cmd } = Cli::parse();
    match cmd {
        Cmd::Init => {
            let store = Store::init(store)?;
            // Two separate repositories: the workbench holds the store's
            // history; the project is an ordinary repo of its own (move an
            // existing checkout into place and its repo is simply kept).
            for (name, dir) in [
                ("workbench", store.workbench_root()),
                ("project", store.project_root()),
            ] {
                if llaundry::git::ensure_repo(&dir)? {
                    println!("initialised {name} repository at {}", dir.display());
                }
            }
            println!(
                "initialised llaundry workbench (store {}, project {})",
                store.workbench_root().join(store.store_name()).display(),
                store.project_root().display()
            );
        }

        Cmd::Add {
            description,
            file,
            author,
            assignee,
            depends_on,
            derived_from,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let id = ops::add(
                &store,
                &vcs,
                NewNode {
                    description: read_description(description, file)?,
                    author,
                    assignee,
                    depends_on,
                    derived_from,
                },
            )?;
            println!("{id}");
        }

        Cmd::Link { from, to, rel } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            ops::link(&store, &vcs, &from, &to, rel)?;
            println!("{from}  +{} -> {to}", rel.as_str());
        }

        Cmd::Edit { id, description, file } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            ops::edit(&store, &vcs, &id, read_description(description, file)?)?;
            println!("{id}  {}", ops::short(&store.node_version(&id)?));
        }

        Cmd::Complete {
            id,
            outputs,
            context,
            message,
            notes,
            notes_file,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let notes = resolve_notes(notes, notes_file, &store, &id, "what happened?")?;
            let commit = ops::complete(
                &store,
                &vcs,
                &id,
                &to_strings(&outputs),
                &to_strings(&context),
                message,
                &notes,
                author,
            )?;
            match commit {
                Some(c) => println!("{id}  done  (output {})", ops::short(&c)),
                None => println!("{id}  done  (no output files)"),
            }
        }

        Cmd::Fail {
            id,
            notes,
            notes_file,
            author,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let notes = resolve_notes(notes, notes_file, &store, &id, "what went wrong?")?;
            ops::fail(&store, &vcs, &id, &notes, author)?;
            println!("{id}  failed");
        }

        Cmd::Show { id } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            print!("{}", show_node(&store, &vcs, &id)?);
        }

        Cmd::List => {
            let store = Store::open(store)?;
            for id in store.list_ids()? {
                let (_, description) = store.read_node(&id)?;
                let status = ops::current_status(&store, &id);
                println!("{:<32} {:<8} {}", id, status.as_str(), llaundry::title_of(&description));
            }
        }

        Cmd::Log { id } => {
            let store = Store::open(store)?;
            if !store.exists(&id) {
                anyhow::bail!("unknown node `{id}`");
            }
            // A node's history *is* git history — the workbench repo's: every
            // definition edit and every result is a commit touching its directory.
            let pathspec = format!("{}/nodes/{id}", store.store_name());
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(store.workbench_root())
                .args(["log", "--oneline", "--stat", "--", &pathspec])
                .status()
                .context("failed to run git log")?;
            if !status.success() {
                anyhow::bail!("git log failed");
            }
        }

        Cmd::Stale => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let mut found = false;
            for id in store.list_ids()? {
                let reasons = ops::staleness(&store, &vcs, &id);
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

        Cmd::Ready { assignee } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            for id in store.list_ids()? {
                if ops::is_ready(&store, &vcs, &id) {
                    let (meta, description) = store.read_node(&id)?;
                    // With --for, a node assigned to the *other* kind is hidden;
                    // unassigned nodes are anyone's to pick up.
                    if matches!((assignee, meta.assignee), (Some(want), Some(has)) if want != has) {
                        continue;
                    }
                    println!("{:<32} {}", id, llaundry::title_of(&description));
                }
            }
        }

        Cmd::Blocked => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let mut any = false;
            for id in store.list_ids()? {
                let blockers = ops::blockers(&store, &vcs, &id);
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
            match ops::origin(&store, &commit)? {
                Some(id) => println!("{id}"),
                None => println!("no node produced {}", ops::short(&commit)),
            }
        }

        Cmd::Outputs { id } => {
            let store = Store::open(store)?;
            if !store.exists(&id) {
                anyhow::bail!("unknown node `{id}`");
            }
            match ops::output_of(&store, &id) {
                Some(commit) => println!("{commit}"),
                None => println!("{id} has produced no output"),
            }
        }

        Cmd::Check => {
            let store = Store::open(store)?;
            let problems = ops::check(&store)?;
            if problems.is_empty() {
                println!("store is consistent");
            } else {
                for p in &problems {
                    println!("{p}");
                }
                eprintln!("{} problem(s) found", problems.len());
                std::process::exit(1);
            }
        }

        Cmd::Settled { id } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let reasons = ops::unsettled(&store, &vcs, &id)?;
            if reasons.is_empty() {
                println!("{id}: settled");
            } else {
                println!("{id}: not settled");
                for r in &reasons {
                    println!("  {r}");
                }
                std::process::exit(1);
            }
        }

        Cmd::Dependents { id } => {
            let store = Store::open(store)?;
            if !store.exists(&id) {
                anyhow::bail!("unknown node `{id}`");
            }
            for dep in ops::dependents(&store, &id)? {
                println!("{dep}");
            }
        }
    }
    Ok(())
}

/// The `show` view, shared in spirit with the MCP server's `show_node`.
fn show_node(store: &Store, vcs: &GitVcs, id: &str) -> Result<String> {
    let (meta, description) = store.read_node(id)?;
    let mut out = String::new();
    use std::fmt::Write;

    writeln!(out, "id:      {id}")?;
    writeln!(out, "status:  {}", ops::current_status(store, id).as_str())?;
    writeln!(out, "author:  {}", meta.author.as_str())?;
    if let Some(assignee) = meta.assignee {
        writeln!(out, "assignee: {}", assignee.as_str())?;
    }
    writeln!(out, "version: {}", ops::short(&store.node_version(id)?))?;
    for dep in &meta.depends_on {
        writeln!(out, "depends_on:   {dep}")?;
    }
    for src in &meta.derived_from {
        writeln!(out, "derived_from: {src}")?;
    }

    if let Some((result, notes)) = store.read_result(id)? {
        writeln!(out, "result:")?;
        writeln!(out, "  outcome: {}", result.outcome.as_str())?;
        writeln!(out, "  author:  {}", result.author.as_str())?;
        if let Some(wb) = &result.worked_by {
            match &wb.model {
                Some(m) => writeln!(out, "  worked by: {} ({m})", wb.backend)?,
                None => writeln!(out, "  worked by: {}", wb.backend)?,
            }
        }
        if let Some(commit) = &result.output_commit {
            writeln!(out, "  output:  commit {}", ops::short(commit))?;
        }
        for ba in &result.built_against {
            match &ba.output {
                Some(o) => writeln!(
                    out,
                    "  built against {} @ {} (output {})",
                    ba.id,
                    ops::short(&ba.pin),
                    ops::short(o)
                )?,
                None => writeln!(out, "  built against {} @ {}", ba.id, ops::short(&ba.pin))?,
            }
        }
        for pin in &result.context {
            let tag = if pin.observed { " (observed)" } else { "" };
            writeln!(out, "  context {} @ {}{tag}", pin.path, ops::short(&pin.blob))?;
        }
        let notes = notes.trim_end();
        if !notes.is_empty() {
            writeln!(out, "  notes:")?;
            for line in notes.lines() {
                writeln!(out, "    {line}")?;
            }
        }
    }

    let reasons = ops::staleness(store, vcs, id);
    if !reasons.is_empty() {
        writeln!(out, "stale:")?;
        for r in &reasons {
            writeln!(out, "  {r}")?;
        }
    }
    let description = description.trim_end();
    if !description.is_empty() {
        writeln!(out, "\n{description}")?;
    }
    Ok(out)
}

/// Resolve the notes for `complete`/`fail`: `--notes` inline, `--notes-file`
/// from a file, or — when neither is given and we are on a terminal — a
/// git-commit-style `$EDITOR` session. Non-interactive callers (agents, scripts)
/// that pass nothing get empty notes, unchanged from before.
fn resolve_notes(
    notes: Option<String>,
    file: Option<PathBuf>,
    store: &Store,
    id: &str,
    ask: &str,
) -> Result<String> {
    use std::io::IsTerminal;
    if let Some(n) = notes {
        return Ok(n);
    }
    if let Some(f) = file {
        return std::fs::read_to_string(&f)
            .with_context(|| format!("reading notes from {}", f.display()));
    }
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Ok(String::new());
    }

    // Interactive and no notes supplied: open $VISUAL/$EDITOR on a template,
    // git-commit style. '#' lines are stripped from the result.
    let (_, description) = store.read_node(id)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    let path = std::env::temp_dir().join(format!("llaundry-notes-{id}.md"));
    std::fs::write(
        &path,
        format!(
            "\n# Notes for {id} — {}\n# {ask} These notes become the body of result.md.\n# Lines starting with '#' are ignored; an empty file records no notes.\n",
            llaundry::title_of(&description)
        ),
    )?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} '{}'", path.display()))
        .status()
        .with_context(|| format!("failed to launch editor `{editor}` (set $EDITOR)"))?;
    if !status.success() {
        anyhow::bail!("editor `{editor}` exited unsuccessfully; aborting");
    }
    let text = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);
    Ok(strip_comment_lines(&text))
}

/// Drop lines starting with '#' and trim surrounding blank space — the
/// git-commit template convention.
fn strip_comment_lines(text: &str) -> String {
    let kept: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
    kept.join("\n").trim().to_string()
}

fn read_description(description: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match (description, file) {
        (Some(d), _) => Ok(d),
        (None, Some(f)) => std::fs::read_to_string(&f)
            .with_context(|| format!("reading description from {}", f.display())),
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

#[cfg(test)]
mod tests {
    use super::strip_comment_lines;

    #[test]
    fn strip_comment_lines_follows_the_git_template_convention() {
        let text = "\n# Notes for task-1 — title\n# ignored\nDid the work.\n\nMore detail.\n# trailing comment\n";
        assert_eq!(strip_comment_lines(text), "Did the work.\n\nMore detail.");
        assert_eq!(strip_comment_lines("# only comments\n#\n"), "");
        assert_eq!(strip_comment_lines(""), "");
    }
}
