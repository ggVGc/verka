//! Which Workspace the screen is showing, and where on this host it is.
//!
//! Held apart from [`App`](crate::app::App) because two questions are asked of
//! it constantly and were answered separately each time: what root to resolve
//! the agent's reported paths against, and what directory it is working in.
//! Both fall back to the directory this client was started in — a replayed
//! journal has no live Workspace but its paths still have to resolve — and
//! that fallback was written three times, in [`crate::app`],
//! [`crate::ui::files`] and [`crate::ui::footer`].
//!
//! The Session on screen is not here: a Session names the durable Workspace it
//! belongs to, but the Workspace outlives it and the operator switches
//! between Sessions within one.

use std::path::{Path, PathBuf};

/// The Workspace the screen is showing: how it is identified, and where it is.
#[derive(Default)]
pub struct Location {
    /// Durable Workspace containing the current Session, when known.
    pub id: Option<String>,
    /// Operator-facing name of the active Workspace. Resolved from Workspace
    /// metadata (with the host directory name as its fallback) by the client,
    /// since Sessions only carry the durable Workspace id.
    pub name: Option<String>,
    /// The host directory backing the agent's sandboxed workspace, when known.
    /// A replayed journal has no live workspace.
    root: Option<PathBuf>,
    /// The directory within `root` a live interaction is working in. Codex can
    /// be told to change it mid-session, so it is not always `root`.
    working_directory: Option<PathBuf>,
}

impl Location {
    /// The host directory backing the agent's workspace, if there is a live
    /// one. `None` for a replayed journal.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Record the host directory backing the agent's workspace. A session
    /// starts working at its root, so both move together.
    pub fn enter(&mut self, root: PathBuf) {
        self.working_directory = Some(root.clone());
        self.root = Some(root);
    }

    /// Follow the agent into another directory, without changing which
    /// Workspace this is.
    pub fn change_directory(&mut self, directory: PathBuf) {
        self.working_directory = Some(directory);
    }

    /// The root to resolve the agent's reported paths against.
    ///
    /// Falls back to wherever this client was started, so a replayed journal
    /// still resolves the paths it mentions rather than showing none.
    pub fn root_or_current_directory(&self) -> Option<PathBuf> {
        self.root.clone().or_else(|| std::env::current_dir().ok())
    }

    /// Where the agent is working, for the footer to name. Same fallback, and
    /// for the same reason.
    pub fn working_directory_or_current(&self) -> Option<PathBuf> {
        self.working_directory
            .clone()
            .or_else(|| std::env::current_dir().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_no_root_until_a_live_workspace_records_one() {
        let location = Location::default();

        assert_eq!(location.root(), None);
    }

    /// A session starts working at the root of the Workspace it was launched
    /// in, so recording one sets both.
    #[test]
    fn entering_a_workspace_starts_working_at_its_root() {
        let mut location = Location::default();

        location.enter(PathBuf::from("/home/op/project"));

        assert_eq!(location.root(), Some(Path::new("/home/op/project")));
        assert_eq!(
            location.working_directory_or_current(),
            Some(PathBuf::from("/home/op/project"))
        );
    }

    /// Codex can be told to work elsewhere mid-session. That moves the working
    /// directory without moving the Workspace, which is still what reported
    /// paths resolve against.
    #[test]
    fn changing_directory_does_not_move_the_workspace() {
        let mut location = Location::default();
        location.enter(PathBuf::from("/home/op/project"));

        location.change_directory(PathBuf::from("/home/op/project/crates/inner"));

        assert_eq!(location.root(), Some(Path::new("/home/op/project")));
        assert_eq!(
            location.working_directory_or_current(),
            Some(PathBuf::from("/home/op/project/crates/inner"))
        );
    }

    /// A replayed journal has no live Workspace, but the paths it mentions are
    /// still worth resolving against wherever the operator is.
    #[test]
    fn without_a_workspace_both_questions_fall_back_to_this_directory() {
        let location = Location::default();
        let current = std::env::current_dir().ok();

        assert_eq!(location.root_or_current_directory(), current);
        assert_eq!(location.working_directory_or_current(), current);
    }
}
