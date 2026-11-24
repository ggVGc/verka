//! Default filesystem locations shared by the Styra server and clients.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;

pub fn default_store() -> Result<PathBuf> {
    store_path(
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
        "styra",
    )
    .context("neither XDG_STATE_HOME nor HOME is set; pass --store explicitly")
}

/// Durable state for a server embedded in its client process.
///
/// Keep this separate from [`default_store`] so a standalone client and a
/// daemon can run at the same time without both believing they exclusively
/// own the same Workspace and Session metadata.
pub fn default_standalone_store() -> Result<PathBuf> {
    store_path(
        std::env::var_os("XDG_STATE_HOME"),
        std::env::var_os("HOME"),
        "styra-standalone",
    )
    .context("neither XDG_STATE_HOME nor HOME is set; pass --store explicitly")
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

fn store_path(
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
    directory: &str,
) -> Option<PathBuf> {
    state_home(xdg_state_home, home).map(|home| home.join(directory))
}

fn runtime_home(xdg_runtime_dir: Option<OsString>) -> Option<PathBuf> {
    xdg_runtime_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests;
