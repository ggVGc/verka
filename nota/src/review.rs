use crate::git::{checked, checked_with_input, output, repository_root, resolve_commit};
use crate::ReviewProvider;
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

/// Resolve a subject and create its append-only review branch. The empty
/// marker commit is created with plumbing commands, so no checkout is changed.
pub fn start_review(
    provider: &dyn ReviewProvider,
    reference: &str,
    branch: Option<&str>,
) -> Result<StartedReview> {
    let subject = provider.resolve_subject(reference)?;
    let repository = repository_root(&subject.repository)?;
    let subject_revision = resolve_commit(&repository, &subject.revision)?;
    let branch = branch
        .map(str::to_string)
        .unwrap_or_else(|| format!("nota/review-{}", ulid::Ulid::new()));
    checked(&repository, &["check-ref-format", "--branch", &branch])?;
    let refname = format!("refs/heads/{branch}");
    if output(&repository, &["show-ref", "--verify", "--quiet", &refname])?
        .status
        .success()
    {
        bail!("review branch `{branch}` already exists");
    }
    let tree = checked(
        &repository,
        &["rev-parse", &format!("{subject_revision}^{{tree}}")],
    )?;
    let title = subject.title.lines().next().unwrap_or("subject").trim();
    let message = format!(
        "Start review of {title}\n\n{REVIEW_TRAILER} {branch}\n{SUBJECT_TRAILER} {subject_revision}\n"
    );
    let marker = checked_with_input(
        &repository,
        &["commit-tree", &tree, "-p", &subject_revision, "-F", "-"],
        &message,
    )?;
    checked(&repository, &["update-ref", &refname, &marker, ""])?;
    Ok(StartedReview {
        branch,
        marker,
        subject: subject_revision,
        repository,
    })
}

/// Add and commit one Markdown review note without including unrelated staged
/// changes in the commit.
pub fn add_note(repository: &Path, message: &str) -> Result<ReviewEntry> {
    let message = require_message(message)?;
    let repository = repository_root(repository)?;
    let _ = find_marker(&repository, "HEAD")?;
    let relative = format!(".nota/notes/note-{}.md", ulid::Ulid::new());
    let path = repository.join(&relative);
    std::fs::create_dir_all(path.parent().expect("note has parent"))?;
    std::fs::write(&path, format!("{}\n", message.trim_end()))
        .with_context(|| format!("writing {}", path.display()))?;
    checked(&repository, &["add", "--force", "--", &relative])?;
    checked_with_input(
        &repository,
        &["commit", "--only", "-F", "-", "--", &relative],
        message,
    )?;
    entry_at(&repository, "HEAD")
}

pub fn load_review(repository: &Path) -> Result<Review> {
    let repository = repository_root(repository)?;
    let branch = checked(&repository, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .context("Nota commands require a checked-out review branch")?;
    load_review_ref(&repository, &branch)
}

/// Load a review branch without requiring it to be checked out. Coordinators
/// can inspect Nota's Git evidence by ref without changing a user's checkout.
pub fn load_review_ref(repository: &Path, branch: &str) -> Result<Review> {
    let repository = repository_root(repository)?;
    let (marker, recorded_branch, subject) = find_marker(&repository, branch)?;
    if branch != recorded_branch {
        bail!("review branch `{branch}` does not match review marker `{recorded_branch}`");
    }
    let commits = checked(
        &repository,
        &[
            "rev-list",
            "--reverse",
            "--first-parent",
            &format!("{marker}..{branch}"),
        ],
    )?;
    let entries = commits
        .lines()
        .filter(|line| !line.is_empty())
        .map(|commit| entry_at(&repository, commit))
        .collect::<Result<Vec<_>>>()?;
    Ok(Review {
        branch: branch.to_string(),
        marker,
        subject,
        entries,
    })
}

fn find_marker(repository: &Path, revision: &str) -> Result<(String, String, String)> {
    let commits = checked(repository, &["rev-list", "--first-parent", revision])
        .with_context(|| format!("reading review history from `{revision}`"))?;
    for commit in commits.lines() {
        let message = checked(repository, &["show", "-s", "--format=%B", commit])?;
        let branch = trailer(&message, REVIEW_TRAILER);
        let subject = trailer(&message, SUBJECT_TRAILER);
        if let (Some(branch), Some(subject)) = (branch, subject) {
            let actual_parent = checked(repository, &["rev-parse", &format!("{commit}^")])?;
            if actual_parent != subject {
                bail!("review marker `{commit}` has an invalid subject trailer");
            }
            return Ok((commit.to_string(), branch, subject));
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

fn entry_at(repository: &Path, revision: &str) -> Result<ReviewEntry> {
    let commit = resolve_commit(repository, revision)?;
    let message = checked(repository, &["show", "-s", "--format=%B", &commit])?;
    let paths = checked(
        repository,
        &["diff-tree", "--no-commit-id", "--name-only", "-r", &commit],
    )?
    .lines()
    .filter(|line| !line.is_empty())
    .map(str::to_string)
    .collect::<Vec<_>>();
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
        commit,
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
