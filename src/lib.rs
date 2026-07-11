//! llaundry — a plain-text node graph for LLM-assisted development, versioned
//! entirely by git.
//!
//! Each node keeps structured data in TOML and prose in Markdown. `node.toml`
//! plus `description.md` form the definition; `result.toml` plus optional
//! `result.md` form the completion record. Status, readiness, and staleness are
//! all derived, never stored.
//!
//! This library holds all of the model and graph functionality; the `llaundry`
//! binary is a thin CLI over it. See DESIGN.md for the model and reasoning.
//!
//! * [`config`] — optional per-store defaults for the work driver.
//! * [`model`] — the on-disk data types.
//! * [`store`] — the two-files-per-node store and blob hashing.
//! * [`vcs`] — the version-control seam ([`Vcs`]); [`git::GitVcs`] is the real impl.
//! * [`ops`] — the operations (add, link, edit, complete, fail) and the derived
//!   queries (status, staleness, readiness, blockers, origin).

pub mod git;
pub mod model;
pub mod ops;
pub mod pairing;
pub mod store;
pub mod vcs;

pub use git::GitVcs;
pub use llaundry_work::{Config, CONFIG_FILE};
pub use model::{
    title_of, AttemptFinal, AttemptMeta, Author, BuiltAgainst, ContextPin, DefinitionVersion,
    DepKind, ExecutionIdentity, NodeMeta, NodeState, Outcome, PublicationIntent, ResultMeta,
    ResultVersion, ReviewDecision, ReviewTarget, Status, WorkedBy,
};
pub use pairing::Pairing;
pub use store::Store;
pub use vcs::Vcs;
