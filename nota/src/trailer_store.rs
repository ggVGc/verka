//! [`ReviewStore`] that encodes Nota's data in Git commit trailers.
//!
//! The marker and every note are empty commits, so writing them never touches
//! a working tree, and a note is told apart from a suggestion by its
//! `Nota-Note` trailer rather than by the files it changes. A note about
//! particular lines records them in a `Nota-Source` trailer as
//! `<commit>:<path>:<first>-<last>`, numbered as that commit has the file.

use crate::git::{Commit, Git, SystemGit};
use crate::review::{
    NoteSource, Review, ReviewDiagnostic, ReviewEntry, ReviewEntryKind, ReviewIndex, ReviewQuery,
    ReviewStore, ReviewSummary, StartedReview,
};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::Path;

const REVIEW_TRAILER: &str = "Nota-Review";
const SUBJECT_TRAILER: &str = "Nota-Subject";
const NOTE_TRAILER: &str = "Nota-Note";
const SOURCE_TRAILER: &str = "Nota-Source";

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
        let branch = match branch {
            Some(branch) => branch.to_string(),
            None => self.default_branch(&repository, revision, &subject)?,
        };
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

    fn add_note(
        &self,
        path: &Path,
        branch: &str,
        message: &str,
        source: Option<&NoteSource>,
    ) -> Result<ReviewEntry> {
        let text = message.trim();
        if text.is_empty() {
            bail!("review message must not be empty");
        }
        let repository = self.git.repository_root(path)?;
        let review = self.read_branch(&repository, branch)?;
        let mut message = format!("{text}\n\n{NOTE_TRAILER}: true\n");
        if let Some(source) = source {
            self.check_source(&repository, source)?;
            message.push_str(&format!("{SOURCE_TRAILER}: {}\n", format_source(source)));
        }
        let commit = self.git.commit_empty(&repository, &review.tip, &message)?;
        self.git
            .update_branch(&repository, branch, &commit, &review.tip)
            .with_context(|| format!("review branch `{branch}` changed while adding the note"))?;
        let mut entries = self.entries(&repository, &[commit])?;
        Ok(entries.remove(0))
    }

    fn load_review(&self, path: &Path, branch: &str) -> Result<Review> {
        let repository = self.git.repository_root(path)?;
        let review = self.read_branch(&repository, branch)?;
        let entries = self.entries(&repository, &review.entries)?;
        Ok(Review {
            branch: branch.to_string(),
            marker: review.marker.commit,
            subject: review.marker.subject,
            entries,
        })
    }

    fn list_reviews(&self, path: &Path, query: &ReviewQuery) -> Result<ReviewIndex> {
        let repository = self.git.repository_root(path)?;
        let subject = query
            .subject
            .as_deref()
            .map(|revision| self.git.resolve_commit(&repository, revision))
            .transpose()?;
        let mut branches = self.git.local_branches(&repository)?;
        branches.sort_by(|a, b| a.0.cmp(&b.0));
        let tips = branches
            .iter()
            .map(|(_, tip)| tip.clone())
            .collect::<Vec<_>>();
        let markers = self.marker_candidates(&repository, &tips)?;
        let mut index = ReviewIndex::default();
        if markers.is_empty() {
            return Ok(index);
        }
        for (branch, tip) in branches {
            let summary = (|| -> Result<Option<ReviewSummary>> {
                let Some(review) = self.read_branch_at(&repository, &branch, tip, &markers)? else {
                    return Ok(None);
                };
                if subject
                    .as_ref()
                    .is_some_and(|subject| *subject != review.marker.subject)
                {
                    return Ok(None);
                }
                let mut notes = 0;
                let mut suggestions = 0;
                for entry in self.entries(&repository, &review.entries)? {
                    match entry.kind {
                        ReviewEntryKind::Note => notes += 1,
                        ReviewEntryKind::Suggestion => suggestions += 1,
                    }
                }
                Ok(Some(ReviewSummary {
                    branch: branch.clone(),
                    marker: review.marker.commit,
                    subject: review.marker.subject,
                    tip: review.tip,
                    notes,
                    suggestions,
                }))
            })();
            match summary {
                Ok(Some(summary)) => index.reviews.push(summary),
                Ok(None) => {}
                Err(error) => index.diagnostics.push(ReviewDiagnostic {
                    branch,
                    message: format!("{error:#}"),
                }),
            }
        }
        Ok(index)
    }
}

impl GitTrailerStore<'_> {
    /// `nota/review-<source>`, where `<source>` is the local branch `revision`
    /// names (or the checked-out branch for `HEAD`), falling back to the short
    /// subject commit. A numeric suffix keeps the name unused.
    fn default_branch(&self, repository: &Path, revision: &str, subject: &str) -> Result<String> {
        let source = if revision == "HEAD" {
            self.git.current_branch(repository)?
        } else if self.git.branch_exists(repository, revision)? {
            Some(revision.to_string())
        } else {
            None
        };
        let source = source.unwrap_or_else(|| subject[..subject.len().min(12)].to_string());
        let base = format!("nota/review-{source}");
        let mut branch = base.clone();
        let mut n = 2;
        while self.git.branch_exists(repository, &branch)? {
            branch = format!("{base}-{n}");
            n += 1;
        }
        Ok(branch)
    }

    /// Find the review marker nearest the tip of `branch` on its first-parent
    /// history, and check that it names `branch`.
    fn read_branch(&self, repository: &Path, branch: &str) -> Result<Branch> {
        let tip = self.git.branch_tip(repository, branch)?;
        let markers = self.marker_candidates(repository, std::slice::from_ref(&tip))?;
        self.read_branch_at(repository, branch, tip, &markers)?
            .with_context(|| format!("`{branch}` is not a Nota review (no review marker found)"))
    }

    /// The commits on the first-parent histories of `tips` that may be review
    /// markers, read in one pass so that discovery never reads each commit of
    /// a long history separately.
    fn marker_candidates(
        &self,
        repository: &Path,
        tips: &[String],
    ) -> Result<HashMap<String, Commit>> {
        let commits = self.git.first_parent_commits_with_trailers(
            repository,
            tips,
            &[REVIEW_TRAILER, SUBJECT_TRAILER],
        )?;
        Ok(commits
            .into_iter()
            .map(|commit| (commit.id.clone(), commit))
            .collect())
    }

    /// Inspect a captured tip so ref changes during discovery cannot mix
    /// versions. `markers` must include the marker candidates of `tip`'s
    /// history. No marker is distinct from a malformed review.
    fn read_branch_at(
        &self,
        repository: &Path,
        branch: &str,
        tip: String,
        markers: &HashMap<String, Commit>,
    ) -> Result<Option<Branch>> {
        let mut history = self
            .git
            .first_parent_history(repository, &tip)
            .with_context(|| format!("reading review history from `{branch}`"))?;
        for position in 0..history.len() {
            let commit = &history[position];
            let Some(candidate) = markers.get(commit) else {
                continue;
            };
            let (_, trailers) = split_trailers(&candidate.message);
            let review = trailer(&trailers, REVIEW_TRAILER);
            let subject = trailer(&trailers, SUBJECT_TRAILER);
            if review.is_none() && subject.is_none() {
                continue;
            }
            let review = review
                .with_context(|| format!("review marker `{commit}` has no review trailer"))?;
            let subject = subject
                .with_context(|| format!("review marker `{commit}` has no subject trailer"))?;
            if candidate.first_parent.as_deref() != Some(subject) {
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
            return Ok(Some(Branch {
                marker,
                entries: history,
                tip,
            }));
        }
        Ok(None)
    }

    /// Reject a source whose lines are not in its commit's version of the
    /// file, or that its trailer could not record.
    fn check_source(&self, repository: &Path, source: &NoteSource) -> Result<()> {
        let NoteSource {
            revision,
            path,
            first,
            last,
        } = source;
        if self.git.resolve_commit(repository, revision)? != *revision {
            bail!("a note's source must name a full commit id, not `{revision}`");
        }
        if path.is_empty() || path.contains('\n') || path.trim() != path {
            bail!("invalid source path `{path}`");
        }
        let Some(text) = self.git.read_file(repository, revision, path)? else {
            bail!("`{path}` is not in {revision}");
        };
        let length = text.lines().count().max(1);
        if *first == 0 || last < first || *last > length {
            bail!("lines {first}-{last} are not in `{path}`, which has {length} lines");
        }
        Ok(())
    }

    /// Read and validate the entry `commits`, in order.
    fn entries(&self, repository: &Path, commits: &[String]) -> Result<Vec<ReviewEntry>> {
        let read = self.git.commits(repository, commits)?;
        if read.len() != commits.len() {
            bail!(
                "expected {} review entries, read {}",
                commits.len(),
                read.len()
            );
        }
        read.into_iter().map(entry).collect()
    }
}

fn entry(commit: Commit) -> Result<ReviewEntry> {
    let Commit {
        id, message, paths, ..
    } = commit;
    let (text, trailers) = split_trailers(&message);
    if trailer(&trailers, NOTE_TRAILER).is_some() {
        if !paths.is_empty() {
            bail!("note commit `{id}` changes project files");
        }
        if text.trim().is_empty() {
            bail!("note commit `{id}` has no text");
        }
        let source = trailer(&trailers, SOURCE_TRAILER)
            .map(|value| {
                parse_source(value)
                    .with_context(|| format!("note commit `{id}` has an invalid source `{value}`"))
            })
            .transpose()?;
        return Ok(ReviewEntry {
            message: text.to_string(),
            commit: id,
            kind: ReviewEntryKind::Note,
            paths,
            source,
        });
    }
    if message.trim().is_empty() {
        bail!("suggestion commit `{id}` has an empty review comment");
    }
    if paths.is_empty() {
        bail!("suggestion commit `{id}` has no changed project files");
    }
    Ok(ReviewEntry {
        commit: id,
        message,
        kind: ReviewEntryKind::Suggestion,
        paths,
        source: None,
    })
}

fn format_source(source: &NoteSource) -> String {
    let NoteSource {
        revision,
        path,
        first,
        last,
    } = source;
    format!("{revision}:{path}:{first}-{last}")
}

/// Read `<commit>:<path>:<first>-<last>`. The commit has no colon and the
/// lines have none, so the path may.
fn parse_source(value: &str) -> Result<NoteSource> {
    let (revision, rest) = value.split_once(':').context("no commit")?;
    let (path, lines) = rest.rsplit_once(':').context("no lines")?;
    let (first, last) = lines.split_once('-').context("no line range")?;
    let (first, last) = (first.parse()?, last.parse()?);
    let is_commit = matches!(revision.len(), 40 | 64)
        && revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !is_commit || path.is_empty() || first == 0 || last < first {
        bail!("not `<commit>:<path>:<first>-<last>`");
    }
    Ok(NoteSource {
        revision: revision.to_string(),
        path: path.to_string(),
        first,
        last,
    })
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
    use super::{format_source, parse_source, split_trailers};
    use crate::review::NoteSource;

    #[test]
    fn sources_round_trip_through_their_trailer() {
        let source = NoteSource {
            revision: "a".repeat(40),
            path: "dir/odd:name.rs".into(),
            first: 3,
            last: 7,
        };
        assert_eq!(parse_source(&format_source(&source)).unwrap(), source);
        for invalid in ["abc:file:1-2", &format!("{}:file:2-1", "a".repeat(40))] {
            assert!(parse_source(invalid).is_err(), "{invalid}");
        }
    }

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
