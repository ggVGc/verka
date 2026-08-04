//! Styra client configuration.
//!
//! These values are static for now. Keeping them behind this type gives a
//! future config-file loader one place to populate without spreading defaults
//! through the interface.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub editor: String,
    pub terminal: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            editor: "nvim".into(),
            terminal: "urxvt".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_editor_is_neovim() {
        let config = Config::default();
        assert_eq!(config.editor, "nvim");
        assert_eq!(config.terminal, "urxvt");
    }
}
