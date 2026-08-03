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
    pub use genta::agent::*;

    /// The interactive providers Styra can launch, in picker order.
    pub const PROVIDERS: [Provider; 2] = [Provider::Codex, Provider::Claude];

    /// Validate a Styra launch selection, excluding Genta's batch-only providers.
    pub fn validate_selection(selection: &Selection) -> anyhow::Result<()> {
        if !PROVIDERS.contains(&selection.provider) {
            anyhow::bail!(
                "agent provider {:?} is not interactive; Styra supports: {}",
                selection.provider.as_str(),
                PROVIDERS.map(|provider| provider.as_str()).join(", ")
            );
        }
        if selection.model.trim().is_empty() {
            anyhow::bail!("the agent model cannot be empty");
        }
        if !selection.provider.efforts().contains(&selection.effort) {
            anyhow::bail!(
                "reasoning effort {:?} is not supported by {}",
                selection.effort.as_str(),
                selection.provider.as_str()
            );
        }
        Ok(())
    }

    /// Resolve an internal launch profile from the operator's selection.
    pub fn resolve_profile(
        selection: &Selection,
        layout: &SandboxLayout,
    ) -> anyhow::Result<Profile> {
        validate_selection(selection)?;
        selection.resolve(layout)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn styra_only_accepts_interactive_providers() {
            validate_selection(&Selection::new(Provider::Codex)).unwrap();
            validate_selection(&Selection::new(Provider::Claude)).unwrap();
            let error = validate_selection(&Selection::new(Provider::CodexExec)).unwrap_err();
            assert!(error.to_string().contains("not interactive"));
        }
    }
}
pub use genta::appserver;
pub use genta::event;
pub use genta::render;

// Driva mount types are embedded in [`types::DrivaOptions`], so a client needs
// them to render the captured policy without depending on Driva directly.
pub use driva::{Mount, MountAccess};

// --- The client-facing interface ---
pub mod client;
pub mod daemon;
pub mod paths;
pub mod protocol;
pub mod spawn;

pub use client::Client;
pub use daemon::{run, serve_if_requested, ServerConfig};
pub use protocol::{
    Direction, DrivaOptions, InteractionActivity, InteractionEnd, InteractionSummary,
    InteractionUpdate, LogEntry, LogLevel, RawLine, SessionSummary, WorkspaceSummary,
};
pub use spawn::ensure_server;

// --- The session runner ---
// An `interaction` is one live agent process serving a persistent session. Public so
// the `styra-server` binary can drive these; not part of the interface a
// client depends on.
pub mod broker;
pub mod interaction;
pub mod journal;
pub mod server;
pub mod workspace;
