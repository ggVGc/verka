//! Model picker history shared by the terminal and editor clients.
use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use styra_protocol::agent::PROVIDERS;
use styra_protocol::ModelSummary;

const FILE_NAME: &str = "recent-models.json";
const RECENT_MODELS: usize = 16;

pub fn default_path() -> Result<PathBuf> {
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context("neither XDG_CONFIG_HOME nor HOME is set; cannot locate Styra preferences")?;
    Ok(home.join("styra").join(FILE_NAME))
}

pub fn path(defaults_path: &Path) -> PathBuf {
    defaults_path.with_file_name(FILE_NAME)
}

/// Unreadable history falls back to catalog order.
pub fn load(path: &Path) -> Vec<String> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn note(recent: &mut Vec<String>, model: &str) {
    recent.retain(|previous| previous != model);
    recent.insert(0, model.to_owned());
    recent.truncate(RECENT_MODELS);
}

/// Read again before writing so another client's recent picks are retained.
pub fn record(path: &Path, model: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("model history path has no parent directory")?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    // A separate lock survives the atomic replacement of the history file.
    // Closing it releases the lock on every return path.
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path.with_extension("lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error()).context("locking model history");
    }
    let mut recent = load(path);
    note(&mut recent, model);
    let mut bytes =
        serde_json::to_vec_pretty(&recent).context("encoding the recently selected models")?;
    bytes.push(b'\n');
    write_atomically(path, &bytes)
}

/// Stable order keeps unselected models in catalog order and each effort ladder together.
pub fn sort<T>(rows: &mut [T], recent: &[String], model: impl Fn(&T) -> &str) {
    rows.sort_by_key(|row| {
        recent
            .iter()
            .position(|previous| previous == model(row))
            .unwrap_or(usize::MAX)
    });
}

pub fn catalog(recent: &[String]) -> Vec<ModelSummary> {
    let mut models = Vec::new();
    for provider in PROVIDERS {
        for model in provider.models() {
            models.push(ModelSummary {
                provider,
                model: model.id.to_owned(),
                efforts: model.efforts.to_vec(),
                default_effort: provider.default_effort_for(model.id),
            });
        }
    }
    sort(&mut models, recent, |row| &row.model);
    models
}

pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("Styra defaults path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating Styra preferences directory {}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("securing Styra preferences directory {}", parent.display()))?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(FILE_NAME);
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("creating temporary file {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("writing {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("flushing {}", temporary.display()))?;
        fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))?;
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

    #[test]
    fn clients_share_history_and_reselecting_moves_a_model_to_the_front() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let catalog = catalog(&[]);
        let first = &catalog[1].model;
        let second = &catalog[catalog.len() - 1].model;
        record(&path, first).unwrap();
        record(&path, second).unwrap();
        record(&path, first).unwrap();
        let recent = load(&path);
        assert_eq!(recent, vec![first.clone(), second.clone()]);
        let ordered = super::catalog(&recent);
        assert_eq!(&ordered[0].model, first);
        assert_eq!(&ordered[1].model, second);
        let remaining: Vec<_> = ordered.iter().skip(2).collect();
        let expected: Vec<_> = catalog
            .iter()
            .filter(|row| &row.model != first && &row.model != second)
            .collect();
        assert_eq!(remaining, expected);
    }

    #[test]
    fn concurrent_clients_preserve_each_others_selections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::thread::scope(|scope| {
            for model in ["one", "two", "three"] {
                let path = &path;
                scope.spawn(move || record(path, model).unwrap());
            }
        });
        let mut models = load(&path);
        models.sort();
        assert_eq!(models, ["one", "three", "two"]);
    }

    #[test]
    fn missing_or_malformed_history_uses_catalog_order_and_history_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        assert!(load(&path).is_empty());
        fs::write(&path, b"invalid json").unwrap();
        assert!(load(&path).is_empty());
        for model in 0..20 {
            record(&path, &model.to_string()).unwrap();
        }
        let recent = load(&path);
        assert_eq!(recent.len(), RECENT_MODELS);
        assert_eq!(recent[0], "19");
    }
}
