//! Styra server: the interactive, isolated agent-session runner, and the
//! interface a client uses to drive it.
//!
//! This crate is two things at once. As an application, its `styra-server`
//! binary owns all mutable and durable session state — process launch, agent
//! stdin/stdout, Genta protocol state, journals, update ordering, and
//! stored-session replay — behind a versioned JSON Unix-socket API. As a
//! library, it exposes only what a client needs to speak that API: the wire
//! contract ([`api`]), a blocking [`Client`], the data vocabulary that crosses
//! the socket ([`types`]), and the default socket location ([`paths`]).
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
pub use genta::agent;
pub use genta::appserver;
pub use genta::event;
pub use genta::render;

// Driva mount types are embedded in [`types::DrivaOptions`], so a client needs
// them to render the captured policy without depending on Driva directly.
pub use driva::{Mount, MountAccess};

// --- The client-facing interface ---
pub mod api;
pub mod client;
pub mod paths;
pub mod types;

pub use client::Client;
pub use types::{
    Direction, DrivaOptions, LogEntry, LogLevel, RawLine, SessionEnd, SessionSummary, SessionUpdate,
};

// --- The session runner ---
// Public so the `styra-server` binary can drive them; not part of the
// interface a client depends on.
pub mod journal;
pub mod server;
pub mod session;
