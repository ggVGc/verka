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

/// The terminal emulator a window of its own is asked of. Both defaults need
/// one — Neovim draws on a terminal, and a shell is typed into one — and Styra
/// is already using the terminal it was started in.
const TERMINAL: &str = "urxvt";

/// How that emulator is told what to run in the window it opens.
const RUN: &str = "-e";

impl Configuration for Defaults {
    fn open_file(&self, path: &Path) -> Command {
        let mut command = in_terminal();
        command.arg(FILE_OPENER).arg(path);
        command
    }

    fn open_terminal(&self, argv: &[OsString]) -> Command {
        let mut command = in_terminal();
        command.args(argv);
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
