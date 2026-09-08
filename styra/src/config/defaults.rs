//! The configuration Styra runs on when nothing has been configured.
//!
//! One place holding every default value, so a future loader has somewhere to
//! fall back to rather than repeating these strings at each use.

use std::path::Path;
use std::process::Command;

use super::Configuration;

/// The compiled-in configuration.
pub struct Defaults;

/// The program files open in.
const FILE_OPENER: &str = "nvim";

/// The terminal emulator it is wrapped in. Neovim draws on a terminal and
/// Styra is already using this one, so the default opener needs a window of
/// its own — which is the default's business to know, not the client's.
const TERMINAL: &str = "urxvt";

impl Configuration for Defaults {
    fn open_file(&self, path: &Path) -> Command {
        let mut command = Command::new(TERMINAL);
        command.arg("-e").arg(FILE_OPENER).arg(path);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn files_open_in_neovim_in_a_new_terminal_window() {
        let command = Defaults.open_file(Path::new("/work/src/monitor.c"));

        assert_eq!(command.get_program(), OsStr::new("urxvt"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("-e"),
                OsStr::new("nvim"),
                OsStr::new("/work/src/monitor.c"),
            ]
        );
    }
}
