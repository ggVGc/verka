//! Linka-orchestrating agent execution.
//!
//! Orka orchestrates a [`linka::Store`] specifically. It selects Linka-ready
//! work, freezes it as a [`linka::WorkSnapshot`] in a durable attempt, runs an
//! agent command through an [`executor::IsolatedExecutor`] with an explicitly
//! chosen capability grant, and submits a version-checked result through
//! Linka's public API. Linka owns all graph semantics; Orka owns attempts,
//! execution policy, transcripts, recovery, and cleanup.
//!
//! The two genuinely replaceable boundaries stay narrow Orka-owned traits:
//!
//! * [`executor`] — running a command behind a concrete capability grant.
//! * [`workspace`] — preparing and cleaning isolated per-attempt working trees.
//!
//! Everything else — selection, snapshotting, and submission — goes through
//! [`linka_work`], a concrete integration with Linka, not a backend-neutral
//! port. [`fakes`] substitute for the executor and workspace boundaries in
//! tests; the Linka store is always real.

pub mod agent;
pub mod attempt;
pub mod candidate;
pub mod config;
pub mod driva_exec;
pub mod engine;
pub mod events;
pub mod executor;
pub mod fakes;
pub mod input;
pub mod linka_work;
pub mod outcome;
pub mod review;
pub mod review_worktree;
pub mod workspace;
