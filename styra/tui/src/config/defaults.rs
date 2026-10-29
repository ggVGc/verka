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

impl Configuration for Defaults {
    fn open_file(&self, path: &Path) -> Command {
        let mut command = in_terminal();
        command.arg(FILE_OPENER).arg(path);
        command
    }

    fn open_file_at(&self, path: &Path, line: u32) -> Command {
        let mut command = in_terminal();
        command.arg(FILE_OPENER).arg(format!("+{line}")).arg(path);
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
    fn default_commands_use_the_expected_program_and_arguments() {
        for (name, command, program, args) in [
            (
                "file at line",
                Defaults.open_file_at(Path::new("/work/src/monitor.c"), 42),
                "urxvt",
                vec!["-e", "nvim", "+42", "/work/src/monitor.c"],
            ),
            (
                "file",
                Defaults.open_file(Path::new("/work/src/monitor.c")),
                "urxvt",
                vec!["-e", "nvim", "/work/src/monitor.c"],
            ),
            (
                "URL",
                Defaults.open_url("https://example.com/a"),
                "firefox",
                vec!["https://example.com/a"],
            ),
            (
                "shell",
                Defaults.open_terminal(&[
                    OsString::from("/usr/bin/tmux"),
                    OsString::from("attach-session"),
                ]),
                "urxvt",
                vec!["-e", "/usr/bin/tmux", "attach-session"],
            ),
        ] {
            assert_eq!(command.get_program(), OsStr::new(program), "{name}");
            assert_eq!(
                argv(&command),
                args.iter().map(OsStr::new).collect::<Vec<_>>(),
                "{name}"
            );
        }
    }

    #[test]
    fn the_interaction_directory_opens_in_fish() {
        assert_eq!(Defaults.shell(), [OsString::from("fish")]);
    }
}
