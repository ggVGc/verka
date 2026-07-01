//! llaundry — a content-addressed, immutable node graph for LLM-assisted
//! development, backed by plain text in git.
//!
//! This library holds all of the database and graph functionality; the `llaundry`
//! binary is a thin CLI over it. See DESIGN.md for the model and reasoning.
//!
//! * [`model`] — the on-disk data types.
//! * [`store`] — the content-addressed object store, refs, and status logs.
//! * [`vcs`] — the version-control seam ([`Vcs`]); [`git::GitVcs`] is the real impl.
//! * [`ops`] — the graph operations (add, link, edit, complete, status) and the
//!   derived queries (staleness, readiness, blockers).

pub mod git;
pub mod model;
pub mod ops;
pub mod store;
pub mod vcs;

pub use git::GitVcs;
pub use model::{Author, Edge, Meta, NodeType, Pin, Status, StatusEvent, StatusLog};
pub use store::Store;
pub use vcs::Vcs;
