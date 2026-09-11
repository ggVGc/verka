//! The stable JSON protocol spoken with `styra-server`.
//!
//! The crate owns the data vocabulary and the typed-answer helpers: the types
//! every request and response serializes to, and nothing about how those bytes
//! travel. Carrying them — the JSONL framing, the socket, the connection — is
//! each peer's own business, so a client can be written against this crate
//! without inheriting a transport it did not choose.
//!
//! A client that cannot be written in Rust gets the same vocabulary generated
//! for it: [`codegen`] reads these type definitions and writes a module in
//! another language with every operation, field, and enum spelling in it, so
//! the bindings are read out of the protocol rather than transcribed beside
//! it. Lua is the language it generates today; the reading and the writing are
//! separated so a second one is a backend rather than a rewrite.

pub mod agent {
    pub use genta::agent::*;

    pub const PROVIDERS: [Provider; 2] = [Provider::Codex, Provider::Claude];

    /// Validate an interactive Styra launch selection.
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
}

pub use driva::{Mount, MountAccess};
pub use genta::{event, render};

#[cfg(feature = "codegen")]
pub mod codegen;
pub mod contract;
pub mod protocol;

pub use protocol::*;
