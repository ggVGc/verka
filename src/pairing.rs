//! The store↔project pairing, read from `<store>/pairing.toml`.
//!
//! The workbench layout pairs the two repositories positionally: the store
//! lives at `../.llaundry` relative to the project root, and nothing else
//! binds them. This file makes the binding a recorded fact: it holds the
//! project repository's *root commit* — the one hash that identifies the
//! repository rather than a point in its history, stable across branching,
//! merging, and ordinary rebases. A full-history rewrite changes it, and
//! deliberately so: every recorded output commit just became suspect, and
//! `llaundry pair --verify` should say so.
//!
//! Unlike `config.toml` this is not a preference but a fact, written once by
//! `init` or `llaundry pair` and versioned with the rest of the store. The
//! project repository stays completely ordinary: the pairing lives entirely
//! on the store side.
//!
//! ```toml
//! schema = 1
//! root-commit = "8a1f9c2e..."   # first-parent root of the project's HEAD
//! paired-at = 1719571200000    # Unix milliseconds
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
        let dir = std::env::temp_dir().join(format!("llaundry-pair-{tag}-{}", std::process::id()));
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
        };
        pairing.save(&dir).unwrap();
        let got = Pairing::load(&dir).unwrap().unwrap();
        assert_eq!(got.root_commit, "8a1f9c2e");
        assert_eq!(got.paired_at, 1719571200000);
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
