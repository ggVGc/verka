//! The on-disk store.
//!
//! Layout (all text, all diff-friendly, all meant to live in git):
//!
//! ```text
//! <root>/
//!   nodes/<id>/
//!     node.md      frontmatter (TOML) + prose — the definition
//!     result.md    frontmatter (TOML) + narrative — the completion record
//!     work.jsonl   recorded interaction log of work sessions (optional)
//! ```
//!
//! There is no object store, no refs, and no status log: git is the only
//! versioning layer. A node's *version* is the git blob id of its `node.md`,
//! computed on demand ([`Store::node_version`]) and never stored inside the
//! file. History is `git log` on the node's directory.

use anyhow::{bail, Context, Result};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{NodeMeta, ResultMeta};

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open an existing store, erroring if it has not been initialised.
    pub fn open(root: PathBuf) -> Result<Self> {
        if !root.join("nodes").is_dir() {
            bail!(
                "no llaundry store at {} (run `llaundry init` first)",
                root.display()
            );
        }
        Ok(Store { root })
    }

    /// Create the directory skeleton for a new store.
    pub fn init(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("nodes"))
            .with_context(|| format!("creating {}/nodes", root.display()))?;
        Ok(Store { root })
    }

    // --- paths ----------------------------------------------------------------

    pub fn node_dir(&self, id: &str) -> PathBuf {
        self.root.join("nodes").join(id)
    }
    fn node_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("node.md")
    }
    fn result_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("result.md")
    }

    pub fn exists(&self, id: &str) -> bool {
        self.node_path(id).is_file()
    }

    // --- node.md (the definition) ----------------------------------------------

    pub fn write_node(&self, id: &str, meta: &NodeMeta, body: &str) -> Result<()> {
        fs::create_dir_all(self.node_dir(id))?;
        let doc = to_document(&toml::to_string_pretty(meta).context("serialising node meta")?, body);
        fs::write(self.node_path(id), doc).with_context(|| format!("writing node `{id}`"))?;
        Ok(())
    }

    pub fn read_node(&self, id: &str) -> Result<(NodeMeta, String)> {
        let text = fs::read_to_string(self.node_path(id))
            .with_context(|| format!("unknown node `{id}`"))?;
        let (front, body) = split_document(&text)
            .with_context(|| format!("malformed node.md for `{id}`"))?;
        let meta = toml::from_str(&front).with_context(|| format!("parsing node.md for `{id}`"))?;
        Ok((meta, body))
    }

    /// The node's version: the git blob id of its `node.md` bytes, computed on
    /// demand. Editing the definition changes it; writing `result.md` does not.
    pub fn node_version(&self, id: &str) -> Result<String> {
        let bytes =
            fs::read(self.node_path(id)).with_context(|| format!("unknown node `{id}`"))?;
        Ok(blob_id(&bytes))
    }

    // --- result.md (the completion record) --------------------------------------

    pub fn write_result(&self, id: &str, meta: &ResultMeta, notes: &str) -> Result<()> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        let doc =
            to_document(&toml::to_string_pretty(meta).context("serialising result meta")?, notes);
        fs::write(self.result_path(id), doc).with_context(|| format!("writing result for `{id}`"))?;
        Ok(())
    }

    /// The node's completion record, or `None` if it has not been worked yet.
    pub fn read_result(&self, id: &str) -> Result<Option<(ResultMeta, String)>> {
        let text = match fs::read_to_string(self.result_path(id)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading result for `{id}`")),
        };
        let (front, notes) = split_document(&text)
            .with_context(|| format!("malformed result.md for `{id}`"))?;
        let meta =
            toml::from_str(&front).with_context(|| format!("parsing result.md for `{id}`"))?;
        Ok(Some((meta, notes)))
    }

    // --- work.jsonl (the interaction log) ----------------------------------------

    fn work_log_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("work.jsonl")
    }

    /// The recorded interaction log of the node's work sessions, or `None` if
    /// none has been recorded. Opaque to every derived query: it is the *story*
    /// of the work (what the worker said and did, one JSON event per line),
    /// never state — status, staleness, and readiness must not read it. It does
    /// not participate in the node's version.
    pub fn read_work_log(&self, id: &str) -> Result<Option<String>> {
        match fs::read_to_string(self.work_log_path(id)) {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading work log for `{id}`")),
        }
    }

    /// Open the node's interaction log for streaming. `append` extends the log
    /// (a continuation of a paused unit of work); `!append` starts it over
    /// (rework — git history keeps the previous story, like an overwritten
    /// `result.md`). The caller writes events line by line, flushing each, so
    /// an abrupt exit at any point loses at most an unflushed tail.
    pub fn open_work_log(&self, id: &str, append: bool) -> Result<fs::File> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        let mut opts = fs::OpenOptions::new();
        opts.create(true);
        if append {
            opts.append(true);
        } else {
            opts.write(true).truncate(true);
        }
        opts.open(self.work_log_path(id))
            .with_context(|| format!("opening work log for `{id}`"))
    }

    /// Whether a repository-relative path is a node's work log — the one file
    /// the clean-tree rule tolerates dirty, because the log is streamed during
    /// a session and is opaque non-state (a commit sweeping half a story is
    /// still a true story-so-far).
    pub fn is_work_log_path(&self, path: &str) -> bool {
        let Some(rest) = path
            .strip_prefix(&self.store_name())
            .and_then(|p| p.strip_prefix("/nodes/"))
        else {
            return false;
        };
        matches!(rest.split_once('/'), Some((id, "work.jsonl")) if !id.is_empty())
    }

    // --- listing -----------------------------------------------------------------

    pub fn list_ids(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.root.join("nodes"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    // --- git integration points ----------------------------------------------------

    /// The project root that git operations and file paths resolve against: the
    /// directory containing the store (e.g. the parent of `.llaundry/`).
    pub fn project_root(&self) -> PathBuf {
        match self.root.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        }
    }

    /// The store directory relative to the project root, for use as a git
    /// pathspec when committing store changes (e.g. `.llaundry`).
    pub fn store_name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }
}

/// Git's blob id for `bytes`: `sha1("blob <len>\0" + bytes)`. Computed locally so
/// pins and node versions need no git invocation (and no repository) to check.
pub fn blob_id(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    hex(&h.finalize())
}

/// The blob id of a file on disk, or `None` if it does not exist.
pub fn file_blob(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| blob_id(&bytes))
}

// --- the `---` frontmatter document format ---------------------------------------

/// `---\n<toml>---\n\n<body>`. The body is omitted entirely when empty.
fn to_document(front: &str, body: &str) -> String {
    let front = if front.ends_with('\n') || front.is_empty() {
        front.to_string()
    } else {
        format!("{front}\n")
    };
    if body.is_empty() {
        format!("---\n{front}---\n")
    } else {
        format!("---\n{front}---\n\n{body}")
    }
}

/// Split a document into its TOML frontmatter and body. The frontmatter ends at
/// the first line that is exactly `---`.
fn split_document(text: &str) -> Result<(String, String)> {
    let rest = text
        .strip_prefix("---\n")
        .context("missing opening `---` frontmatter delimiter")?;
    let mut front = String::new();
    let mut consumed = 4; // the opening "---\n"
    let mut closed = false;
    for line in rest.split_inclusive('\n') {
        consumed += line.len();
        if line.trim_end_matches(['\r', '\n']) == "---" {
            closed = true;
            break;
        }
        front.push_str(line);
    }
    if !closed {
        bail!("missing closing `---` frontmatter delimiter");
    }
    let body = text[consumed..].strip_prefix('\n').unwrap_or(&text[consumed..]);
    Ok((front, body.to_string()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Author, Outcome};

    #[test]
    fn blob_id_matches_git() {
        // `echo 'hello' | git hash-object --stdin`
        assert_eq!(blob_id(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
        // `printf '' | git hash-object --stdin`
        assert_eq!(blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn document_roundtrip() {
        let (front, body) = split_document(&to_document("a = 1\n", "body\nlines\n")).unwrap();
        assert_eq!(front, "a = 1\n");
        assert_eq!(body, "body\nlines\n");

        // An empty body round-trips to empty.
        let (front, body) = split_document(&to_document("a = 1\n", "")).unwrap();
        assert_eq!(front, "a = 1\n");
        assert_eq!(body, "");

        // A body containing `---` lines is not mistaken for a delimiter, because
        // the split stops at the *first* closing delimiter.
        let (_, body) = split_document(&to_document("a = 1\n", "x\n---\ny\n")).unwrap();
        assert_eq!(body, "x\n---\ny\n");
    }

    #[test]
    fn node_and_result_roundtrip() {
        let dir = std::env::temp_dir().join(format!("llaundry-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".llaundry")).unwrap();

        let meta = NodeMeta {
            schema: 1,
            title: "hello".into(),
            author: Author::Human,
            assignee: None,
            depends_on: vec!["node-a".into()],
            derived_from: vec![],
        };
        store.write_node("node-1", &meta, "the body").unwrap();
        let (got, body) = store.read_node("node-1").unwrap();
        assert_eq!(got.title, "hello");
        assert_eq!(got.depends_on, vec!["node-a".to_string()]);
        assert_eq!(body, "the body");

        // The version changes exactly when the definition changes.
        let v1 = store.node_version("node-1").unwrap();
        store.write_node("node-1", &meta, "other body").unwrap();
        assert_ne!(v1, store.node_version("node-1").unwrap());

        // No result yet; then one round-trips, without touching the version.
        assert!(store.read_result("node-1").unwrap().is_none());
        let v2 = store.node_version("node-1").unwrap();
        let result = ResultMeta {
            at: 0,
            author: Author::Machine,
            node_version: v2.clone(),
            outcome: Outcome::Done,
            output_commit: Some("abc".into()),
            built_against: vec![],
            context: vec![],
        };
        store.write_result("node-1", &result, "did the thing").unwrap();
        let (r, notes) = store.read_result("node-1").unwrap().unwrap();
        assert_eq!(r.output_commit.as_deref(), Some("abc"));
        assert_eq!(r.outcome, Outcome::Done);
        assert_eq!(notes, "did the thing");
        assert_eq!(store.node_version("node-1").unwrap(), v2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_log_appends_and_starts_over() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("llaundry-worklog-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".llaundry")).unwrap();

        // Only existing nodes can carry a log.
        assert!(store.open_work_log("node-nope", true).is_err());

        let meta = NodeMeta {
            schema: 1,
            title: "hello".into(),
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
        };
        store.write_node("node-1", &meta, "").unwrap();
        assert_eq!(store.read_work_log("node-1").unwrap(), None);

        // Streamed writes accumulate across sessions and leave the version alone.
        let v = store.node_version("node-1").unwrap();
        writeln!(store.open_work_log("node-1", true).unwrap(), r#"{{"event":"a"}}"#).unwrap();
        writeln!(store.open_work_log("node-1", true).unwrap(), r#"{{"event":"b"}}"#).unwrap();
        assert_eq!(
            store.read_work_log("node-1").unwrap().unwrap(),
            "{\"event\":\"a\"}\n{\"event\":\"b\"}\n"
        );
        assert_eq!(store.node_version("node-1").unwrap(), v);

        // Rework starts the log over.
        writeln!(store.open_work_log("node-1", false).unwrap(), r#"{{"event":"c"}}"#).unwrap();
        assert_eq!(store.read_work_log("node-1").unwrap().unwrap(), "{\"event\":\"c\"}\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn work_log_paths_are_recognised() {
        let store = Store {
            root: PathBuf::from(".llaundry"),
        };
        assert!(store.is_work_log_path(".llaundry/nodes/node-1/work.jsonl"));
        assert!(!store.is_work_log_path(".llaundry/nodes/node-1/node.md"));
        assert!(!store.is_work_log_path(".llaundry/nodes/node-1/result.md"));
        assert!(!store.is_work_log_path("src/nodes/x/work.jsonl"));
        assert!(!store.is_work_log_path(".llaundry/nodes//work.jsonl"));
        assert!(!store.is_work_log_path(".llaundry/nodes/node-1/sub/work.jsonl"));
        assert!(!store.is_work_log_path("work.jsonl"));
    }
}
