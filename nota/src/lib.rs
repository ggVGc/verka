//! Git-native review branches.

mod git;
mod locate;
mod review;
mod trailer_store;

pub use git::{Commit, FileDiff, Git, Hunk, SystemGit};
pub use locate::{note_source, place, Location, PlacedEntry, PlacedReview, Status, Target};
pub use review::{
    NoteSource, Review, ReviewDiagnostic, ReviewEntry, ReviewEntryKind, ReviewIndex, ReviewQuery,
    ReviewStore, ReviewSummary, StartedReview,
};
pub use trailer_store::GitTrailerStore;
