//! `linka` — a tiny CLI over a git-versioned graph of work nodes.
//!
//! This binary is a thin shell: it parses arguments, opens the store, wires up
//! the real [`GitVcs`], and delegates every operation to the library. All human
//! formatting lives in [`render`]. See DESIGN.md for the model.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, Write};
use std::path::PathBuf;

use linka::graph::Graph;
use linka::model::{Conclusion, NewAttachment, NewCandidate, Submission};
use linka::ops::{self, NewNode};
use linka::{
    check, AttachmentKey, Author, CandidateId, DepKind, GitVcs, Namespace, NodeId, ProjectPath,
    Store,
};

mod render;

#[derive(Parser)]
#[command(name = "linka", version, about = "A git-versioned graph of work nodes")]
struct Cli {
    /// Path to the store directory.
    #[arg(long, env = "LINKA_DIR", default_value = ".linka", global = true)]
    store: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new workbench: the store, both repositories, and their pairing.
    Init {
        /// A descriptive short-name for the project, recorded on the pairing
        /// for human readers (never checked).
        #[arg(long)]
        name: Option<String>,
    },

    /// Add a node. Prints its id.
    Add {
        /// The node's description; its first line names it. A review node
        /// falls back to naming the candidate it reviews.
        #[arg(long, required_unless_present_any = ["file", "verifies"])]
        description: Option<String>,
        /// Description read from a file (mutually exclusive with --description).
        #[arg(long, conflicts_with = "description")]
        file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
        /// Who the work is for (e.g. `human` for a question). Unset means
        /// anyone may work it.
        #[arg(long, value_enum)]
        assignee: Option<Author>,
        /// A node this one depends on (repeatable).
        #[arg(long = "depends-on")]
        depends_on: Vec<NodeId>,
        /// A node this one is derived from (repeatable).
        #[arg(long = "derived-from")]
        derived_from: Vec<NodeId>,
        /// Make this a review node for an exact candidate. Its source node is
        /// added as lineage, so the review pins the exact artifact.
        #[arg(long)]
        verifies: Option<CandidateId>,
    },

    /// Add <to> to one of <from>'s edge lists (a definition change).
    Link {
        /// The node that gains the edge.
        from: NodeId,
        to: NodeId,
        #[arg(long, value_enum, default_value = "depends-on")]
        rel: DepKind,
    },

    /// Edit a node's description (a definition change: it reopens a done node
    /// and makes dependents' pins stale).
    Edit {
        id: NodeId,
        #[arg(long, required_unless_present = "file")]
        description: Option<String>,
        #[arg(long, conflicts_with = "description")]
        file: Option<PathBuf>,
    },

    /// Record a node's work as done: commit the produced files as one output
    /// commit, pin what the work was built against, and write the result.
    Complete {
        id: NodeId,
        /// A produced file or directory, relative to the project root
        /// (repeatable). Omit entirely for work that produces no files.
        #[arg(long = "output", short = 'o')]
        outputs: Vec<ProjectPath>,
        /// A consumed file that is no node's output (repeatable). Pinned by
        /// content, so a later change to it flags this node.
        #[arg(long = "context", short = 'c')]
        context: Vec<ProjectPath>,
        /// Message for the output commit (defaults to the node's title).
        #[arg(long, short = 'm')]
        message: Option<String>,
        /// What happened during the work (written to result.md).
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, conflicts_with = "notes")]
        notes_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Record a node's work as failed, with notes on what went wrong.
    Fail {
        id: NodeId,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, conflicts_with = "notes")]
        notes_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Conclude a review node: accepted, rejected, or abandoned.
    Verify {
        id: NodeId,
        #[arg(long, value_enum)]
        outcome: ReviewOutcome,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, conflicts_with = "notes")]
        notes_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "human")]
        author: Author,
    },

    /// Propose a node's recorded output as a candidate for a target branch.
    Propose {
        id: NodeId,
        /// The branch the work sits on, for humans to look at.
        #[arg(long)]
        branch: String,
        /// The branch the candidate is intended for.
        #[arg(long)]
        target: String,
    },

    /// Publish an accepted candidate by idempotent fast-forward.
    Publish { id: CandidateId },

    /// Record context a node's work turned out to have read.
    Observe {
        id: NodeId,
        /// A consumed file, relative to the project root (repeatable).
        #[arg(long = "path", short = 'p', required = true)]
        paths: Vec<ProjectPath>,
    },

    /// Show a node: definition, derived state, result, and staleness.
    Show { id: NodeId },

    /// List every node with its derived state.
    List,

    /// List candidates, optionally only those of one source node.
    Candidates { node: Option<NodeId> },

    /// Show one candidate, its decision, and the reviews that decided it.
    Candidate { id: CandidateId },

    /// List the review nodes that verify a candidate.
    Verifications { candidate: CandidateId },

    /// Show a node's git history: every definition and result change.
    Log { id: NodeId },

    /// Commit opaque data associated with a node.
    Attach {
        id: NodeId,
        #[arg(long)]
        namespace: Namespace,
        #[arg(long)]
        key: AttachmentKey,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        media_type: Option<String>,
    },

    /// List opaque data attached to a node.
    Attachments { id: NodeId },

    /// Write one attachment's payload to standard output.
    Attachment {
        id: NodeId,
        namespace: Namespace,
        key: AttachmentKey,
    },

    /// Report nodes whose recorded work has been invalidated, with reasons.
    Stale,

    /// List work that is ready: not complete, with every dependency complete.
    Ready {
        /// Only nodes assigned to this worker kind (e.g. `human`: the inbox of
        /// pending questions). Unassigned nodes match either.
        #[arg(long = "for", value_enum)]
        assignee: Option<Author>,
    },

    /// List nodes blocked by an unsatisfied dependency, with reasons.
    Blocked,

    /// Find which node's work produced a given output commit.
    Origin { commit: String },

    /// Show the output commit a node produced, if any.
    Outputs { id: NodeId },

    /// List the nodes that depend on, or derive from, a node.
    Dependents { id: NodeId },

    /// Integrity-check the store (fsck). Exits non-zero if problems are found.
    Check {
        /// Also verify that recorded output artifacts exist and are retained.
        #[arg(long)]
        artifacts: bool,
    },

    /// Check whether a node is settled: it, and everything derived from it, is
    /// complete. Exits non-zero if not.
    Settled { id: NodeId },

    /// Record which project repository this store describes — or, with
    /// --verify, check the recorded pairing.
    Pair {
        /// Verify the recorded pairing instead of recording one (read-only).
        #[arg(long)]
        verify: bool,
        /// With --verify: also check that every recorded output commit still
        /// exists in the project repository.
        #[arg(long, requires = "verify")]
        deep: bool,
        /// Re-pair even if the store is paired to a different root.
        #[arg(long, conflicts_with = "verify")]
        force: bool,
        /// A descriptive short-name for the project (never checked).
        #[arg(long, conflicts_with = "verify")]
        name: Option<String>,
    },
}

/// The three conclusions a review may reach, as CLI values.
#[derive(Clone, Copy, clap::ValueEnum)]
enum ReviewOutcome {
    Accepted,
    Rejected,
    Abandoned,
}

impl ReviewOutcome {
    fn conclusion(self) -> Conclusion {
        match self {
            Self::Accepted => Conclusion::Accepted,
            Self::Rejected => Conclusion::Rejected,
            Self::Abandoned => Conclusion::Abandoned,
        }
    }
}

/// Open the store at `root` and wire up the real [`GitVcs`] against it.
fn open_store(root: PathBuf) -> Result<(Store, GitVcs)> {
    let store = Store::open(root)?;
    let vcs = GitVcs::for_store(&store);
    Ok((store, vcs))
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
            if initialized.created_project_root {
                println!("created empty root commit in the project repository");
            }
            println!("{}", render::pairing_line(&initialized.pairing));
        }

        Cmd::Add {
            description,
            file,
            author,
            assignee,
            depends_on,
            derived_from,
            verifies,
        } => {
            let (store, vcs) = open_store(store)?;
            let mut description = read_description(description, file)?;
            if description.trim().is_empty() {
                if let Some(candidate) = &verifies {
                    description = format!("Review candidate {candidate}");
                }
            }
            let id = ops::add(
                &store,
                &vcs,
                NewNode {
                    description,
                    author,
                    assignee,
                    depends_on,
                    derived_from,
                },
                verifies,
            )?;
            println!("{id}");
        }

        Cmd::Link { from, to, rel } => {
            let (store, vcs) = open_store(store)?;
            ops::link(&store, &vcs, &from, &to, rel)?;
            println!("{from}  +{} -> {to}", rel.as_str());
        }

        Cmd::Edit {
            id,
            description,
            file,
        } => {
            let (store, vcs) = open_store(store)?;
            let outcome = ops::edit(&store, &vcs, &id, read_description(description, file)?)?;
            let version = ops::short_definition(&store.node_version(&id)?);
            match outcome {
                ops::EditOutcome::Edited => println!("{id}  {version}"),
                ops::EditOutcome::Unchanged => println!("{id}  {version}  (unchanged)"),
            }
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
            let (store, vcs) = open_store(store)?;
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
                Some(commit) => println!("{id}  done  (output {})", ops::short(&commit)),
                None => println!("{id}  done  (no output files)"),
            }
        }

        Cmd::Fail {
            id,
            notes,
            notes_file,
            author,
        } => {
            let (store, vcs) = open_store(store)?;
            let notes = resolve_notes(notes, notes_file, &store, &id, "what went wrong?")?;
            conclude(&store, &vcs, &id, Conclusion::Failed, notes, author)?;
            println!("{id}  failed");
        }

        Cmd::Verify {
            id,
            outcome,
            notes,
            notes_file,
            author,
        } => {
            let (store, vcs) = open_store(store)?;
            let notes = resolve_notes(
                notes,
                notes_file,
                &store,
                &id,
                "what did the review conclude?",
            )?;
            let conclusion = outcome.conclusion();
            let word = conclusion.outcome().as_str();
            conclude(&store, &vcs, &id, conclusion, notes, author)?;
            println!("{id}  {word}");
        }

        Cmd::Propose { id, branch, target } => {
            let (store, vcs) = open_store(store)?;
            let candidate = ops::register_candidate(
                &store,
                &vcs,
                NewCandidate {
                    node: id,
                    branch,
                    target,
                    external: None,
                },
            )?;
            println!(
                "{}  artifact {}  -> {}",
                candidate.id,
                ops::short(&candidate.artifact.id),
                candidate.target
            );
        }

        Cmd::Publish { id } => {
            let (store, vcs) = open_store(store)?;
            let candidate = store.read_candidate(&id)?;
            let graph = Graph::load(&store, &vcs)?;
            match graph.decision(&id).map_err(anyhow::Error::msg)? {
                linka::CandidateDecision::Accepted => {}
                decision => anyhow::bail!(
                    "candidate `{id}` is {}, not accepted",
                    format!("{decision:?}").to_lowercase()
                ),
            }
            ops::publish(&vcs, &candidate)?;
            println!("published {id} onto {}", candidate.target);
        }

        Cmd::Observe { id, paths } => {
            let (store, vcs) = open_store(store)?;
            let version = store
                .current_result_version(&id)?
                .with_context(|| format!("node `{id}` has no result to observe context for"))?;
            let added =
                ops::record_observed_context(&store, &vcs, &id, &version, &to_strings(&paths))?;
            println!("{id}  {added} new context pin(s)");
        }

        Cmd::Show { id } => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            print!("{}", render::show(&store, &graph, &id)?);
        }

        Cmd::List => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            for id in graph.ids() {
                println!("{}", render::node_line(&store, &graph, id));
            }
        }

        Cmd::Candidates { node } => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            let candidates: Vec<_> = graph
                .candidates()
                .filter(|candidate| node.as_ref().is_none_or(|node| candidate.node == *node))
                .collect();
            if candidates.is_empty() {
                println!("no candidates");
            }
            for candidate in candidates {
                println!("{}", render::candidate_line(&graph, candidate));
            }
        }

        Cmd::Candidate { id } => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            let candidate = store.read_candidate(&id)?;
            print!("{}", render::candidate(&graph, &candidate)?);
        }

        Cmd::Verifications { candidate } => {
            let store = Store::open(store)?;
            let verifications = check::verifications_for(&store, &candidate)?;
            if verifications.is_empty() {
                println!("no verifications");
            }
            for verification in verifications {
                println!("{verification}");
            }
        }

        Cmd::Log { id } => {
            let store = Store::open(store)?;
            if !store.exists(&id) {
                anyhow::bail!("unknown node `{id}`");
            }
            // A node's history *is* git history — the workbench repository's:
            // every definition edit and every result is a commit touching its
            // directory.
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

        Cmd::Attach {
            id,
            namespace,
            key,
            file,
            media_type,
        } => {
            let (store, vcs) = open_store(store)?;
            let attachments = ops::attach(
                &store,
                &vcs,
                &id,
                vec![NewAttachment {
                    namespace,
                    key,
                    media_type,
                    data: std::fs::read(&file)
                        .with_context(|| format!("reading {}", file.display()))?,
                }],
            )?;
            for attachment in attachments {
                println!(
                    "{}/{}  {} bytes  {}",
                    attachment.namespace, attachment.key, attachment.size, attachment.content
                );
            }
        }

        Cmd::Attachments { id } => {
            let store = Store::open(store)?;
            for attachment in store.list_attachments(&id)? {
                println!(
                    "{}/{}  {} bytes  {}{}",
                    attachment.namespace,
                    attachment.key,
                    attachment.size,
                    attachment.content,
                    attachment
                        .media_type
                        .map(|media_type| format!("  {media_type}"))
                        .unwrap_or_default()
                );
            }
        }

        Cmd::Attachment { id, namespace, key } => {
            let store = Store::open(store)?;
            let (_, data) = store
                .read_attachment(&id, &namespace, &key)?
                .with_context(|| format!("no attachment `{namespace}/{key}` on node `{id}`"))?;
            io::stdout().write_all(&data)?;
        }

        Cmd::Stale => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            let stale = graph.stale();
            if stale.is_empty() {
                println!("all nodes up to date");
            }
            for (id, reasons) in stale {
                println!("{id}:");
                for reason in reasons {
                    println!("  {}", render::format_staleness(reason));
                }
            }
        }

        Cmd::Ready { assignee } => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            for id in graph.ready(assignee) {
                println!("{}", render::node_line(&store, &graph, id));
            }
        }

        Cmd::Blocked => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            let blocked = graph.blocked();
            if blocked.is_empty() {
                println!("nothing blocked");
            }
            for (id, blockers) in blocked {
                println!("{id}:");
                for blocker in blockers {
                    println!("  blocked by {}", render::format_blocker(blocker));
                }
            }
        }

        Cmd::Origin { commit } => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            match graph.origin(&commit) {
                Some(id) => println!("{id}"),
                None => println!("no node produced {}", ops::short(&commit)),
            }
        }

        Cmd::Outputs { id } => {
            let store = Store::open(store)?;
            if !store.exists(&id) {
                anyhow::bail!("unknown node `{id}`");
            }
            match store
                .read_result(&id)?
                .and_then(|(result, _)| result.output)
            {
                Some(artifact) => println!("{}", artifact.id),
                None => println!("{id} has produced no output"),
            }
        }

        Cmd::Dependents { id } => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            if !graph.contains(&id) {
                anyhow::bail!("unknown node `{id}`");
            }
            for dependent in graph.dependents(&id) {
                println!("{dependent}");
            }
        }

        Cmd::Check { artifacts } => {
            let (store, vcs) = open_store(store)?;
            let problems = if artifacts {
                check::check_artifacts(&store, &vcs)?
            } else {
                check::check_workbench(&store, &vcs)?
            };
            report(problems, "store is consistent")?;
        }

        Cmd::Settled { id } => {
            let (store, vcs) = open_store(store)?;
            let graph = Graph::load(&store, &vcs)?;
            if !graph.contains(&id) {
                anyhow::bail!("unknown node `{id}`");
            }
            let reasons = graph.settled(&id);
            if reasons.is_empty() {
                println!("{id}: settled");
            } else {
                println!("{id}: not settled");
                for reason in &reasons {
                    println!("  {reason}");
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
            let (store, vcs) = open_store(store)?;
            if verify {
                let (recorded, problems) = check::verify_pairing(&store, &vcs, deep)?;
                match recorded {
                    None => {
                        println!("store is not paired (run `linka pair` to record the project)")
                    }
                    Some(pairing) if problems.is_empty() => {
                        println!("{} — ok", render::pairing_line(&pairing))
                    }
                    Some(_) => report(problems, "")?,
                }
            } else {
                let pairing = ops::pair(&store, &vcs, name, force)?;
                println!("{}", render::pairing_line(&pairing));
            }
        }
    }
    Ok(())
}

/// Freeze the node's inputs and record one conclusion against them. Every
/// result the CLI writes goes through the same protocol the library offers
/// external callers.
fn conclude(
    store: &Store,
    vcs: &GitVcs,
    id: &NodeId,
    conclusion: Conclusion,
    notes: String,
    author: Author,
) -> Result<()> {
    let snapshot = ops::snapshot(store, vcs, id, &[])?;
    ops::submit(
        store,
        vcs,
        Submission {
            snapshot,
            conclusion,
            notes,
            author,
            producer: None,
            attachments: Vec::new(),
        },
    )
    .map_err(anyhow::Error::msg)
}

/// Print problems and exit non-zero, or print `clean` when there are none.
fn report(problems: Vec<String>, clean: &str) -> Result<()> {
    if problems.is_empty() {
        if !clean.is_empty() {
            println!("{clean}");
        }
        return Ok(());
    }
    for problem in &problems {
        println!("{problem}");
    }
    eprintln!("{} problem(s) found", problems.len());
    std::process::exit(1);
}

/// Resolve notes: `--notes` inline, `--notes-file` from a file, or — when
/// neither is given and we are on a terminal — a git-commit-style `$EDITOR`
/// session. Non-interactive callers that pass nothing get empty notes.
fn resolve_notes(
    notes: Option<String>,
    file: Option<PathBuf>,
    store: &Store,
    id: &NodeId,
    ask: &str,
) -> Result<String> {
    use std::io::IsTerminal;
    if let Some(notes) = notes {
        return Ok(notes);
    }
    if let Some(file) = file {
        return std::fs::read_to_string(&file)
            .with_context(|| format!("reading notes from {}", file.display()));
    }
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Ok(String::new());
    }

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
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect();
    kept.join("\n").trim().to_string()
}

fn read_description(description: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match (description, file) {
        (Some(description), _) => Ok(description),
        (None, Some(file)) => std::fs::read_to_string(&file)
            .with_context(|| format!("reading description from {}", file.display())),
        (None, None) => Ok(String::new()),
    }
}

/// Convert CLI path arguments to project-root-relative strings.
fn to_strings(paths: &[ProjectPath]) -> Vec<String> {
    paths.iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::strip_comment_lines;

    #[test]
    fn strip_comment_lines_follows_the_git_template_convention() {
        let text = "\n# Notes for node-1 — title\n# ignored\nDid the work.\n\nMore detail.\n# trailing comment\n";
        assert_eq!(strip_comment_lines(text), "Did the work.\n\nMore detail.");
        assert_eq!(strip_comment_lines("# only comments\n#\n"), "");
        assert_eq!(strip_comment_lines(""), "");
    }
}
