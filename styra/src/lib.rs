//! Styra: an interactive, isolated agent session runner.
//!
//! All coding-agent knowledge — launch profiles, wire protocols, event
//! decoding, the app-server handshake — lives in the `genta` library and is
//! re-exported here under the same module names. The rest of the application
//! consumes only Genta's event vocabulary, and Driva stays an uninterpreted
//! process transport. See `DESIGN.md`.

pub use genta::agent;
pub use genta::appserver;
pub use genta::event;
pub use genta::render;

pub mod app;
pub mod journal;
pub mod session;
pub mod ui;
