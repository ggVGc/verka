//! The configuration Styra runs on when nothing has been configured.
//!
//! One place holding every default value, so a future loader has somewhere to
//! fall back to rather than repeating these strings at each use.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use super::Configuration;

/// The compiled-in configuration.
pub struct Defaults;

/// The program files open in.
const FILE_OPENER: &str = "nvim";

/// The browser web addresses open in. Graphical, so it gets no terminal.
const BROWSER: &str = "firefox";

/// The terminal emulator a window of its own is asked of. Both defaults need
/// one — Neovim draws on a terminal, and a shell is typed into one — and Styra
/// is already using the terminal it was started in.
const TERMINAL: &str = "urxvt";

/// How that emulator is told what to run in the window it opens.
const RUN: &str = "-e";

/// The shell opened in an interaction's working directory.
const SHELL: &str = "fish";

/// What a checkout's changes since its branch point are shown with: Git's own
/// `diff`, which pages through `less` on a terminal, so the window stays open
/// until the operator is done reading.
const GIT: &str = "git";

impl Configuration for Defaults {
    fn open_file(&self, path: &Path) -> Command {
        let mut command = in_terminal();
        command.arg(FILE_OPENER).arg(path);
        command
    }

    fn open_url(&self, url: &str) -> Command {
        let mut command = Command::new(BROWSER);
        command.arg(url);
        command
    }

    fn open_terminal(&self, argv: &[OsString]) -> Command {
        let mut command = in_terminal();
        command.args(argv);
        command
    }

    fn shell(&self) -> Vec<OsString> {
        vec![OsString::from(SHELL)]
    }

    fn open_diff(&self, worktree: &Path, base: &str) -> Command {
        let mut command = in_terminal();
        // Against the working tree rather than `HEAD`, so what the agent has
        // not committed yet is shown alongside what it has.
        command
            .arg(GIT)
            .arg("-C")
            .arg(worktree)
            .arg("diff")
            .arg(base);
        command
    }
}

/// The emulator, ready for the command it is to run. Both defaults open a
/// window the same way, so they say how once.
fn in_terminal() -> Command {
    let mut command = Command::new(TERMINAL);
    command.arg(RUN);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn argv(command: &Command) -> Vec<&OsStr> {
        command.get_args().collect()
    }

    #[test]
    fn files_open_in_neovim_in_a_new_terminal_window() {
        let command = Defaults.open_file(Path::new("/work/src/monitor.c"));

        assert_eq!(command.get_program(), OsStr::new("urxvt"));
        assert_eq!(
            argv(&command),
            [
                OsStr::new("-e"),
                OsStr::new("nvim"),
                OsStr::new("/work/src/monitor.c"),
            ]
        );
    }

    #[test]
    fn web_addresses_open_in_firefox_without_a_terminal() {
        let command = Defaults.open_url("https://example.com/a");

        assert_eq!(command.get_program(), OsStr::new("firefox"));
        assert_eq!(argv(&command), [OsStr::new("https://example.com/a")]);
    }

    #[test]
    fn the_interaction_directory_opens_in_fish() {
        assert_eq!(Defaults.shell(), [OsString::from("fish")]);
    }

    #[test]
    fn a_diff_from_the_branch_point_opens_git_diff_in_a_new_terminal_window() {
        let command = Defaults.open_diff(Path::new("/work/tree"), "4bf5c35d");

        assert_eq!(command.get_program(), OsStr::new("urxvt"));
        assert_eq!(
            argv(&command),
            [
                OsStr::new("-e"),
                OsStr::new("git"),
                OsStr::new("-C"),
                OsStr::new("/work/tree"),
                OsStr::new("diff"),
                OsStr::new("4bf5c35d"),
            ]
        );
    }

    #[test]
    fn a_shell_runs_in_a_new_terminal_window() {
        let command = Defaults.open_terminal(&[
            OsString::from("/usr/bin/tmux"),
            OsString::from("attach-session"),
        ]);

        assert_eq!(command.get_program(), OsStr::new("urxvt"));
        assert_eq!(
            argv(&command),
            [
                OsStr::new("-e"),
                OsStr::new("/usr/bin/tmux"),
                OsStr::new("attach-session"),
            ]
        );
    }
}
