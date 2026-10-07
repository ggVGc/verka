use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedReview {
    pub branch: String,
    pub marker: String,
    pub subject: String,
    pub repository: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Review {
    pub branch: String,
    pub marker: String,
    pub subject: String,
    pub entries: Vec<ReviewEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewEntry {
    pub commit: String,
    /// The note text, or the suggestion's full commit message.
    pub message: String,
    pub kind: ReviewEntryKind,
    /// The project files a suggestion changes; always empty for a note.
    pub paths: Vec<String>,
    /// The lines a note refers to; `None` for a general note and for every
    /// suggestion.
    pub source: Option<NoteSource>,
}

/// Lines of a file as one commit has it. Recording the commit keeps the line
/// numbers meaningful after the file changes: they can be carried to any
/// other version of the file through a diff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NoteSource {
    /// The full id of the commit whose version of `path` the lines number.
    pub revision: String,
    /// Relative to the repository root.
    pub path: String,
    /// The first and last line, counting from 1.
    pub first: usize,
    pub last: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewEntryKind {
    Note,
    Suggestion,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewQuery {
    /// A Git revision to resolve and match against the exact pinned subject.
    pub subject: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewSummary {
    pub branch: String,
    pub marker: String,
    pub subject: String,
    /// The captured branch tip used to read this summary.
    pub tip: String,
    pub notes: usize,
    pub suggestions: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewDiagnostic {
    pub branch: String,
    pub message: String,
}

/// Valid reviews and per-branch failures. Ordinary branches are omitted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReviewIndex {
    pub reviews: Vec<ReviewSummary>,
    pub diagnostics: Vec<ReviewDiagnostic>,
}

/// How Nota records reviews. A review is still a Git branch whose entries are
/// first-parent commits; a store decides how markers and notes are encoded in
/// those commits and validates them on load.
pub trait ReviewStore {
    /// Create an append-only review branch for `revision` in the repository
    /// containing `path`, without changing any checkout. `None` generates a
    /// branch name.
    fn start_review(
        &self,
        path: &Path,
        revision: &str,
        branch: Option<&str>,
    ) -> Result<StartedReview>;

    /// The review branch checked out in the working tree containing `path`.
    fn current_review(&self, path: &Path) -> Result<String>;

    /// Append one prose note to the review on `branch`, about the `source`
    /// lines when given. The branch need not be checked out, and no working
    /// tree or index is touched.
    fn add_note(
        &self,
        path: &Path,
        branch: &str,
        message: &str,
        source: Option<&NoteSource>,
    ) -> Result<ReviewEntry>;

    /// Load and validate the review on `branch` without requiring it to be
    /// checked out.
    fn load_review(&self, path: &Path, branch: &str) -> Result<Review>;

    /// Discover local review branches, ordered by branch name. Invalid reviews
    /// are reported separately so they do not hide valid reviews.
    fn list_reviews(&self, path: &Path, query: &ReviewQuery) -> Result<ReviewIndex>;
}
