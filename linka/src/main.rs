//! `linka` — a tiny CLI over a git-versioned node graph.
//!
//! This binary is a thin shell: it parses arguments, opens the store, wires up the
//! real [`GitVcs`], and delegates every operation to the `linka` library. See
//! DESIGN.md for the model and the reasoning behind it.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io;
use std::path::PathBuf;

use linka::model::{Blocker, BlockerReason, NodeState, StalenessReason};
use linka::ops::{self, NewNode};
use linka::{Author, DepKind, GitVcs, NodeId, ProjectPath, Store};

mod journal;

#[derive(Parser)]
#[command(
    name = "linka",
    version,
    about = "A git-versioned graph of LLM-development nodes"
)]
struct Cli {
    /// Path to the store directory.
    #[arg(long, env = "LINKA_DIR", default_value = ".linka", global = true)]
    store: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new, empty store.
    Init {
        /// A descriptive short-name for the project, recorded on the pairing
        /// for human readers (never checked).
        #[arg(long)]
        name: Option<String>,
    },

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
        depends_on: Vec<NodeId>,
        /// Another node this one is derived from (repeatable), by id.
        #[arg(long = "derived-from")]
        derived_from: Vec<NodeId>,
    },

    /// Add <to> to one of <from>'s dependency lists (a definition change).
    Link {
        /// Source node (the one that gains the dependency).
        from: NodeId,
        /// Target node.
        to: NodeId,
        #[arg(long, value_enum, default_value = "depends-on")]
        rel: DepKind,
    },

    /// Edit a node's description (a definition change: reopens a done node
    /// and makes dependents' pins stale).
    Edit {
        id: NodeId,
        /// The new description; its first line serves as the title.
        #[arg(long, required_unless_present = "file")]
        description: Option<String>,
        #[arg(long, conflicts_with = "description")]
        file: Option<PathBuf>,
    },

    /// Record a node's work as done: commit the produced files as one output
    /// commit, pin what the work was built against, and write its result files.
    Complete {
        id: NodeId,
        /// A produced file, relative to the project root (repeatable). May be
        /// omitted entirely for graph-only work that produces no files.
        #[arg(long = "output", short = 'o')]
        outputs: Vec<ProjectPath>,
        /// A consumed file that is not any node's output (repeatable). Pinned by
        /// content, so a later change to it flags this node.
        #[arg(long = "context", short = 'c')]
        context: Vec<ProjectPath>,
        /// Message for the output commit (defaults to the first line of the
        /// node's description).
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// Narrative of what happened during the work (written to result.md).
        #[arg(long)]
        notes: Option<String>,
        /// Read the notes from a file instead.
        #[arg(long, conflicts_with = "notes")]
        notes_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Resume an interrupted recoverable completion journal.
    Recover { submission: String },

    /// Record a node's work as failed, with notes on what went wrong.
    Fail {
        id: NodeId,
        #[arg(long)]
        notes: Option<String>,
        /// Read the notes from a file instead.
        #[arg(long, conflicts_with = "notes")]
        notes_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Show a node: definition, derived status, result, and staleness reasons.
    Show { id: NodeId },

    /// List every node with its derived status.
    List,

    /// Show a node's git history (every definition and result change).
    Log { id: NodeId },

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
    Outputs { id: NodeId },

    /// List the nodes that depend on (or derive from) a node.
    Dependents { id: NodeId },

    /// Integrity-check the store (fsck): parse errors, missing edge targets,
    /// duplicates, self-references, and dependency cycles. Exits non-zero if
    /// problems are found.
    Check {
        /// Also verify that referenced output artifacts exist and are retained.
        #[arg(long)]
        artifacts: bool,
    },

    /// Preview or apply deterministic stored-schema upgrades.
    Migrate {
        /// Preview required changes without writing them.
        #[arg(long)]
        check: bool,
    },

    /// Check whether a node is settled: done, not stale, and all work derived
    /// from it (transitively) also done and not stale. Exits non-zero if not.
    Settled { id: NodeId },

    /// Record which project repository this store describes, keyed by the
    /// project's root commit — or, with --verify, check the recorded pairing.
    Pair {
        /// Verify the recorded pairing instead of recording one (read-only).
        /// Exits non-zero if the pairing does not hold.
        #[arg(long)]
        verify: bool,
        /// With --verify: also check that every recorded output commit still
        /// exists in the project repository (detects history rewrites that
        /// leave the root commit intact).
        #[arg(long, requires = "verify")]
        deep: bool,
        /// Re-pair even if the store is paired to a different root (after a
        /// deliberate history rewrite).
        #[arg(long, conflicts_with = "verify")]
        force: bool,
        /// A descriptive short-name for the project, recorded on the pairing
        /// for human readers (never checked). Updatable on a re-pair.
        #[arg(long, conflicts_with = "verify")]
        name: Option<String>,
    },
}

fn main() -> Result<()> {
    let Cli { store, cmd } = Cli::parse();
    match cmd {
        Cmd::Init { name } => {
            let initialized = ops::init_workbench(store, name)?;
            let store = &initialized.store;
            if initialized.created_workbench_repo {
                println!(
                    "initialised workbench repository at {}",
                    store.workbench_root().display()
                );
            }
            if initialized.created_project_repo {
                println!(
                    "initialised project repository at {}",
                    store.project_root().display()
                );
            }
            println!(
                "initialised linka workbench (store {}, project {})",
                store.workbench_root().join(store.store_name()).display(),
                store.project_root().display()
            );
            // Pair the store to the project by its root commit; a fresh
            // project gets an empty root commit so it has an identity to
            // anchor to (an adopted checkout already has one).
            if initialized.created_project_root {
                println!("created empty root commit in the project repository");
            }
            println!("{}", pairing_line(&initialized.pairing));
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
                    depends_on: depends_on.into_iter().map(Into::into).collect(),
                    derived_from: derived_from.into_iter().map(Into::into).collect(),
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

        Cmd::Edit {
            id,
            description,
            file,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            ops::edit(&store, &vcs, &id, read_description(description, file)?)?;
            println!("{id}  {}", ops::short_definition(&store.node_version(&id)?));
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
            let (_, description) = store.read_node(&id)?;
            let output_message =
                message.unwrap_or_else(|| linka::title_of(&description).to_string());
            let commit = journal::complete(
                &store,
                &vcs,
                &id,
                &to_strings(&outputs),
                &to_strings(&context),
                output_message,
                notes,
                author,
            )?;
            match commit {
                Some(c) => println!("{id}  done  (output {})", ops::short(&c)),
                None => println!("{id}  done  (no output files)"),
            }
        }

        Cmd::Recover { submission } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let mut record = journal::load(&store, &submission)?;
            journal::recover(&store, &vcs, &mut record)?;
            println!("{}  {:?}", record.id, record.phase);
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
            let vcs = GitVcs::for_store(&store);
            let mut errors = 0;
            for id in store.list_ids()? {
                let (_, description) = match store.read_node(&id) {
                    Ok(node) => node,
                    Err(error) => {
                        report_node_error(&id, &error);
                        errors += 1;
                        continue;
                    }
                };
                let state = match ops::node_state(&store, &vcs, &id) {
                    Ok(state) => state,
                    Err(error) => {
                        report_node_error(&id, &error);
                        errors += 1;
                        continue;
                    }
                };
                println!(
                    "{:<32} {:<8} {}",
                    id,
                    state_summary(&state),
                    linka::title_of(&description)
                );
            }
            finish_node_queries(errors)?;
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
            let mut errors = 0;
            for id in store.list_ids()? {
                let reasons = match ops::staleness(&store, &vcs, &id) {
                    Ok(reasons) => reasons,
                    Err(error) => {
                        report_node_error(&id, &error);
                        errors += 1;
                        continue;
                    }
                };
                if !reasons.is_empty() {
                    found = true;
                    println!("{id}:");
                    for r in &reasons {
                        println!("  {}", format_staleness(r));
                    }
                }
            }
            if !found && errors == 0 {
                println!("all nodes up to date");
            }
            finish_node_queries(errors)?;
        }

        Cmd::Ready { assignee } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let mut errors = 0;
            for id in store.list_ids()? {
                let (meta, description) = match store.read_node(&id) {
                    Ok(node) => node,
                    Err(error) => {
                        report_node_error(&id, &error);
                        errors += 1;
                        continue;
                    }
                };
                match ops::node_state(&store, &vcs, &id) {
                    Ok(state)
                        if state.is_ready()
                            && !matches!((assignee, meta.assignee), (Some(want), Some(has)) if want != has) =>
                    {
                        println!(
                            "{:<32} {}  {}",
                            id,
                            state_summary(&state),
                            linka::title_of(&description)
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        report_node_error(&id, &error);
                        errors += 1;
                    }
                }
            }
            finish_node_queries(errors)?;
        }

        Cmd::Blocked => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            let mut any = false;
            let mut errors = 0;
            for id in store.list_ids()? {
                let blockers = match ops::blockers(&store, &vcs, &id) {
                    Ok(blockers) => blockers,
                    Err(error) => {
                        report_node_error(&id, &error);
                        errors += 1;
                        continue;
                    }
                };
                if !blockers.is_empty() {
                    any = true;
                    println!("{id}:");
                    for b in &blockers {
                        println!("  blocked by {}", format_blocker(b));
                    }
                }
            }
            if !any && errors == 0 {
                println!("nothing blocked");
            }
            finish_node_queries(errors)?;
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
            match ops::output_of(&store, &id)? {
                Some(commit) => println!("{commit}"),
                None => println!("{id} has produced no output"),
            }
        }

        Cmd::Check { artifacts } => {
            let store = Store::open(store)?;
            let problems = if artifacts {
                let vcs = GitVcs::for_store(&store);
                ops::check_artifacts(&store, &vcs)?
            } else {
                ops::check(&store)?
            };
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

        Cmd::Migrate { check } => {
            let store = Store::open(store)?;
            let changes = if check {
                ops::migration_plan(&store)?
            } else {
                let vcs = GitVcs::for_store(&store);
                ops::migrate(&store, &vcs)?
            };
            if changes.is_empty() {
                println!("store schema is current");
            } else {
                for change in &changes {
                    println!("{change}");
                }
                if check {
                    std::process::exit(1);
                }
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

        Cmd::Pair {
            verify,
            deep,
            force,
            name,
        } => {
            let store = Store::open(store)?;
            let vcs = GitVcs::for_store(&store);
            if verify {
                let (recorded, problems) = ops::verify_pairing(&store, &vcs, deep)?;
                match recorded {
                    None => {
                        println!("store is not paired (run `linka pair` to record the project)")
                    }
                    Some(pairing) if problems.is_empty() => {
                        println!("{} — ok", pairing_line(&pairing));
                    }
                    Some(_) => {
                        for p in &problems {
                            println!("{p}");
                        }
                        eprintln!("{} problem(s) found", problems.len());
                        std::process::exit(1);
                    }
                }
            } else {
                let pairing = ops::pair(&store, &vcs, name, force)?;
                println!("{}", pairing_line(&pairing));
            }
        }

        Cmd::Dependents { id } => {
            let store = Store::open(store)?;
            for dep in ops::dependents(&store, &id)? {
                println!("{dep}");
            }
        }
    }
    Ok(())
}

/// One human line describing a pairing: the checked root, then whatever
/// informational fields it carries.
fn pairing_line(pairing: &linka::Pairing) -> String {
    let mut line = format!(
        "paired to project root {}",
        ops::short(&pairing.root_commit)
    );
    if let Some(name) = &pairing.name {
        line.push_str(&format!(" ({name})"));
    }
    if let Some(remote) = &pairing.remote {
        line.push_str(&format!(", remote {remote}"));
    }
    line
}

fn report_node_error(id: &str, error: &anyhow::Error) {
    eprintln!("{id}: error: {error:#}");
}

fn finish_node_queries(errors: usize) -> Result<()> {
    if errors > 0 {
        anyhow::bail!("could not evaluate {errors} node(s)");
    }
    Ok(())
}

fn state_summary(state: &NodeState) -> String {
    if state.is_complete() {
        return "complete".into();
    }
    if state.is_ready() {
        if state.currency == linka::Currency::Stale {
            let reason = state
                .staleness
                .first()
                .map(format_staleness)
                .unwrap_or_else(|| "recorded evidence changed".into());
            return format!("ready (previous result stale: {reason})");
        }
        if state.outcome == linka::RecordedOutcome::Failed {
            return "ready (previous attempt failed)".into();
        }
        return "ready".into();
    }
    match state.blockers.first() {
        Some(blocker) => format!("blocked by {}", format_blocker(blocker)),
        None => "blocked".into(),
    }
}

fn format_blocker(blocker: &Blocker) -> String {
    let reason = match blocker.reason {
        BlockerReason::Missing => "missing",
        BlockerReason::Open => "not complete (open)",
        BlockerReason::Failed => "not complete (failed)",
        BlockerReason::Stale => "not complete (stale)",
    };
    format!("{}: {reason}", blocker.id)
}

fn format_staleness(reason: &StalenessReason) -> String {
    match reason {
        StalenessReason::DefinitionChanged {
            metadata,
            description,
        } => {
            let mut files = Vec::new();
            if *metadata {
                files.push("node.toml");
            }
            if *description {
                files.push("description.md");
            }
            format!("definition changed since the work ({})", files.join(", "))
        }
        StalenessReason::ConsumedDefinitionChanged { id } => {
            format!("dependency {id}: definition moved")
        }
        StalenessReason::ConsumedNodeMissing { id } => format!("dependency {id}: missing"),
        StalenessReason::ConsumedResultChanged { id } => {
            format!("dependency {id}: result changed since it was consumed")
        }
        StalenessReason::ConsumedOutputChanged { id } => format!("dependency {id}: output changed"),
        StalenessReason::ContextChanged { path } => format!("context {path}: content changed"),
        StalenessReason::ContextMissing { path } => format!("context {path}: missing"),
        StalenessReason::OutputDrifted { artifact, detail } => format!(
            "output changed since {artifact}:\n      {}",
            detail.replace('\n', "\n      ")
        ),
    }
}

/// The `show` view.
fn show_node(store: &Store, vcs: &GitVcs, id: &str) -> Result<String> {
    let (meta, description) = store.read_node(id)?;
    let mut out = String::new();
    use std::fmt::Write;

    writeln!(out, "id:      {id}")?;
    let state = ops::node_state(store, vcs, id)?;
    writeln!(out, "status:  {}", state_summary(&state))?;
    writeln!(out, "author:  {}", meta.author.as_str())?;
    if let Some(assignee) = meta.assignee {
        writeln!(out, "assignee: {}", assignee.as_str())?;
    }
    writeln!(
        out,
        "version: {}",
        ops::short_definition(&store.node_version(id)?)
    )?;
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
        if result.project.is_none() {
            writeln!(out, "  warning: legacy result has no project revision")?;
        }
        if let Some(producer) = &result.producer {
            writeln!(out, "  producer: {} {}", producer.namespace, producer.data)?;
        }
        if let Some(commit) = ops::output_commit(&result) {
            writeln!(out, "  output:  commit {}", ops::short(commit))?;
        }
        for ba in &result.consumed {
            let result_pin = ba
                .result
                .as_ref()
                .map_or_else(|| "none".into(), ops::short_result);
            match &ba.output {
                Some(o) => writeln!(
                    out,
                    "  built against {} @ {} (result {}, output {})",
                    ba.id,
                    ops::short_definition(&ba.definition),
                    result_pin,
                    ops::short(&o.id)
                )?,
                None => writeln!(
                    out,
                    "  built against {} @ {} (result {})",
                    ba.id,
                    ops::short_definition(&ba.definition),
                    result_pin
                )?,
            }
        }
        for pin in &result.context {
            let tag = if pin.observed { " (observed)" } else { "" };
            writeln!(
                out,
                "  context {} @ {}{tag}",
                pin.path,
                ops::short(&pin.identity)
            )?;
        }
        let notes = notes.trim_end();
        if !notes.is_empty() {
            writeln!(out, "  notes:")?;
            for line in notes.lines() {
                writeln!(out, "    {line}")?;
            }
        }
    }

    let reasons = state.staleness;
    if !reasons.is_empty() {
        writeln!(out, "stale:")?;
        for r in &reasons {
            writeln!(out, "  {}", format_staleness(r))?;
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
    let path = std::env::temp_dir().join(format!("linka-notes-{id}.md"));
    std::fs::write(
        &path,
        format!(
            "\n# Notes for {id} — {}\n# {ask} These notes become the body of result.md.\n# Lines starting with '#' are ignored; an empty file records no notes.\n",
            linka::title_of(&description)
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
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
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
fn to_strings(paths: &[ProjectPath]) -> Vec<String> {
    paths.iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::{state_summary, strip_comment_lines};
    use linka::{Blocker, BlockerReason, Currency, NodeState, RecordedOutcome, StalenessReason};

    #[test]
    fn strip_comment_lines_follows_the_git_template_convention() {
        let text = "\n# Notes for task-1 — title\n# ignored\nDid the work.\n\nMore detail.\n# trailing comment\n";
        assert_eq!(strip_comment_lines(text), "Did the work.\n\nMore detail.");
        assert_eq!(strip_comment_lines("# only comments\n#\n"), "");
        assert_eq!(strip_comment_lines(""), "");
    }

    #[test]
    fn state_summary_distinguishes_prior_evidence_and_blocking() {
        let stale = NodeState {
            outcome: RecordedOutcome::Succeeded,
            currency: Currency::Stale,
            staleness: vec![StalenessReason::ContextMissing {
                path: "input".into(),
            }],
            blockers: vec![],
        };
        assert!(state_summary(&stale).starts_with("ready (previous result stale:"));
        let failed = NodeState {
            outcome: RecordedOutcome::Failed,
            currency: Currency::Current,
            staleness: vec![],
            blockers: vec![],
        };
        assert_eq!(state_summary(&failed), "ready (previous attempt failed)");
        let blocked = NodeState {
            blockers: vec![Blocker {
                id: "dependency".into(),
                reason: BlockerReason::Stale,
            }],
            ..failed
        };
        assert_eq!(
            state_summary(&blocked),
            "blocked by dependency: not complete (stale)"
        );
    }
}
