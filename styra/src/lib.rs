//! Styra: an interactive, isolated agent session runner.
//!
//! The provider wire format stops in [`event`]; the rest of the application
//! consumes only Styra's own event vocabulary, and Driva stays an
//! uninterpreted process transport. See `DESIGN.md`.

pub mod agent;
pub mod app;
pub mod appserver;
pub mod event;
pub mod journal;
pub mod session;
pub mod ui;
