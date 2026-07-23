//! Genta: knowledge of coding agents (codex, Claude Code) as processes.
//!
//! The library owns everything agent-specific: how an agent is launched
//! ([`agent`] profiles carrying command lines, mounts, and environment), the
//! wire protocols it speaks and their decoding into a stable event vocabulary
//! ([`event`]), and the stateful `codex app-server` handshake ([`appserver`]).
//!
//! Genta is transport-agnostic: it never spawns processes or owns pipes.
//! Hosts (Styra, Orka) launch the process through their own executor and feed
//! lines through the decoders here.

pub mod agent;
pub mod appserver;
pub mod event;
