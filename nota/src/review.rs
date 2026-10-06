use crate::git::Git;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const REVIEW_TRAILER: &str = "Nota-Review:";
const SUBJECT_TRAILER: &str = "Nota-Subject:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedReview {
    pub branch: String,
    pub marker: String,
    pub subject: String,
    pub repository: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub branch: String,
    pub marker: String,
    pub subject: String,
    pub entries: Vec<ReviewEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewEntry {
    pub commit: String,
    pub message: String,
    pub kind: ReviewEntryKind,
    pub paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewEntryKind {
    Note,
    Suggestion,
}

/// The empty commit that starts a review, as recorded in its trailers.
struct Marker {
    commit: String,
    branch: String,
    subject: String,
}

/// Create an append-only review branch for `revision` in the repository
/// containing `path`. The empty marker commit is created without changing any
/// checkout.
pub fn start_review(
    git: &dyn Git,
    path: &Path,
    revision: &str,
    branch: Option<&str>,
) -> Result<StartedReview> {
    let repository = git.repository_root(path)?;
    let subject_revision = git.resolve_commit(&repository, revision)?;
    let branch = branch
        .map(str::to_string)
        .unwrap_or_else(|| format!("nota/review-{}", ulid::Ulid::new()));
    git.validate_branch_name(&repository, &branch)?;
    if git.branch_exists(&repository, &branch)? {
        bail!("review branch `{branch}` already exists");
    }
    let message = format!(
        "Start review of {revision}\n\n{REVIEW_TRAILER} {branch}\n{SUBJECT_TRAILER} {subject_revision}\n"
    );
    let marker = git.commit_empty(&repository, &subject_revision, &message)?;
    git.create_branch(&repository, &branch, &marker)?;
    Ok(StartedReview {
        branch,
        marker,
        subject: subject_revision,
        repository,
    })
}

/// Add and commit one Markdown review note without including unrelated staged
/// changes in the commit.
pub fn add_note(git: &dyn Git, repository: &Path, message: &str) -> Result<ReviewEntry> {
    let message = require_message(message)?;
    let repository = git.repository_root(repository)?;
    read_review(git, &repository, "HEAD")?;
    let path = format!(".nota/notes/note-{}.md", ulid::Ulid::new());
    let contents = format!("{}\n", message.trim_end());
    let commit = git.commit_file(&repository, &path, &contents, message)?;
    entry_at(git, &repository, &commit)
}

pub fn load_review(git: &dyn Git, repository: &Path) -> Result<Review> {
    let repository = git.repository_root(repository)?;
    let branch = git
        .current_branch(&repository)?
        .context("Nota commands require a checked-out review branch")?;
    load_review_ref(git, &repository, &branch)
}

/// Load a review branch without requiring it to be checked out. Coordinators
/// can inspect Nota's Git evidence by ref without changing a user's checkout.
pub fn load_review_ref(git: &dyn Git, repository: &Path, branch: &str) -> Result<Review> {
    let repository = git.repository_root(repository)?;
    let (marker, commits) = read_review(git, &repository, branch)?;
    if branch != marker.branch {
        bail!(
            "review branch `{branch}` does not match review marker `{}`",
            marker.branch
        );
    }
    let entries = commits
        .iter()
        .map(|commit| entry_at(git, &repository, commit))
        .collect::<Result<Vec<_>>>()?;
    Ok(Review {
        branch: branch.to_string(),
        marker: marker.commit,
        subject: marker.subject,
        entries,
    })
}

/// Find the review marker nearest `revision` on its first-parent history, and
/// the entry commits that follow it, oldest first.
fn read_review(git: &dyn Git, repository: &Path, revision: &str) -> Result<(Marker, Vec<String>)> {
    let mut history = git
        .first_parent_history(repository, revision)
        .with_context(|| format!("reading review history from `{revision}`"))?;
    for position in 0..history.len() {
        let commit = &history[position];
        let message = git.commit_message(repository, commit)?;
        let branch = trailer(&message, REVIEW_TRAILER);
        let subject = trailer(&message, SUBJECT_TRAILER);
        if let (Some(branch), Some(subject)) = (branch, subject) {
            if git.first_parent(repository, commit)?.as_deref() != Some(subject.as_str()) {
                bail!("review marker `{commit}` has an invalid subject trailer");
            }
            let marker = Marker {
                commit: commit.clone(),
                branch,
                subject,
            };
            history.truncate(position);
            history.reverse();
            return Ok((marker, history));
        }
    }
    bail!("current branch is not a Nota review (no review marker found)")
}

fn trailer(message: &str, name: &str) -> Option<String> {
    message
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(name).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn entry_at(git: &dyn Git, repository: &Path, commit: &str) -> Result<ReviewEntry> {
    let message = git.commit_message(repository, commit)?;
    let paths = git.changed_paths(repository, commit)?;
    let kind = if !paths.is_empty() && paths.iter().all(|path| path.starts_with(".nota/notes/")) {
        ReviewEntryKind::Note
    } else {
        ReviewEntryKind::Suggestion
    };
    if kind == ReviewEntryKind::Suggestion {
        if message.trim().is_empty() {
            bail!("suggestion commit `{commit}` has an empty review comment");
        }
        if paths.is_empty() {
            bail!("suggestion commit `{commit}` has no changed project files");
        }
        if paths
            .iter()
            .any(|path| path == ".nota" || path.starts_with(".nota/"))
        {
            bail!("suggestion commit `{commit}` may not contain Nota files");
        }
    }
    Ok(ReviewEntry {
        commit: commit.to_string(),
        message,
        kind,
        paths,
    })
}

fn require_message(message: &str) -> Result<&str> {
    if message.trim().is_empty() {
        bail!("review message must not be empty");
    }
    Ok(message)
}
