//! The configuration Styra runs on when nothing has been configured.
//!
//! One place holding every default value, so a future loader has somewhere to
//! fall back to rather than repeating these strings at each use.

use super::Configuration;

/// The compiled-in configuration.
pub struct Defaults;

/// The program files open in.
const FILE_OPENER: &str = "nvim";

/// The terminal emulator it opens in.
const TERMINAL: &str = "urxvt";

impl Configuration for Defaults {
    fn file_opener(&self) -> &str {
        FILE_OPENER
    }

    fn terminal(&self) -> &str {
        TERMINAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_open_in_neovim() {
        assert_eq!(Defaults.file_opener(), "nvim");
        assert_eq!(Defaults.terminal(), "urxvt");
    }
}
