//! Persistent preferences owned by the terminal client.
//!
//! Sessions record what they actually launched with on the server. This file
//! has a different job: remember what a brand-new client should offer before
//! any Session exists.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use styra_server::agent::{validate_selection, Provider, Selection};

const FILE_NAME: &str = "defaults.json";

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

/// Read the saved default, falling back only when no preference has been saved
/// yet. A malformed or no-longer-valid file is reported instead of silently
/// launching something other than what the operator selected.
pub fn load_or_default(path: &Path) -> Result<Selection> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Selection::new(Provider::Codex));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading Styra defaults from {}", path.display()));
        }
    };
    let selection: Selection = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing Styra defaults from {}", path.display()))?;
    validate_selection(&selection)
        .with_context(|| format!("invalid Styra defaults in {}", path.display()))?;
    Ok(selection)
}

/// Atomically replace the saved default selection.
pub fn save(path: &Path, selection: &Selection) -> Result<()> {
    validate_selection(selection).context("refusing to save an invalid launch selection")?;
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
        serde_json::to_writer_pretty(&mut file, selection)
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
        assert_eq!(
            load_or_default(&path).unwrap(),
            Selection::new(Provider::Codex)
        );
    }

    #[test]
    fn a_saved_provider_model_and_effort_round_trip() {
        let root = temp_path("round-trip");
        fs::remove_dir_all(&root).ok();
        let path = root.join(FILE_NAME);
        let selection = Selection {
            provider: Provider::Claude,
            model: "claude-sonnet-5".into(),
            effort: Effort::Max,
        };

        save(&path, &selection).unwrap();
        assert_eq!(load_or_default(&path).unwrap(), selection);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
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
