//! Styra client configuration.
//!
//! Configuration is a trait rather than a struct so the interface depends on
//! what it asks for and not on where the answers came from. Today the only
//! implementation is [`Defaults`], compiled in; nothing loads a file yet. When
//! something does, it becomes another implementor and no caller changes.

pub mod defaults;

pub use defaults::Defaults;

/// The settings the terminal client reads.
pub trait Configuration {
    /// The program a file is handed to when the operator opens one — from the
    /// Files view, a typed `files` answer, or a reference in a reply.
    fn file_opener(&self) -> &str;

    /// The terminal emulator the opener is started in. The client itself owns
    /// this terminal, so an opener that draws on one of its own needs a window
    /// to draw in.
    fn terminal(&self) -> &str;
}
