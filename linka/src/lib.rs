//! A git-versioned graph of work nodes.
//!
//! Linka records what work means, how work items relate, and what results were
//! produced. Git provides history, integrity, blame, and distribution; Linka
//! provides the graph semantics git cannot express: what a unit of work is,
//! what it depends on, what evidence covers it, and whether that evidence
//! still holds.
//!
//! * [`model`] — the records, the validated identifiers, and derived state.
//! * [`store`] — the layout, record I/O, blob ids, and the mutation lock.
//! * [`vcs`] — the version-control seam; [`git::GitVcs`] is the real one.
//! * [`graph`] — one memoized evaluation pass and its projections.
//! * [`ops`] — the fact writers, the snapshot/submission protocol, publication.
//! * [`check`] — integrity checking and pairing verification.
//!
//! Modules export named items: the public surface is written down, not implied.

pub mod check;
pub mod git;
pub mod graph;
pub mod model;
pub mod ops;
pub mod pairing;
pub mod store;
pub mod vcs;

pub use check::{check, check_artifacts, check_workbench, verifications_for, verify_pairing};
pub use git::GitVcs;
pub use graph::Graph;
pub use model::{
    component, title_of, ArtifactRef, Attachment, AttachmentKey, Author, Blocker, BlockerReason,
    Candidate, CandidateDecision, CandidateId, Conclusion, ConsumedNode, ContextPin, Currency,
    DefinitionVersion, DepKind, ExternalIdentity, IntegrationStatus, Namespace, Namespaced,
    NewAttachment, NewCandidate, NodeId, NodeMeta, NodeState, ObservedContext, Outcome,
    OutcomeFamily, ProjectPath, ProjectSnapshot, RecordedOutcome, ResultMeta, ResultVersion,
    StalenessReason, Submission, SubmissionConflict, WorkSnapshot, Workability, ATTACHMENT_SCHEMA,
    CANDIDATE_SCHEMA, DEFINITION_SCHEMA, OBSERVATION_SCHEMA, RESULT_SCHEMA, SNAPSHOT_SCHEMA,
};
pub use ops::{NewNode, SubmissionError};
pub use pairing::Pairing;
pub use store::Store;
pub use vcs::{MemoizingVcs, OfflineVcs, Vcs};
