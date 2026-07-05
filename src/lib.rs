//! llaundry — a plain-text node graph for LLM-assisted development, versioned
//! entirely by git.
//!
//! Each node is a directory of two markdown files: `node.md` (the definition;
//! its git blob id is the node's version) and `result.md` (the record of the
//! node's one unit of work: outcome, output commit, and what it was built
//! against). Status, readiness, and staleness are all derived, never stored.
//!
//! This library holds all of the model and graph functionality; the `llaundry`
//! binary is a thin CLI over it. See DESIGN.md for the model and reasoning.
//!
//! * [`model`] — the on-disk data types.
//! * [`store`] — the two-files-per-node store and blob hashing.
//! * [`vcs`] — the version-control seam ([`Vcs`]); [`git::GitVcs`] is the real impl.
//! * [`ops`] — the operations (add, link, edit, complete, fail) and the derived
//!   queries (status, staleness, readiness, blockers, origin).

pub mod git;
pub mod model;
pub mod ops;
pub mod store;
pub mod vcs;

pub use git::GitVcs;
pub use model::{
    Author, BuiltAgainst, ContextPin, DepKind, NodeMeta, Outcome, ResultMeta, Status,
};
pub use store::Store;
pub use vcs::Vcs;
