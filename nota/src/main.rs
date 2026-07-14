use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use nota::{
    add_note, commit_suggestion, load_review, start_review, GitProvider, LinkaProvider,
    ReviewEntryKind,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "nota",
    about = "Record a code review as notes and suggestion commits on a Git branch"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a review branch at an exact provider-supplied subject.
    Start {
        #[command(subcommand)]
        provider: StartProvider,
    },
    /// Add and commit one Markdown review note.
    Note {
        message: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
    },
    /// Commit the currently staged code changes as one suggestion.
    Suggest {
        comment: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
    },
    /// Show the review represented by the current branch.
    Show {
        #[arg(long, default_value = ".")]
        repository: PathBuf,
    },
}

#[derive(Subcommand)]
enum StartProvider {
    /// Resolve an ordinary Git revision.
    Git {
        revision: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        branch: Option<String>,
    },
    /// Resolve the current successful Git output of a Linka node.
    Linka {
        node: String,
        /// Workbench root containing .linka/ and project/. Defaults to the
        /// nearest ancestor containing .linka/.
        #[arg(long)]
        workbench: Option<PathBuf>,
        #[arg(long)]
        branch: Option<String>,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Start { provider } => {
            let started = match provider {
                StartProvider::Git {
                    revision,
                    repository,
                    branch,
                } => start_review(&GitProvider::new(repository), &revision, branch.as_deref())?,
                StartProvider::Linka {
                    node,
                    workbench,
                    branch,
                } => {
                    let workbench = locate_workbench(workbench)?;
                    let store = linka::Store::open(workbench.join(".linka"))?;
                    start_review(&LinkaProvider::new(&store), &node, branch.as_deref())?
                }
            };
            println!("review   {}", started.branch);
            println!("subject  {}", started.subject);
            println!("marker   {}", started.marker);
            println!(
                "worktree git -C {} worktree add <path> {}",
                started.repository.display(),
                started.branch
            );
        }
        Command::Note {
            message,
            repository,
        } => {
            let entry = add_note(&repository, &message)?;
            println!("{}  note", short(&entry.commit));
        }
        Command::Suggest {
            comment,
            repository,
        } => {
            let entry = commit_suggestion(&repository, &comment)?;
            println!("{}  suggestion", short(&entry.commit));
        }
        Command::Show { repository } => {
            let review = load_review(&repository)?;
            println!("review   {}", review.branch);
            println!("subject  {}", review.subject);
            println!("marker   {}", review.marker);
            if review.entries.is_empty() {
                println!("entries  none");
            }
            for entry in review.entries {
                let kind = match entry.kind {
                    ReviewEntryKind::Note => "note",
                    ReviewEntryKind::Suggestion => "suggestion",
                };
                let summary = entry.message.lines().next().unwrap_or_default();
                println!("{}  {kind:<10} {summary}", short(&entry.commit));
                for path in entry.paths {
                    println!("             {path}");
                }
            }
        }
    }
    Ok(())
}

fn locate_workbench(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = given {
        if !root.join(".linka").is_dir() {
            bail!("no .linka store under {}", root.display());
        }
        return Ok(root);
    }
    let mut directory = std::env::current_dir()?;
    loop {
        if directory.join(".linka").is_dir() {
            return Ok(directory);
        }
        if !directory.pop() {
            bail!("no workbench found: no ancestor contains .linka/");
        }
    }
}

fn short(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}
