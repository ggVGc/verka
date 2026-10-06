//! The Linka domain model: the on-disk data types.
//!
//! This crate holds only the types; reading, writing, and operating on a store
//! of them lives in the `linka` crate, which re-exports this one as
//! `linka::model`.
//!
//! A node separates structured data from prose: `node.toml` and
//! `description.md` form its definition, while `result.toml` and the optional
//! `result.md` form its completion record.
//!
//! Status is never stored. It is derived from whether `result.toml` exists,
//! what its `outcome` says, and whether its definition version still matches.
//!
//! * [`ids`] — validated node ids, candidate ids, and project paths.
//! * [`node`] — node definitions and attachments.
//! * [`result`] — completion records, outcomes, and consumed pins.
//! * [`state`] — derived state: currency, staleness, blockers, integration.
//! * [`submission`] — work snapshots and checked submissions.

pub mod ids;
pub mod node;
pub mod result;
pub mod state;
pub mod submission;

pub use ids::{CandidateId, NodeId, ProjectPath};
pub use node::{
    title_of, Author, DefinitionVersion, DepKind, NewNodeAttachment, NodeAttachment, NodeMeta,
    ATTACHMENT_SCHEMA, DEFINITION_SCHEMA,
};
pub use result::{
    ArtifactRef, ConsumedNode, ContextObservation, ContextPin, Outcome, ProducerEvidence,
    ProjectSnapshot, ResultMeta, ResultOutcome, ResultVersion, VerificationOutcome,
    OBSERVATION_SCHEMA, RESULT_SCHEMA,
};
pub use state::{
    Blocker, BlockerReason, Currency, IntegrationStatus, NodeState, RecordedOutcome,
    StalenessReason, StateClass,
};
pub use submission::{
    ResultSubmission, SubmissionConflict, SubmissionEnvelope, VerificationSubmission, WorkSnapshot,
    SNAPSHOT_SCHEMA,
};
