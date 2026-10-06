use anyhow::Result;
use std::path::{Path, PathBuf};

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
    /// The note text, or the suggestion's full commit message.
    pub message: String,
    pub kind: ReviewEntryKind,
    /// The project files a suggestion changes; always empty for a note.
    pub paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewEntryKind {
    Note,
    Suggestion,
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

    /// Append one prose note to the review on `branch`. The branch need not be
    /// checked out, and no working tree or index is touched.
    fn add_note(&self, path: &Path, branch: &str, message: &str) -> Result<ReviewEntry>;

    /// Load and validate the review on `branch` without requiring it to be
    /// checked out.
    fn load_review(&self, path: &Path, branch: &str) -> Result<Review>;
}
