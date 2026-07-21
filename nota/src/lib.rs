//! Git-native review branches.

mod git;
pub mod providers;
mod review;

use anyhow::Result;
use std::path::PathBuf;

pub use providers::GitProvider;
pub use review::{
    add_note, load_review, load_review_ref, start_review, Review, ReviewEntry, ReviewEntryKind,
    StartedReview,
};

/// Resolves an application-specific reference to exact Git content.
pub trait ReviewProvider {
    fn resolve_subject(&self, reference: &str) -> Result<ReviewSubject>;
}

/// The exact Git subject from which Nota can start a review branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewSubject {
    pub repository: PathBuf,
    pub revision: String,
    pub title: String,
}
