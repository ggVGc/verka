//! Integrated frontends for the llaundry workspace.
//!
//! Each node keeps structured data in TOML and prose in Markdown. `node.toml`
//! plus `description.md` form the definition; `result.toml` plus optional
//! `result.md` form the completion record. Status, readiness, and staleness are
//! all derived, never stored.
//!
//! New integrations should depend directly on `llaundry-core`,
//! `llaundry-work`, and/or `llaundry-review`. This crate composes the three
//! applications into the original Git workbench and its CLI/MCP/TUI frontends.
//!
//! * `llaundry_work::config` — optional per-store defaults for the work driver.
//! * [`model`] — the on-disk data types, re-exported from the owning crates.
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
    title_of, ArtifactRef, Attempt, AttemptFinished, Author, Candidate, ConsumedNode, ContextPin,
    Decision, DefinitionVersion, DepKind, ExecutionIdentity, NodeMeta, NodeState, Outcome,
    PublicationIntent, ResultMeta, ResultVersion, ReviewDecision, Status, WorkEvidence, WorkedBy,
};
pub use pairing::Pairing;
pub use store::Store;
pub use vcs::Vcs;
