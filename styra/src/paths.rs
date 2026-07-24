//! Default filesystem locations shared by the Styra server and clients.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;

pub fn default_store() -> Result<PathBuf> {
    config_home(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .context("neither XDG_CONFIG_HOME nor HOME is set; pass --store or --socket explicitly")
    .map(|home| home.join("styra"))
}

pub fn default_socket() -> Result<PathBuf> {
    Ok(default_store()?.join("styra.sock"))
}

fn config_home(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(PathBuf::from).map(|home| home.join(".config")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_config_home_takes_precedence() {
        assert_eq!(
            config_home(Some("/xdg".into()), Some("/home/user".into())),
            Some(PathBuf::from("/xdg"))
        );
    }

    #[test]
    fn home_supplies_the_xdg_default_when_the_variable_is_unset_or_empty() {
        assert_eq!(
            config_home(None, Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.config"))
        );
        assert_eq!(
            config_home(Some(OsString::new()), Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.config"))
        );
    }
}
