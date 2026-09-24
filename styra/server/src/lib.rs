//! Styra server: the interactive, isolated agent-session runner, and the
//! interface a client uses to drive it.
//!
//! This crate is two things at once. As an application, its `styra-server`
//! binary owns all mutable and durable state for a session and its live interaction —
//! process launch, agent
//! stdin/stdout, Genta protocol state, journals, update ordering, and
//! stored-session replay — behind a JSON Unix-socket API. As a
//! library, it exposes only what a client needs to speak that API: the wire
//! contract and data vocabulary ([`protocol`]), a blocking [`Client`], and the
//! default socket location ([`paths`]).
//!
//! All coding-agent knowledge — launch profiles, wire protocols, event
//! decoding, the app-server handshake — lives in the `genta` library and is
//! re-exported here under the same module names. Clients consume only Genta's
//! event vocabulary; Driva stays an uninterpreted process transport. See
//! `DESIGN.md`.

// Genta's event vocabulary and rendering cross the wire, so they are part of
// the interface. `agent` and `appserver` are agent-specific knowledge the
// session runner uses; a client touches only `agent::SandboxLayout` to render
// sandbox-relative paths.
pub mod agent {
    // The catalogs — which providers are interactive, which models are worth
    // offering, and which effort rungs each model accepts — are part of the
    // vocabulary a client shares, so they live in `styra_protocol` and are only
    // re-exported here. Only profile resolution, which a client never does, is
    // the session runner's own.
    pub use styra_protocol::agent::*;

    /// Resolve an internal launch profile from the operator's selection.
    pub fn resolve_profile(
        selection: &Selection,
        layout: &SandboxLayout,
    ) -> anyhow::Result<Profile> {
        validate_selection(selection)?;
        selection.resolve(layout)
    }
}
pub use genta::appserver;
pub use genta::claude_stream;
pub use genta::event;
pub use genta::render;

// Driva mount types are embedded in [`types::DrivaOptions`], so a client needs
// them to render the captured policy without depending on Driva directly.
pub use driva::{EnvironmentOrigin, FloorEntry, FloorKind, Mount, MountAccess, WritableMountMode};

// --- The client-facing interface ---
// `contract` is here rather than with the session runner because both sides
// need it: the server frames and parses with it, and a client uses it to name
// a shape and to show an operator the instructions their message was sent with.
pub mod client;
pub mod contract;
pub mod daemon;
// `git` serves both sides too: a client asks it which directories a launch has
// to mount, and the session runner asks it what a checkout is.
pub mod git;
pub mod paths;
pub mod protocol;
pub mod spawn;
// The JSONL framing both peers carry protocol values over. It belongs to the
// transport, not to the vocabulary, so it lives here rather than in
// `styra/protocol`.
pub mod transport;

pub use client::{Client, InProcessServer};
pub use daemon::{in_process, run, serve_if_requested, ServerConfig};
pub use protocol::WorkspaceLaunchChange;
pub use protocol::{
    Answer, AnswerValue, AttributedMount, AttributedVariable, BaseCapability, BaseEntry,
    BranchHistory, Contract, Direction, DrivaOptions, FileLocation, InteractionActivity,
    InteractionActivityReason, InteractionEnd, InteractionSummary, InteractionUpdate, LaunchMount,
    LaunchPolicy, LoadedInteraction, LogEntry, LogLevel, MountOrigin, ProviderRaw, QueuedMessage,
    QuotaEvent, QuotaStatus, RawLine, SessionOrigin, SessionSummary, TemplateSummary,
    VariableOrigin, WorkspaceSummary,
};
pub use spawn::ensure_server;

// --- The session runner ---
// An `interaction` is one live agent process serving a persistent session. Public so
// the `styra-server` binary can drive these; not part of the interface a
// client depends on.
pub mod broker;
// A cheap one-shot question Styra asks an agent for its own purposes, as
// opposed to an `interaction`, which is the operator's own session.
pub mod errand;
pub mod interaction;
pub mod journal;
pub mod naming;
pub mod quota;
pub mod roster;
pub mod server;
pub mod tooling;
pub mod workspace;
pub mod worktree;
