//! A Git working tree diffed against a commit or snapshot tree — in Styra,
//! an interaction's checkout against its first snapshot — read, kept, and drawn
//! as a scrolling view.
//!
//! Styra opens the view with `d`; the `styra-diff` program opens it on its
//! own, over any repository. Both drive the same [`CheckoutDiff`] with the
//! same keys ([`keys`]), and draw the view where their layout has room.
//!
//! The view is built on `styra-ui`, so a diff here is styled and highlighted
//! exactly as Styra styles diffs elsewhere.

pub mod files;
pub mod git;
pub mod keys;
pub mod report;
mod state;
mod view;

pub use keys::Outcome;
pub use report::{FileReport, Report, ReportState};
pub use state::{CheckoutDiff, DiffTarget};
pub use view::{render, CheckoutDiffFeedback, CheckoutDiffView, Tint};
