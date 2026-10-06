//! [`ReviewStore`] that encodes Nota's data in Git commit trailers.
//!
//! The marker and every note are empty commits, so writing them never touches
//! a working tree, and a note is told apart from a suggestion by its
//! `Nota-Note` trailer rather than by the files it changes.

use crate::git::{Git, SystemGit};
use crate::review::{Review, ReviewEntry, ReviewEntryKind, ReviewStore, StartedReview};
use anyhow::{bail, Context, Result};
use std::path::Path;

const REVIEW_TRAILER: &str = "Nota-Review";
const SUBJECT_TRAILER: &str = "Nota-Subject";
const NOTE_TRAILER: &str = "Nota-Note";

/// [`ReviewStore`] whose markers and notes are empty commits carrying
/// trailers, and whose suggestions are ordinary project commits.
#[derive(Clone, Copy)]
pub struct GitTrailerStore<'a> {
    git: &'a dyn Git,
}

impl<'a> GitTrailerStore<'a> {
    pub fn new(git: &'a dyn Git) -> Self {
        Self { git }
    }
}

impl Default for GitTrailerStore<'static> {
    fn default() -> Self {
        Self::new(&SystemGit)
    }
}

/// The empty commit that starts a review, as recorded in its trailers.
struct Marker {
    commit: String,
    subject: String,
}

/// A review branch as read from Git, before its entries are validated.
struct Branch {
    marker: Marker,
    /// Entry commits after the marker, oldest first.
    entries: Vec<String>,
    tip: String,
}

impl ReviewStore for GitTrailerStore<'_> {
    fn start_review(
        &self,
        path: &Path,
        revision: &str,
        branch: Option<&str>,
    ) -> Result<StartedReview> {
        let repository = self.git.repository_root(path)?;
        let subject = self.git.resolve_commit(&repository, revision)?;
        let branch = branch
            .map(str::to_string)
            .unwrap_or_else(|| format!("nota/review-{}", ulid::Ulid::new()));
        self.git.validate_branch_name(&repository, &branch)?;
        if self.git.branch_exists(&repository, &branch)? {
            bail!("review branch `{branch}` already exists");
        }
        let message = format!(
            "Start review of {revision}\n\n{REVIEW_TRAILER}: {branch}\n{SUBJECT_TRAILER}: {subject}\n"
        );
        let marker = self.git.commit_empty(&repository, &subject, &message)?;
        self.git.create_branch(&repository, &branch, &marker)?;
        Ok(StartedReview {
            branch,
            marker,
            subject,
            repository,
        })
    }

    fn current_review(&self, path: &Path) -> Result<String> {
        let repository = self.git.repository_root(path)?;
        self.git
            .current_branch(&repository)?
            .context("no review branch is checked out")
    }

    fn add_note(&self, path: &Path, branch: &str, message: &str) -> Result<ReviewEntry> {
        let text = message.trim();
        if text.is_empty() {
            bail!("review message must not be empty");
        }
        let repository = self.git.repository_root(path)?;
        let review = self.read_branch(&repository, branch)?;
        let message = format!("{text}\n\n{NOTE_TRAILER}: true\n");
        let commit = self.git.commit_empty(&repository, &review.tip, &message)?;
        self.git
            .update_branch(&repository, branch, &commit, &review.tip)
            .with_context(|| format!("review branch `{branch}` changed while adding the note"))?;
        self.entry_at(&repository, &commit)
    }

    fn load_review(&self, path: &Path, branch: &str) -> Result<Review> {
        let repository = self.git.repository_root(path)?;
        let review = self.read_branch(&repository, branch)?;
        let entries = review
            .entries
            .iter()
            .map(|commit| self.entry_at(&repository, commit))
            .collect::<Result<Vec<_>>>()?;
        Ok(Review {
            branch: branch.to_string(),
            marker: review.marker.commit,
            subject: review.marker.subject,
            entries,
        })
    }
}

impl GitTrailerStore<'_> {
    /// Find the review marker nearest the tip of `branch` on its first-parent
    /// history, and check that it names `branch`.
    fn read_branch(&self, repository: &Path, branch: &str) -> Result<Branch> {
        let mut history = self
            .git
            .first_parent_history(repository, branch)
            .with_context(|| format!("reading review history from `{branch}`"))?;
        let tip = history.first().cloned().context("empty history")?;
        for position in 0..history.len() {
            let commit = &history[position];
            let message = self.git.commit_message(repository, commit)?;
            let (_, trailers) = split_trailers(&message);
            let (Some(review), Some(subject)) = (
                trailer(&trailers, REVIEW_TRAILER),
                trailer(&trailers, SUBJECT_TRAILER),
            ) else {
                continue;
            };
            if self.git.first_parent(repository, commit)?.as_deref() != Some(subject) {
                bail!("review marker `{commit}` has an invalid subject trailer");
            }
            if review != branch {
                bail!("review branch `{branch}` does not match review marker `{review}`");
            }
            let marker = Marker {
                commit: commit.clone(),
                subject: subject.to_string(),
            };
            history.truncate(position);
            history.reverse();
            return Ok(Branch {
                marker,
                entries: history,
                tip,
            });
        }
        bail!("`{branch}` is not a Nota review (no review marker found)")
    }

    fn entry_at(&self, repository: &Path, commit: &str) -> Result<ReviewEntry> {
        let message = self.git.commit_message(repository, commit)?;
        let paths = self.git.changed_paths(repository, commit)?;
        let (text, trailers) = split_trailers(&message);
        if trailer(&trailers, NOTE_TRAILER).is_some() {
            if !paths.is_empty() {
                bail!("note commit `{commit}` changes project files");
            }
            if text.trim().is_empty() {
                bail!("note commit `{commit}` has no text");
            }
            return Ok(ReviewEntry {
                commit: commit.to_string(),
                message: text.to_string(),
                kind: ReviewEntryKind::Note,
                paths,
            });
        }
        if message.trim().is_empty() {
            bail!("suggestion commit `{commit}` has an empty review comment");
        }
        if paths.is_empty() {
            bail!("suggestion commit `{commit}` has no changed project files");
        }
        Ok(ReviewEntry {
            commit: commit.to_string(),
            message,
            kind: ReviewEntryKind::Suggestion,
            paths,
        })
    }
}

/// Split a commit message into its text and the `Key: value` trailers of its
/// final paragraph. As in Git, a message's first paragraph is never trailers.
fn split_trailers(message: &str) -> (&str, Vec<(&str, &str)>) {
    let message = message.trim_end();
    let Some(split) = message.rfind("\n\n") else {
        return (message, Vec::new());
    };
    let trailers = message[split + 2..]
        .lines()
        .map(trailer_line)
        .collect::<Option<Vec<_>>>();
    match trailers {
        Some(trailers) if !trailers.is_empty() => (message[..split].trim_end(), trailers),
        _ => (message, Vec::new()),
    }
}

fn trailer_line(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    let is_token = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    is_token.then(|| (key, value.trim()))
}

/// The last non-empty value of trailer `key`.
fn trailer<'m>(trailers: &[(&str, &'m str)], key: &str) -> Option<&'m str> {
    trailers
        .iter()
        .rev()
        .find(|(name, value)| *name == key && !value.is_empty())
        .map(|(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::split_trailers;

    #[test]
    fn only_a_final_paragraph_of_trailers_is_split_off() {
        assert_eq!(
            split_trailers("Text\n\nMore: prose\n\nNota-Note: true\n"),
            ("Text\n\nMore: prose", vec![("Nota-Note", "true")])
        );
        assert_eq!(
            split_trailers("Nota-Note: true"),
            ("Nota-Note: true", vec![])
        );
        assert_eq!(
            split_trailers("Text\n\nnot a trailer\nNota-Note: true"),
            ("Text\n\nnot a trailer\nNota-Note: true", vec![])
        );
    }
}
