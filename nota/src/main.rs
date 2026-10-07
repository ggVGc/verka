use anyhow::Result;
use clap::{Parser, Subcommand};
use nota::{GitTrailerStore, ReviewEntryKind, ReviewQuery, ReviewStore};
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
    /// List local review branches and their entry counts.
    List {
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        /// Match the exact pinned subject of this Git revision.
        #[arg(long)]
        subject: Option<String>,
        /// Emit reviews and diagnostics as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Start a review branch at a Git revision.
    Start {
        revision: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long)]
        branch: Option<String>,
    },
    /// Append one note to a review branch without touching any checkout.
    Note {
        message: String,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        /// The review branch; defaults to the checked-out branch.
        #[arg(long)]
        branch: Option<String>,
    },
    /// Show a review.
    Show {
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        /// The review branch; defaults to the checked-out branch.
        #[arg(long)]
        branch: Option<String>,
        /// Emit the review and its entries as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let store = GitTrailerStore::default();
    let review_branch = |repository: &PathBuf, branch: Option<String>| match branch {
        Some(branch) => Ok(branch),
        None => store.current_review(repository),
    };
    match cli.command {
        Command::List {
            repository,
            subject,
            json,
        } => {
            let index = store.list_reviews(&repository, &ReviewQuery { subject })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&index)?);
            } else {
                if index.reviews.is_empty() {
                    println!("reviews  none");
                }
                for review in index.reviews {
                    println!(
                        "{}  subject {}  {} notes  {} suggestions",
                        review.branch,
                        short(&review.subject),
                        review.notes,
                        review.suggestions
                    );
                }
                for diagnostic in index.diagnostics {
                    eprintln!("warning: {}: {}", diagnostic.branch, diagnostic.message);
                }
            }
        }
        Command::Start {
            revision,
            repository,
            branch,
        } => {
            let started = store.start_review(&repository, &revision, branch.as_deref())?;
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
            branch,
        } => {
            let branch = review_branch(&repository, branch)?;
            let entry = store.add_note(&repository, &branch, &message)?;
            println!("{}  note", short(&entry.commit));
        }
        Command::Show {
            repository,
            branch,
            json,
        } => {
            let branch = review_branch(&repository, branch)?;
            let review = store.load_review(&repository, &branch)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&review)?);
                return Ok(());
            }
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

fn short(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}
