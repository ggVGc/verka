//! The store↔project pairing, read from `<store>/pairing.toml`.
//!
//! The workbench layout pairs the two repositories positionally: the store
//! lives at `../.linka` relative to the project root, and nothing else
//! binds them. This file makes the binding a recorded fact: it holds the
//! project repository's *root commit* — the one hash that identifies the
//! repository rather than a point in its history, stable across branching,
//! merging, and ordinary rebases. A full-history rewrite changes it, and
//! deliberately so: every recorded output commit just became suspect, and
//! `linka pair --verify` should say so.
//!
//! Unlike `config.toml` this is not a preference but a fact, written once by
//! `init` or `linka pair` and versioned with the rest of the store. The
//! project repository stays completely ordinary: the pairing lives entirely
//! on the store side.
//!
//! Besides the checked identity, the file may carry purely informational
//! fields for human readers — a descriptive short-name and the project's
//! remote URL, captured at pairing time. Nothing verifies them.
//!
//! ```toml
//! schema = 1
//! root-commit = "8a1f9c2e..."   # first-parent root of the project's HEAD
//! paired-at = 1719571200000    # Unix milliseconds
//! name = "splurt"              # optional, informational only
//! remote = "git@host:me/splurt.git"  # optional, informational only
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The pairing file's name inside the store root.
pub const PAIRING_FILE: &str = "pairing.toml";

/// The recorded pairing: which project repository this store describes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Pairing {
    pub schema: u32,
    /// The project repository's root commit (first-parent root of HEAD).
    pub root_commit: String,
    /// When the pairing was recorded, Unix milliseconds.
    pub paired_at: i64,
    /// A descriptive short-name for the project. Informational only — for
    /// human readers of the store; never checked against anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The project's remote URL as observed at pairing time (git remote
    /// `origin`). Informational only; never checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl Pairing {
    /// Load `<store_root>/pairing.toml`, returning `None` if the store is not
    /// paired (the file does not exist). A present-but-unreadable or malformed
    /// file is an error — a damaged pairing should be surfaced, not treated as
    /// unpaired.
    pub fn load(store_root: &Path) -> Result<Option<Self>> {
        let path = store_root.join(PAIRING_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        toml::from_str(&text)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display()))
    }

    /// Write `<store_root>/pairing.toml`. The caller commits it to the
    /// workbench repository like any other store change.
    pub fn save(&self, store_root: &Path) -> Result<()> {
        let path = store_root.join(PAIRING_FILE);
        let data = toml::to_string_pretty(self).context("serialising pairing")?;
        std::fs::write(&path, data).with_context(|| format!("writing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("linka-pair-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_means_unpaired() {
        let dir = temp_dir("missing");
        assert!(Pairing::load(&dir).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pairing_roundtrips() {
        let dir = temp_dir("roundtrip");
        let pairing = Pairing {
            schema: 1,
            root_commit: "8a1f9c2e".into(),
            paired_at: 1719571200000,
            name: Some("splurt".into()),
            remote: Some("git@host:me/splurt.git".into()),
        };
        pairing.save(&dir).unwrap();
        let got = Pairing::load(&dir).unwrap().unwrap();
        assert_eq!(got.root_commit, "8a1f9c2e");
        assert_eq!(got.paired_at, 1719571200000);
        assert_eq!(got.name.as_deref(), Some("splurt"));
        assert_eq!(got.remote.as_deref(), Some("git@host:me/splurt.git"));

        // The informational fields are optional: an identity-only file loads,
        // and an identity-only pairing serialises without empty keys.
        let bare = Pairing {
            schema: 1,
            root_commit: "8a1f9c2e".into(),
            paired_at: 0,
            name: None,
            remote: None,
        };
        bare.save(&dir).unwrap();
        let text = std::fs::read_to_string(dir.join(PAIRING_FILE)).unwrap();
        assert!(!text.contains("name"), "{text}");
        assert!(!text.contains("remote"), "{text}");
        assert!(Pairing::load(&dir).unwrap().unwrap().name.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_file_is_an_error_not_unpaired() {
        let dir = temp_dir("malformed");
        std::fs::write(dir.join(PAIRING_FILE), "not = = toml").unwrap();
        assert!(Pairing::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
