use anyhow::Context;
use anyhow::Result;
use clap::{Parser, Subcommand};
use nota::{GitTrailerStore, ReviewEntryKind, ReviewQuery, ReviewStore, SystemGit, Target};
use std::io::Read;
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
        /// The file the note is about, relative to the repository root.
        #[arg(long, requires = "lines")]
        path: Option<String>,
        /// The lines of `--path` the note is about: `<first>` or
        /// `<first>-<last>`, numbered as in `--contents`.
        #[arg(long, requires = "path", value_parser = parse_lines)]
        lines: Option<(usize, usize)>,
        /// The commit to record the lines against; they are carried over to
        /// it from `--contents`.
        #[arg(long, requires = "path", default_value = "HEAD")]
        revision: String,
        /// A file holding the text the lines number, such as an editor's
        /// unsaved buffer, or `-` for stdin; defaults to the working tree
        /// file.
        #[arg(long, requires = "path")]
        contents: Option<PathBuf>,
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
        /// Also place each note and suggestion hunk in `worktree`, the files
        /// in the working tree, or in a revision.
        #[arg(long, requires = "json", value_name = "worktree|REVISION")]
        at: Option<String>,
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
            path,
            lines,
            revision,
            contents,
        } => {
            let branch = review_branch(&repository, branch)?;
            let source = match (path, lines) {
                (Some(path), Some(lines)) => {
                    let contents = contents.map(read_contents).transpose()?;
                    Some(nota::note_source(
                        &SystemGit,
                        &repository,
                        &revision,
                        &path,
                        lines,
                        contents.as_deref(),
                    )?)
                }
                _ => None,
            };
            let entry = store.add_note(&repository, &branch, &message, source.as_ref())?;
            println!("{}  note", short(&entry.commit));
        }
        Command::Show {
            repository,
            branch,
            json,
            at,
        } => {
            let branch = review_branch(&repository, branch)?;
            let review = store.load_review(&repository, &branch)?;
            if let Some(at) = at {
                let target = match at.as_str() {
                    "worktree" => Target::Worktree,
                    _ => Target::Revision(at),
                };
                let placed = nota::place(&SystemGit, &repository, review, &target)?;
                println!("{}", serde_json::to_string_pretty(&placed)?);
                return Ok(());
            }
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

fn parse_lines(value: &str) -> Result<(usize, usize), String> {
    let (first, last) = value.split_once('-').unwrap_or((value, value));
    match (first.parse(), last.parse()) {
        (Ok(first), Ok(last)) if first > 0 && last >= first => Ok((first, last)),
        _ => Err(format!("`{value}` is not `<first>` or `<first>-<last>`")),
    }
}

fn read_contents(path: PathBuf) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut contents = String::new();
        std::io::stdin().read_to_string(&mut contents)?;
        return Ok(contents);
    }
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn short(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}
