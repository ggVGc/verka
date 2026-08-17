//! Persistent preferences owned by the terminal client.
//!
//! Sessions record what they actually launched with on the server. This file
//! has a different job: remember what a brand-new client should offer before
//! any Session exists — both the agent to launch and the sandbox policy to
//! launch it under.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use styra_server::agent::{validate_selection, Provider, Selection};

use crate::app::LaunchInputs;

const FILE_NAME: &str = "defaults.json";

/// What a brand-new client starts from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub selection: Selection,
    /// The standing sandbox policy: templates, extra mounts, and whether
    /// networking is permitted. Empty unless the operator saved one.
    pub launch: LaunchInputs,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            selection: Selection::new(Provider::Codex),
            launch: LaunchInputs::default(),
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
                launch: LaunchInputs::default(),
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
pub fn save_launch(path: &Path, launch: &LaunchInputs) -> Result<()> {
    let mut defaults = load_or_default(path).unwrap_or_default();
    defaults.launch = launch.clone();
    save(path, &defaults)
}

/// Atomically replace the saved defaults.
pub fn save(path: &Path, defaults: &Defaults) -> Result<()> {
    validate_selection(&defaults.selection)
        .context("refusing to save an invalid launch selection")?;
    let parent = path
        .parent()
        .context("Styra defaults path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating Styra preferences directory {}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("securing Styra preferences directory {}", parent.display()))?;

    let temporary = parent.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("creating temporary defaults file {}", temporary.display()))?;
        serde_json::to_writer_pretty(&mut file, defaults)
            .context("encoding the Styra launch defaults")?;
        file.write_all(b"\n")
            .context("finishing the Styra launch defaults")?;
        file.sync_all()
            .context("flushing the Styra launch defaults")?;
        fs::rename(&temporary, path)
            .with_context(|| format!("installing Styra defaults at {}", path.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        fs::remove_file(&temporary).ok();
    }
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_server::agent::Effort;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("styra-preferences-{}-{name}", std::process::id()))
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
        let path = temp_path("missing").join(FILE_NAME);
        fs::remove_dir_all(path.parent().unwrap()).ok();
        let defaults = load_or_default(&path).unwrap();
        assert_eq!(defaults.selection, Selection::new(Provider::Codex));
        assert_eq!(defaults.launch, LaunchInputs::default());
    }

    #[test]
    fn a_saved_selection_and_launch_policy_round_trip() {
        let root = temp_path("round-trip");
        fs::remove_dir_all(&root).ok();
        let path = root.join(FILE_NAME);
        let defaults = Defaults {
            selection: Selection {
                provider: Provider::Claude,
                model: "claude-sonnet-5".into(),
                effort: Effort::Max,
            },
            launch: LaunchInputs {
                network: true,
                templates: vec!["rust".into()],
                mounts: vec![styra_server::LaunchMount {
                    source: PathBuf::from("/srv/data"),
                    destination: None,
                    writable: false,
                }],
            },
        };

        save(&path, &defaults).unwrap();
        assert_eq!(load_or_default(&path).unwrap(), defaults);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).ok();
    }

    /// Each of the two halves is saved without disturbing the other, so
    /// pressing `D` in the launch picker cannot silently drop a saved sandbox
    /// policy (or the other way round).
    #[test]
    fn saving_one_half_of_the_defaults_keeps_the_other() {
        let root = temp_path("halves");
        fs::remove_dir_all(&root).ok();
        let path = root.join(FILE_NAME);
        let launch = LaunchInputs {
            network: true,
            templates: vec!["browser".into()],
            mounts: Vec::new(),
        };
        let selection = Selection {
            provider: Provider::Claude,
            model: "claude-sonnet-5".into(),
            effort: Effort::Max,
        };

        save_launch(&path, &launch).unwrap();
        save_selection(&path, &selection).unwrap();

        let defaults = load_or_default(&path).unwrap();
        assert_eq!(defaults.launch, launch);
        assert_eq!(defaults.selection, selection);
        fs::remove_dir_all(root).ok();
    }

    /// A defaults file written before the launch policy existed holds a bare
    /// selection. It must keep working rather than erroring its owner out of
    /// their next launch.
    #[test]
    fn a_defaults_file_holding_only_a_selection_is_still_read() {
        let root = temp_path("legacy");
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let path = root.join(FILE_NAME);
        let selection = Selection {
            provider: Provider::Claude,
            model: "claude-sonnet-5".into(),
            effort: Effort::Max,
        };
        fs::write(&path, serde_json::to_vec(&selection).unwrap()).unwrap();

        let defaults = load_or_default(&path).unwrap();
        assert_eq!(defaults.selection, selection);
        assert_eq!(defaults.launch, LaunchInputs::default());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn malformed_saved_defaults_are_not_silently_ignored() {
        let root = temp_path("malformed");
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let path = root.join(FILE_NAME);
        fs::write(&path, b"{ definitely not json").unwrap();

        let error = load_or_default(&path).unwrap_err();
        assert!(error.to_string().contains("parsing Styra defaults"));
        fs::remove_dir_all(root).ok();
    }
}
