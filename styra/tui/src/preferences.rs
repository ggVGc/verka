//! Persistent preferences owned by the terminal client.
//!
//! Sessions record what they actually launched with on the server. This file
//! has a different job: remember what a brand-new client should offer before
//! any Session exists — both the agent to launch and the sandbox policy to
//! launch it under.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use styra_protocol::agent::{validate_selection, Provider, Selection};

use crate::app::LaunchPolicy;

const FILE_NAME: &str = "defaults.json";

/// What a brand-new client starts from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub selection: Selection,
    /// This client's standing launch policy: what it offers as a new
    /// interaction's own inputs, over whatever the Workspace being launched in
    /// already stands for. Empty unless the operator saved one.
    pub launch: LaunchPolicy,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            selection: Selection::new(Provider::Codex),
            launch: LaunchPolicy::default(),
        }
    }
}

/// The per-user file holding the default launch selection.
pub fn default_path() -> Result<PathBuf> {
    config_home(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .context("neither XDG_CONFIG_HOME nor HOME is set; cannot locate Styra preferences")
    .map(|home| home.join("styra").join(FILE_NAME))
}

fn config_home(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(PathBuf::from).map(|home| home.join(".config")))
}

/// Read the saved defaults, falling back only when no preference has been saved
/// yet. A malformed or no-longer-valid file is reported instead of silently
/// launching something other than what the operator selected.
///
/// A file written before defaults grew past the selection holds a bare
/// [`Selection`]. That shape is still read, and keeps its meaning, rather than
/// erroring every operator who has one out of their next launch; the next save
/// rewrites it in the current shape.
pub fn load_or_default(path: &Path) -> Result<Defaults> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Defaults::default());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading Styra defaults from {}", path.display()));
        }
    };
    let defaults = match serde_json::from_slice::<Defaults>(&bytes) {
        Ok(defaults) => defaults,
        Err(current) => match serde_json::from_slice::<Selection>(&bytes) {
            Ok(selection) => Defaults {
                selection,
                launch: LaunchPolicy::default(),
            },
            // Report the failure to read the current shape: a file that is
            // neither is far more likely to be a damaged current one than an
            // ancient one, and that is the more useful error to show.
            Err(_) => {
                return Err(current)
                    .with_context(|| format!("parsing Styra defaults from {}", path.display()))
            }
        },
    };
    validate_selection(&defaults.selection)
        .with_context(|| format!("invalid Styra defaults in {}", path.display()))?;
    Ok(defaults)
}

/// Replace the saved default selection, keeping any saved launch policy.
pub fn save_selection(path: &Path, selection: &Selection) -> Result<()> {
    let mut defaults = load_or_default(path).unwrap_or_default();
    defaults.selection = selection.clone();
    save(path, &defaults)
}

/// Replace the saved default launch policy, keeping the saved selection.
pub fn save_launch(path: &Path, launch: &LaunchPolicy) -> Result<()> {
    let mut defaults = load_or_default(path).unwrap_or_default();
    defaults.launch = launch.clone();
    save(path, &defaults)
}

pub fn load_recent_models(defaults_path: &Path) -> Vec<String> {
    styra_server::model_picker::load(&styra_server::model_picker::path(defaults_path))
}

pub fn record_recent_model(defaults_path: &Path, model: &str) -> Result<()> {
    styra_server::model_picker::record(&styra_server::model_picker::path(defaults_path), model)
}

/// Atomically replace the saved defaults.
pub fn save(path: &Path, defaults: &Defaults) -> Result<()> {
    validate_selection(&defaults.selection)
        .context("refusing to save an invalid launch selection")?;
    let mut bytes =
        serde_json::to_vec_pretty(defaults).context("encoding the Styra launch defaults")?;
    bytes.push(b'\n');
    styra_server::model_picker::write_atomically(path, &bytes)
}

/// Write `bytes` to `path` through a temporary file in the same directory, so
/// a crash mid-write leaves the previous contents rather than a half-file.
/// The preferences directory and everything in it stay owner-only.
#[cfg(test)]
mod tests {
    use super::*;
    use styra_protocol::agent::Effort;

    fn defaults_path() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings").join(FILE_NAME);
        (directory, path)
    }

    fn claude_selection() -> Selection {
        Selection {
            provider: Provider::Claude,
            model: "claude-sonnet-5".into(),
            effort: Effort::Max,
        }
    }

    #[test]
    fn xdg_config_home_takes_precedence_with_a_home_fallback() {
        assert_eq!(
            config_home(Some("/config".into()), Some("/home/user".into())),
            Some(PathBuf::from("/config"))
        );
        assert_eq!(
            config_home(None, Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.config"))
        );
        assert_eq!(
            config_home(Some(OsString::new()), Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.config"))
        );
    }

    #[test]
    fn a_missing_file_uses_the_declared_codex_defaults() {
        let (_directory, path) = defaults_path();
        let defaults = load_or_default(&path).unwrap();
        assert_eq!(defaults.selection, Selection::new(Provider::Codex));
        assert_eq!(defaults.launch, LaunchPolicy::default());
    }

    #[test]
    fn a_saved_selection_and_launch_policy_round_trip() {
        let (_directory, path) = defaults_path();
        let defaults = Defaults {
            selection: claude_selection(),
            launch: LaunchPolicy {
                network: Some(true),
                writable_workspace: None,
                templates: vec!["rust".into()],
                mounts: vec![styra_protocol::LaunchMount {
                    source: PathBuf::from("/srv/data"),
                    destination: None,
                    writable: false,
                }],
                ignore_workspace: false,
                ..LaunchPolicy::default()
            },
        };

        save(&path, &defaults).unwrap();
        assert_eq!(load_or_default(&path).unwrap(), defaults);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    /// Each of the two halves is saved without disturbing the other, so
    /// pressing `D` in the launch picker cannot silently drop a saved sandbox
    /// policy (or the other way round).
    #[test]
    fn saving_one_half_of_the_defaults_keeps_the_other() {
        let (_directory, path) = defaults_path();
        let launch = LaunchPolicy {
            network: Some(true),
            templates: vec!["browser".into()],
            ..LaunchPolicy::default()
        };
        let selection = claude_selection();

        save_launch(&path, &launch).unwrap();
        save_selection(&path, &selection).unwrap();

        let defaults = load_or_default(&path).unwrap();
        assert_eq!(defaults.launch, launch);
        assert_eq!(defaults.selection, selection);
    }

    /// A defaults file written before the launch policy existed holds a bare
    /// selection. It must keep working rather than erroring its owner out of
    /// their next launch.
    #[test]
    fn a_defaults_file_holding_only_a_selection_is_still_read() {
        let (_directory, path) = defaults_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let selection = claude_selection();
        fs::write(&path, serde_json::to_vec(&selection).unwrap()).unwrap();

        let defaults = load_or_default(&path).unwrap();
        assert_eq!(defaults.selection, selection);
        assert_eq!(defaults.launch, LaunchPolicy::default());
    }

    #[test]
    fn malformed_saved_defaults_are_not_silently_ignored() {
        let (_directory, path) = defaults_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{ definitely not json").unwrap();

        let error = load_or_default(&path).unwrap_err();
        assert!(error.to_string().contains("parsing Styra defaults"));
    }
}
