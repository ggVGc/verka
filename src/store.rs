//! The on-disk store.
//!
//! Layout (all text, all diff-friendly, all meant to live in git):
//!
//! ```text
//! <root>/
//!   pairing.toml      which project repo this store describes (optional; see pairing)
//!   nodes/<id>/
//!     node.toml       structured definition metadata
//!     description.md  definition prose
//!     result.toml     structured completion record (optional)
//!     result.md       completion narrative (optional)
//!     work.jsonl   recorded interaction log of work sessions (optional)
//!   attempts/<id>/
//!     attempt.toml  durable execution identity and frozen inputs
//!     work.jsonl    attempt transcript
//!     result.toml   attempt-scoped outcome (optional)
//!     result.md     attempt narrative (optional)
//!     final.toml    sealed backend-exit evidence (optional)
//! ```
//!
//! There is no object store, no refs, and no status log: git is the only
//! versioning layer. A node's version is the pair of Git blob ids for
//! `node.toml` and `description.md`, computed on demand.
//!
//! The store lives in a *workbench*: an outer directory (its own git repo)
//! holding the store next to the project, which is a completely ordinary,
//! separate git repository (see ISOLATION.md):
//!
//! ```text
//! <workbench>/       outer repo — store history
//!   .llaundry/       the store (<root> above)
//!   project/         inner repo — the actual project
//! ```
//!
//! Work sessions run inside `project/` with file tools scoped to it; the
//! store sits above the granted subtree, so a node's context stays what the
//! graph says it is without any deny rules.

use anyhow::{bail, Context, Result};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{AttemptFinal, AttemptMeta, DefinitionVersion, NodeMeta, PublicationIntent, ResultMeta, ResultVersion};

/// The project directory inside a workbench, beside the store.
pub const PROJECT_DIR: &str = "project";

pub struct Store {
    root: PathBuf,
}

pub struct ExecutionLock {
    path: PathBuf,
}

impl Drop for ExecutionLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
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

    /// Create the directory skeleton for a new store, including the project
    /// directory beside it (the workbench layout).
    pub fn init(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("nodes"))
            .with_context(|| format!("creating {}/nodes", root.display()))?;
        let store = Store { root };
        let project = store.project_root();
        fs::create_dir_all(&project).with_context(|| format!("creating {}", project.display()))?;
        Ok(store)
    }

    // --- paths ----------------------------------------------------------------

    /// The store's root directory (holds `nodes/`, `config.toml`, `pairing.toml`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn node_dir(&self, id: &str) -> PathBuf {
        self.root.join("nodes").join(id)
    }
    fn node_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("node.toml")
    }
    fn description_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("description.md")
    }
    fn result_meta_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("result.toml")
    }
    fn result_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("result.md")
    }

    pub fn attempt_dir(&self, id: &str) -> PathBuf {
        self.root.join("attempts").join(id)
    }

    fn attempt_meta_path(&self, id: &str) -> PathBuf {
        self.attempt_dir(id).join("attempt.toml")
    }

    fn attempt_result_meta_path(&self, id: &str) -> PathBuf {
        self.attempt_dir(id).join("result.toml")
    }

    fn attempt_result_path(&self, id: &str) -> PathBuf {
        self.attempt_dir(id).join("result.md")
    }

    fn attempt_final_path(&self, id: &str) -> PathBuf {
        self.attempt_dir(id).join("final.toml")
    }

    fn attempt_log_path(&self, id: &str) -> PathBuf {
        self.attempt_dir(id).join("work.jsonl")
    }

    fn publication_path(&self, review: &str) -> PathBuf {
        self.root.join("publications").join(review).join("publication.toml")
    }

    pub fn exists(&self, id: &str) -> bool {
        self.node_path(id).is_file()
    }

    // --- definition ------------------------------------------------------------

    pub fn write_node(&self, id: &str, meta: &NodeMeta, description: &str) -> Result<()> {
        fs::create_dir_all(self.node_dir(id))?;
        let data = toml::to_string_pretty(meta).context("serialising node metadata")?;
        fs::write(self.node_path(id), data).with_context(|| format!("writing node `{id}`"))?;
        fs::write(self.description_path(id), description)
            .with_context(|| format!("writing description for `{id}`"))?;
        Ok(())
    }

    pub fn read_node(&self, id: &str) -> Result<(NodeMeta, String)> {
        let data = fs::read_to_string(self.node_path(id))
            .with_context(|| format!("unknown node `{id}`"))?;
        let meta =
            toml::from_str(&data).with_context(|| format!("parsing node.toml for `{id}`"))?;
        let description = fs::read_to_string(self.description_path(id))
            .with_context(|| format!("reading description.md for `{id}`"))?;
        Ok((meta, description))
    }

    /// The node's version: Git blob ids of its structured metadata and prose.
    pub fn node_version(&self, id: &str) -> Result<DefinitionVersion> {
        let metadata =
            fs::read(self.node_path(id)).with_context(|| format!("unknown node `{id}`"))?;
        let description = fs::read(self.description_path(id))
            .with_context(|| format!("reading description.md for `{id}`"))?;
        Ok(DefinitionVersion {
            metadata: blob_id(&metadata),
            description: blob_id(&description),
        })
    }

    // --- result (structured record plus optional prose) -------------------------

    pub fn write_result(&self, id: &str, meta: &ResultMeta, notes: &str) -> Result<()> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        let data = toml::to_string_pretty(meta).context("serialising result metadata")?;
        fs::write(self.result_meta_path(id), data)
            .with_context(|| format!("writing result metadata for `{id}`"))?;
        if notes.is_empty() {
            match fs::remove_file(self.result_path(id)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("removing empty result notes for `{id}`"))
                }
            }
        } else {
            fs::write(self.result_path(id), notes)
                .with_context(|| format!("writing result notes for `{id}`"))?;
        }
        Ok(())
    }

    /// The node's completion record, or `None` if it has not been worked yet.
    pub fn read_result(&self, id: &str) -> Result<Option<(ResultMeta, String)>> {
        let data = match fs::read_to_string(self.result_meta_path(id)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if self.result_path(id).exists() {
                    bail!("result.md exists without result.toml for `{id}`");
                }
                return Ok(None);
            }
            Err(e) => return Err(e).with_context(|| format!("reading result for `{id}`")),
        };
        let meta =
            toml::from_str(&data).with_context(|| format!("parsing result.toml for `{id}`"))?;
        let notes = match fs::read_to_string(self.result_path(id)) {
            Ok(notes) => notes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("reading result.md for `{id}`")),
        };
        Ok(Some((meta, notes)))
    }

    pub fn result_version(&self, id: &str) -> Result<ResultVersion> {
        let metadata = fs::read(self.result_meta_path(id))
            .with_context(|| format!("node `{id}` has no result"))?;
        let notes = match fs::read(self.result_path(id)) {
            Ok(bytes) => Some(blob_id(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("reading result.md for `{id}`")),
        };
        Ok(ResultVersion {
            metadata: blob_id(&metadata),
            notes,
        })
    }

    pub fn write_attempt(&self, meta: &AttemptMeta) -> Result<()> {
        fs::create_dir_all(self.attempt_dir(&meta.id))?;
        let data = toml::to_string_pretty(meta).context("serialising attempt metadata")?;
        fs::write(self.attempt_meta_path(&meta.id), data)
            .with_context(|| format!("writing attempt `{}`", meta.id))
    }

    pub fn read_attempt(&self, id: &str) -> Result<AttemptMeta> {
        let data = fs::read_to_string(self.attempt_meta_path(id))
            .with_context(|| format!("unknown attempt `{id}`"))?;
        toml::from_str(&data).with_context(|| format!("parsing attempt `{id}`"))
    }

    pub fn list_attempt_ids(&self) -> Result<Vec<String>> {
        let dir = self.root.join("attempts");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Serialize the short authorization-and-recording phase for one logical
    /// node. The directory creation is atomic across processes.
    pub fn lock_execution(&self, node: &str) -> Result<ExecutionLock> {
        let name = blob_id(node.as_bytes());
        let path = self.root.join("locks/executions").join(name);
        fs::create_dir_all(path.parent().expect("lock has parent"))?;
        match fs::create_dir(&path) {
            Ok(()) => Ok(ExecutionLock { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("another process is starting work on node `{node}`")
            }
            Err(e) => Err(e).with_context(|| format!("locking execution of `{node}`")),
        }
    }

    pub fn write_attempt_result(&self, id: &str, meta: &ResultMeta, notes: &str) -> Result<()> {
        self.read_attempt(id)?;
        let data = toml::to_string_pretty(meta).context("serialising attempt result")?;
        fs::write(self.attempt_result_meta_path(id), data)?;
        if notes.is_empty() {
            let _ = fs::remove_file(self.attempt_result_path(id));
        } else {
            fs::write(self.attempt_result_path(id), notes)?;
        }
        Ok(())
    }

    pub fn read_attempt_result(&self, id: &str) -> Result<Option<(ResultMeta, String)>> {
        let data = match fs::read_to_string(self.attempt_result_meta_path(id)) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let meta = toml::from_str(&data).with_context(|| format!("parsing result for attempt `{id}`"))?;
        let notes = fs::read_to_string(self.attempt_result_path(id)).unwrap_or_default();
        Ok(Some((meta, notes)))
    }

    pub fn attempt_result_version(&self, id: &str) -> Result<ResultVersion> {
        let metadata = fs::read(self.attempt_result_meta_path(id))?;
        let notes = fs::read(self.attempt_result_path(id)).ok().map(|bytes| blob_id(&bytes));
        Ok(ResultVersion { metadata: blob_id(&metadata), notes })
    }

    pub fn write_attempt_final(&self, id: &str, final_meta: &AttemptFinal) -> Result<()> {
        self.read_attempt(id)?;
        fs::write(self.attempt_final_path(id), toml::to_string_pretty(final_meta)?)?;
        Ok(())
    }

    pub fn read_attempt_final(&self, id: &str) -> Result<Option<AttemptFinal>> {
        let data = match fs::read_to_string(self.attempt_final_path(id)) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(Some(toml::from_str(&data)?))
    }

    pub fn open_attempt_log(&self, id: &str, append: bool) -> Result<std::fs::File> {
        self.read_attempt(id)?;
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(self.attempt_log_path(id))
            .with_context(|| format!("opening work log for attempt `{id}`"))
    }

    pub fn read_attempt_log(&self, id: &str) -> Result<Option<String>> {
        match fs::read_to_string(self.attempt_log_path(id)) {
            Ok(log) => Ok(Some(log)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn write_publication(&self, publication: &PublicationIntent) -> Result<()> {
        let path = self.publication_path(&publication.review);
        fs::create_dir_all(path.parent().expect("publication has parent"))?;
        fs::write(&path, toml::to_string_pretty(publication)?)
            .with_context(|| format!("writing publication for review `{}`", publication.review))
    }

    pub fn read_publication(&self, review: &str) -> Result<Option<PublicationIntent>> {
        let path = self.publication_path(review);
        let data = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Some(toml::from_str(&data)
            .with_context(|| format!("parsing publication for review `{review}`"))?))
    }

    pub fn list_publication_ids(&self) -> Result<Vec<String>> {
        let dir = self.root.join("publications");
        if !dir.exists() { return Ok(Vec::new()); }
        let mut ids = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        Ok(ids)
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

    /// The workbench root: the directory containing the store (e.g. the parent
    /// of `.llaundry/`). Its git repository holds the store's history.
    pub fn workbench_root(&self) -> PathBuf {
        match self.root.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        }
    }

    /// The project root that output commits and pinned file paths resolve
    /// against: the `project/` directory inside the workbench — an ordinary
    /// git repository of its own, entirely separate from the store's.
    pub fn project_root(&self) -> PathBuf {
        self.workbench_root().join(PROJECT_DIR)
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
        assert_eq!(
            blob_id(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        // `printf '' | git hash-object --stdin`
        assert_eq!(blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn node_and_result_roundtrip() {
        let dir = std::env::temp_dir().join(format!("llaundry-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".llaundry")).unwrap();

        let meta = NodeMeta {
            schema: 1,
            author: Author::Human,
            assignee: None,
            depends_on: vec!["node-a".into()],
            derived_from: vec![],
            review: None,
        };
        store
            .write_node("node-1", &meta, "hello\n\nthe details")
            .unwrap();
        let (got, description) = store.read_node("node-1").unwrap();
        assert_eq!(got.depends_on, vec!["node-a".to_string()]);
        assert_eq!(description, "hello\n\nthe details");
        assert_eq!(crate::model::title_of(&description), "hello");

        // The version changes exactly when the definition changes.
        let v1 = store.node_version("node-1").unwrap();
        store
            .write_node("node-1", &meta, "other description")
            .unwrap();
        assert_ne!(v1, store.node_version("node-1").unwrap());

        // No result yet; then one round-trips, without touching the version.
        assert!(store.read_result("node-1").unwrap().is_none());
        let v2 = store.node_version("node-1").unwrap();
        let result = ResultMeta {
            at: 0,
            author: Author::Machine,
            definition: v2.clone(),
            outcome: Outcome::Done,
            publication_pending: false,
            input_commit: None,
            input_tree: None,
            attempt_id: None,
            candidate_branch: None,
            review_decision: None,
            suggestion_branch: None,
            suggestion_commit: None,
            output_commit: Some("abc".into()),
            integrated_commit: None,
            target_ref: None,
            target_previous: None,
            worked_by: None,
            built_against: vec![],
            context: vec![],
        };
        store
            .write_result("node-1", &result, "did the thing")
            .unwrap();
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
        let dir =
            std::env::temp_dir().join(format!("llaundry-worklog-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".llaundry")).unwrap();

        // Only existing nodes can carry a log.
        assert!(store.open_work_log("node-nope", true).is_err());

        let meta = NodeMeta {
            schema: 1,
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
            review: None,
        };
        store.write_node("node-1", &meta, "").unwrap();
        assert_eq!(store.read_work_log("node-1").unwrap(), None);

        // Streamed writes accumulate across sessions and leave the version alone.
        let v = store.node_version("node-1").unwrap();
        writeln!(
            store.open_work_log("node-1", true).unwrap(),
            r#"{{"event":"a"}}"#
        )
        .unwrap();
        writeln!(
            store.open_work_log("node-1", true).unwrap(),
            r#"{{"event":"b"}}"#
        )
        .unwrap();
        assert_eq!(
            store.read_work_log("node-1").unwrap().unwrap(),
            "{\"event\":\"a\"}\n{\"event\":\"b\"}\n"
        );
        assert_eq!(store.node_version("node-1").unwrap(), v);

        // Rework starts the log over.
        writeln!(
            store.open_work_log("node-1", false).unwrap(),
            r#"{{"event":"c"}}"#
        )
        .unwrap();
        assert_eq!(
            store.read_work_log("node-1").unwrap().unwrap(),
            "{\"event\":\"c\"}\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_lays_out_the_workbench() {
        let dir = std::env::temp_dir().join(format!("llaundry-layout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".llaundry")).unwrap();

        // Store and project sit side by side under the workbench root.
        assert_eq!(store.workbench_root(), dir);
        assert_eq!(store.project_root(), dir.join(PROJECT_DIR));
        assert!(dir.join(".llaundry/nodes").is_dir());
        assert!(store.project_root().is_dir());
        assert_eq!(store.store_name(), ".llaundry");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
