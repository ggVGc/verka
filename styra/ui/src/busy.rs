//! The notice that stands in the message box's place while the client is
//! blocked on a synchronous call the operator started from it.
//!
//! Sending into a new Git workspace branches the repository and checks out a
//! linked worktree before the prompt goes anywhere, and nothing is read from
//! the keyboard until that returns. Leaving the box on screen over that wait
//! offers typing that does not happen; a notice down among the others is easy
//! to miss and says nothing about the keyboard. So the box goes, and in its
//! place is the one thing that is true: the client is working, and waiting is
//! all there is to do.

/// What the blocking notice says: the work in progress, in the operator's
/// terms.
pub struct BusyView<'a> {
    pub message: &'a str,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/busy.rs"]
mod rendering;
