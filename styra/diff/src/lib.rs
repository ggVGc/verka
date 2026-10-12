//! A Git working tree diffed against a commit — in Styra, an interaction's
//! checkout against the commit its branch was made at — read, kept, and drawn
//! as a scrolling view.
//!
//! Styra opens the view with `d`; the `styra-diff` program opens it on its
//! own, over any repository. Both drive the same [`CheckoutDiff`]: the host
//! decides which keys ask for which [`Action`], and draws the view where its
//! layout has room.
//!
//! The view is built on `styra-ui`, so a diff here is styled and highlighted
//! exactly as Styra styles diffs elsewhere.

pub mod files;
pub mod git;
mod state;
mod view;

pub use state::{Action, CheckoutDiff, DiffTarget};
pub use view::{render, CheckoutDiffFeedback, CheckoutDiffView, Tint};
