//! Default filesystem locations shared by the Styra server and clients.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;

pub fn default_store() -> Result<PathBuf> {
    state_home(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
        .context("neither XDG_STATE_HOME nor HOME is set; pass --store explicitly")
        .map(|home| home.join("styra"))
}

pub fn default_socket() -> Result<PathBuf> {
    runtime_home(std::env::var_os("XDG_RUNTIME_DIR"))
        .context("XDG_RUNTIME_DIR is not set; pass --socket explicitly")
        .map(|home| home.join("styra/styra.sock"))
}

fn state_home(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg_state_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        })
}

fn runtime_home(xdg_runtime_dir: Option<OsString>) -> Option<PathBuf> {
    xdg_runtime_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_state_home_takes_precedence() {
        assert_eq!(
            state_home(Some("/state".into()), Some("/home/user".into())),
            Some(PathBuf::from("/state"))
        );
    }

    #[test]
    fn home_supplies_the_state_default_when_the_variable_is_unset_or_empty() {
        assert_eq!(
            state_home(None, Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.local/state"))
        );
        assert_eq!(
            state_home(Some(OsString::new()), Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.local/state"))
        );
    }

    #[test]
    fn runtime_directory_has_no_persistent_fallback() {
        assert_eq!(
            runtime_home(Some("/run/user/1000".into())),
            Some(PathBuf::from("/run/user/1000"))
        );
        assert_eq!(runtime_home(None), None);
        assert_eq!(runtime_home(Some(OsString::new())), None);
    }
}
