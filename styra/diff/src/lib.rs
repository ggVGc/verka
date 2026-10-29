//! A Git working tree diffed against a commit or snapshot tree — in Styra,
//! an interaction's checkout against where its branch will be integrated —
//! read, kept, and drawn as a scrolling view.
//!
//! Styra opens the view with `d`; the `styra-diff` program opens it on its
//! own, over any repository. Both drive the same [`CheckoutDiff`] with the
//! same keys ([`keys`]), and draw the view where their layout has room.
//!
//! The view is built on `styra-ui`, so a diff here is styled and highlighted
//! exactly as Styra styles diffs elsewhere.

pub mod files;
mod find;
pub mod git;
mod highlight;
pub mod keys;
pub mod layers;
mod prompt;
pub mod report;
mod split;
pub use prompt::Prompt;
mod state;
mod view;

#[cfg(test)]
mod test_support;

pub use keys::Outcome;
pub use report::{FileReport, Report, ReportPage, ReportState};
pub use state::{CheckoutDiff, Commits, DiffTarget, Source, Supplied};
pub use view::{render, CheckoutDiffFeedback, CheckoutDiffView, Tint};
