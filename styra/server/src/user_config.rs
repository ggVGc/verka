//! The operator's own server settings, kept with the rest of their Styra
//! configuration rather than in any one Workspace.
//!
//! Today it holds one thing: environment variables every interaction sandbox
//! is given, whatever the Workspace, profile, or templates. It is read afresh
//! at each launch, so an edit applies to the next interaction without
//! restarting the server.
//!
//! ```toml
//! [environment]
//! CARGO_TERM_COLOR = "always"
//! RUSTUP_HOME = "~/.rustup"
//! ```

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "config.toml";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserConfig {
    /// Variables set in every interaction sandbox.
    pub environment: BTreeMap<String, String>,
}

/// `$XDG_CONFIG_HOME/styra/config.toml`, beside the client's own preferences.
pub fn default_path() -> Option<PathBuf> {
    config_home(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .map(|home| home.join("styra").join(FILE_NAME))
}

fn config_home(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(PathBuf::from).map(|home| home.join(".config")))
}

/// Read the file, treating one that does not exist as empty. A file that
/// exists but cannot be read or parsed is an error: launching without the
/// variables the operator asked for would be a quieter failure, not a safer one.
pub fn load(path: &Path) -> Result<UserConfig> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserConfig::default());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading Styra user config {}", path.display()));
        }
    };
    toml::from_str(&text).with_context(|| format!("parsing Styra user config {}", path.display()))
}

/// The environment from the file at `path`, ready to layer into a launch.
///
/// A `~` names the *host* home, as it does in a template's environment: the
/// sandbox's own `HOME` is a disposable directory nothing outside it points at.
pub fn environment(path: Option<&Path>) -> Result<BTreeMap<OsString, OsString>> {
    let Some(path) = path else {
        return Ok(BTreeMap::new());
    };
    let mut environment = load(path)?
        .environment
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();
    driva::expand_environment_home(&mut environment)?;
    Ok(environment)
}

#[cfg(test)]
mod tests;
