//! A git-versioned graph of LLM-development nodes.
//!
//! Each node keeps structured data in TOML and prose in Markdown. `node.toml`
//! plus `description.md` form the definition; `result.toml` plus optional
//! `result.md` form the completion record. Status, readiness, and staleness are
//! all derived, never stored.
//!
//! * [`model`] — the on-disk data types and derived status.
//! * [`store`] — the two-files-per-node store and blob hashing.
//! * [`vcs`] — the version-control seam ([`Vcs`]); [`git::GitVcs`] is the real impl.
//! * [`ops`] — the operations (add, link, edit, complete, fail) and the derived
//!   queries (status, staleness, readiness, blockers, origin).

pub mod candidate;
pub mod git;
pub mod model;
pub mod ops;
pub mod pairing;
pub mod store;
pub mod vcs;

pub use candidate::{
    CandidateRecord, CandidateState, CandidateStore, ExternalIdentity, NewCandidate,
};
pub use git::GitVcs;
pub use model::{
    title_of, ArtifactRef, Author, Blocker, BlockerReason, CandidateId, ConsumedNode,
    ContextObservation, ContextPin, Currency, DefinitionVersion, DepKind, IntegrationStatus,
    NodeId, NodeMeta, NodeState, Outcome, ProducerEvidence, ProjectPath, ProjectSnapshot,
    RecordedOutcome, ResultMeta, ResultSubmission, ResultVersion, StalenessReason, Status,
    SubmissionConflict, WorkSnapshot, DEFINITION_SCHEMA, OBSERVATION_SCHEMA, RESULT_SCHEMA,
    SNAPSHOT_SCHEMA,
};
pub use pairing::Pairing;
pub use store::Store;
pub use vcs::{ArtifactStore, BranchStore, ContextIdentity, RepositoryIdentity, StoreHistory, Vcs};
