//! Styra client configuration.
//!
//! Configuration is a trait rather than a struct so the interface depends on
//! what it asks for and not on where the answers came from. Today the only
//! implementation is [`Defaults`], compiled in; nothing loads a file yet. When
//! something does, it becomes another implementor and no caller changes.

pub mod defaults;

pub use defaults::Defaults;

use std::path::Path;
use std::process::Command;

/// The settings the terminal client reads.
pub trait Configuration {
    /// The command that opens `path` for the operator — from the Files view, a
    /// typed `files` answer, or a reference in a reply.
    ///
    /// A whole command rather than a program name, because how a file is
    /// opened is as much configuration as what opens it: a terminal editor has
    /// to be wrapped in an emulator, a graphical one must not be, and either
    /// may want arguments of its own. Deciding that here leaves the client with
    /// nothing to assume — it spawns what it is given.
    fn open_file(&self, path: &Path) -> Command;
}
