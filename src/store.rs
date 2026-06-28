//! The on-disk store.
//!
//! Layout (all text, all diff-friendly, all meant to live in git):
//!
//! ```text
//! <root>/
//!   objects/<hash>/meta.toml   immutable definition  (content-addressed)
//!   objects/<hash>/body.md     immutable prose        (content-addressed)
//!   refs/<logical-id>          one line: the current version hash  (mutable)
//!   status/<logical-id>.toml   append-only [[event]] log            (append-only)
//! ```
//!
//! `objects/` is the source of truth and is immutable + content-addressed, exactly
//! like git's object store: writing the same content twice is idempotent, and
//! nothing is ever edited in place. `refs/` is the single mutable surface — the
//! pointer that answers "which immutable version is current". `status/` is an
//! append-only event log kept *outside* the hashed content.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::model::{Meta, StatusEvent, StatusLog};

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open an existing store, erroring if it has not been initialised.
    pub fn open(root: PathBuf) -> Result<Self> {
        if !root.join("objects").is_dir() {
            bail!(
                "no llaundry store at {} (run `llaundry init` first)",
                root.display()
            );
        }
        Ok(Store { root })
    }

    /// Create the directory skeleton for a new store.
    pub fn init(root: PathBuf) -> Result<Self> {
        for sub in ["objects", "refs", "status"] {
            fs::create_dir_all(root.join(sub))
                .with_context(|| format!("creating {}/{sub}", root.display()))?;
        }
        Ok(Store { root })
    }

    // --- paths --------------------------------------------------------------

    fn object_dir(&self, hash: &str) -> PathBuf {
        self.root.join("objects").join(hash)
    }
    fn ref_path(&self, id: &str) -> PathBuf {
        self.root.join("refs").join(id)
    }
    fn status_path(&self, id: &str) -> PathBuf {
        self.root.join("status").join(format!("{id}.toml"))
    }

    // --- objects (immutable, content-addressed) -----------------------------

    /// The node hash is `sha256(meta_bytes || 0x00 || body_bytes)`.
    ///
    /// The hash is computed over the exact bytes we are about to write, so the
    /// filename and the content can never drift. The hash is deliberately *not*
    /// stored inside the object (that would be self-referential); the directory
    /// name is the only place it lives.
    fn hash(meta_bytes: &str, body: &str) -> String {
        let mut h = Sha256::new();
        h.update(meta_bytes.as_bytes());
        h.update([0u8]);
        h.update(body.as_bytes());
        hex(&h.finalize())
    }

    /// Write an immutable object and return its hash. Idempotent: if an identical
    /// object already exists it is left untouched.
    pub fn put_object(&self, meta: &Meta, body: &str) -> Result<String> {
        let meta_bytes = toml::to_string_pretty(meta).context("serialising meta")?;
        let hash = Self::hash(&meta_bytes, body);
        let dir = self.object_dir(&hash);
        if !dir.exists() {
            fs::create_dir_all(&dir)?;
            fs::write(dir.join("meta.toml"), &meta_bytes)?;
            fs::write(dir.join("body.md"), body)?;
        }
        Ok(hash)
    }

    pub fn get_object(&self, hash: &str) -> Result<(Meta, String)> {
        let dir = self.object_dir(hash);
        let meta_bytes = fs::read_to_string(dir.join("meta.toml"))
            .with_context(|| format!("reading object {hash}"))?;
        let meta: Meta = toml::from_str(&meta_bytes).context("parsing meta.toml")?;
        let body = fs::read_to_string(dir.join("body.md")).unwrap_or_default();
        Ok((meta, body))
    }

    // --- refs (the one mutable pointer) -------------------------------------

    pub fn set_ref(&self, id: &str, hash: &str) -> Result<()> {
        fs::write(self.ref_path(id), format!("{hash}\n"))
            .with_context(|| format!("writing ref {id}"))?;
        Ok(())
    }

    pub fn get_ref(&self, id: &str) -> Result<String> {
        let s = fs::read_to_string(self.ref_path(id))
            .with_context(|| format!("unknown node `{id}`"))?;
        Ok(s.trim().to_string())
    }

    pub fn list_refs(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.root.join("refs"))? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    // --- status (append-only event log) -------------------------------------

    /// Append one status event. Appending a fresh `[[event]]` block keeps the
    /// file valid TOML while only ever adding lines — a clean, minimal diff.
    pub fn append_status(&self, id: &str, event: &StatusEvent) -> Result<()> {
        let block = toml::to_string(&StatusLog {
            events: vec![event.clone()],
        })?;
        let path = self.status_path(id);
        // Separate blocks with a blank line for readability (none before the first).
        let separator = if path.exists() { "\n" } else { "" };
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening status log for {id}"))?;
        write!(f, "{separator}{block}")?;
        Ok(())
    }

    pub fn status_log(&self, id: &str) -> Result<StatusLog> {
        match fs::read_to_string(self.status_path(id)) {
            Ok(s) => Ok(toml::from_str(&s).context("parsing status log")?),
            Err(_) => Ok(StatusLog::default()),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}
