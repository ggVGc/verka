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

pub mod agent;

pub use driva::{EnvironmentOrigin, FloorEntry, FloorKind, Mount, MountAccess, WritableMountMode};
pub use genta::{event, render};

#[cfg(feature = "codegen")]
pub mod codegen;
pub mod contract;
pub mod protocol;

pub use protocol::*;
