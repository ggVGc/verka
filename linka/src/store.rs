//! The on-disk store for the core graph namespace, plus workbench paths.
//!
//! Layout (all text, all diff-friendly, all meant to live in git):
//!
//! ```text
//! <root>/
//!   pairing.toml      which project repo this store describes (optional; see pairing)
//!   nodes/<id>/       core graph (this module)
//!     node.toml       structured definition metadata
//!     description.md  definition prose
//!     result.toml     structured completion record (optional)
//!     result.md       completion narrative (optional)
//!     work.jsonl      legacy execution log (read-only compatibility only)
//! ```
//!
//! Applications layered on top (execution harnesses, review tools) may keep
//! their own namespaces beside `nodes/`; this library neither reads nor
//! interprets them.
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
//!   .linka/       the store (<root> above)
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

use crate::model::{
    ContextObservation, DefinitionVersion, NodeId, NodeMeta, ResultMeta, ResultVersion,
};

/// Git's blob id for `bytes`, computed locally so version identity needs no
/// git invocation.
pub fn blob_id(bytes: &[u8]) -> String {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()).as_bytes());
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

/// The project directory inside a workbench, beside the store.
pub const PROJECT_DIR: &str = "project";

pub struct Store {
    root: PathBuf,
}

pub struct MutationLock {
    path: PathBuf,
    _file: fs::File,
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Store {
    pub fn mutation_lock(&self) -> Result<MutationLock> {
        let path = self.root.join("mutation.lock");
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("acquiring store mutation lock {}", path.display()))?;
        Ok(MutationLock { path, _file: file })
    }
    /// Open an existing store, erroring if it has not been initialised.
    pub fn open(root: PathBuf) -> Result<Self> {
        if !root.join("nodes").is_dir() {
            bail!(
                "no linka store at {} (run `linka init` first)",
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
        match id.parse::<NodeId>() {
            Ok(id) => self.root.join("nodes").join(id.as_str()),
            Err(_) => self.root.join("nodes").join(".invalid-node-id"),
        }
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

    pub fn exists(&self, id: &str) -> bool {
        self.node_path(id).is_file()
    }

    // --- definition ------------------------------------------------------------

    pub fn write_node(&self, id: &str, meta: &NodeMeta, description: &str) -> Result<()> {
        validate_node_id(id)?;
        fs::create_dir_all(self.node_dir(id))?;
        let data = toml::to_string_pretty(meta).context("serialising node metadata")?;
        fs::write(self.node_path(id), data).with_context(|| format!("writing node `{id}`"))?;
        fs::write(self.description_path(id), description)
            .with_context(|| format!("writing description for `{id}`"))?;
        Ok(())
    }

    pub fn read_node(&self, id: &str) -> Result<(NodeMeta, String)> {
        validate_node_id(id)?;
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
        validate_node_id(id)?;
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
        validate_node_id(id)?;
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
        validate_node_id(id)?;
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
        validate_node_id(id)?;
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

    // --- legacy compatibility and immutable observations -------------------------

    fn legacy_work_log_path(&self, id: &str) -> PathBuf {
        self.node_dir(id).join("work.jsonl")
    }

    /// The recorded interaction log of the node's work sessions, or `None` if
    /// none has been recorded. Opaque to every derived query: it is the *story*
    /// of the work (what the worker said and did, one JSON event per line),
    /// never state — status, staleness, and readiness must not read it. It does
    /// not participate in the node's version.
    #[deprecated(note = "compatibility reader only; execution logs belong to the coordinator")]
    pub fn read_work_log(&self, id: &str) -> Result<Option<String>> {
        match fs::read_to_string(self.legacy_work_log_path(id)) {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading work log for `{id}`")),
        }
    }

    pub fn write_context_observation(
        &self,
        id: &str,
        observation: &ContextObservation,
    ) -> Result<()> {
        validate_node_id(id)?;
        let data =
            toml::to_string_pretty(observation).context("serialising context observation")?;
        let identity = blob_id(data.as_bytes());
        let dir = self.node_dir(id).join("observations");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{identity}.toml"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(data.as_bytes())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).with_context(|| format!("writing {}", path.display())),
        }
        Ok(())
    }

    pub fn read_context_observations(&self, id: &str) -> Result<Vec<ContextObservation>> {
        validate_node_id(id)?;
        let dir = self.node_dir(id).join("observations");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("reading {}", dir.display())),
        };
        let mut observations = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let data = fs::read_to_string(&path)?;
            observations.push(
                toml::from_str(&data).with_context(|| format!("parsing {}", path.display()))?,
            );
        }
        Ok(observations)
    }

    pub fn replace_context_observations(
        &self,
        id: &str,
        observations: &[ContextObservation],
    ) -> Result<()> {
        let dir = self.node_dir(id).join("observations");
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        for observation in observations {
            self.write_context_observation(id, observation)?;
        }
        Ok(())
    }

    // --- listing -----------------------------------------------------------------

    pub fn list_ids(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.root.join("nodes"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("node directory name is not UTF-8"))?;
                validate_node_id(&name)
                    .with_context(|| format!("invalid node directory `{name}`"))?;
                ids.push(name);
            }
        }
        ids.sort();
        Ok(ids)
    }

    // --- git integration points ----------------------------------------------------

    /// The workbench root: the directory containing the store (e.g. the parent
    /// of `.linka/`). Its git repository holds the store's history.
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
    /// pathspec when committing store changes (e.g. `.linka`).
    pub fn store_name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }
}

fn validate_node_id(id: &str) -> Result<NodeId> {
    id.parse().map_err(anyhow::Error::msg)
}

/// The blob id of a file on disk, or `None` only when it is proven absent.
pub fn file_blob(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(blob_id(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading context {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtifactRef, Author, Outcome};

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
        let dir = std::env::temp_dir().join(format!("linka-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".linka")).unwrap();

        let meta = NodeMeta {
            schema: 1,
            author: Author::Human,
            assignee: None,
            depends_on: vec!["node-a".parse().unwrap()],
            derived_from: vec![],
            extensions: Default::default(),
        };
        store
            .write_node("node-1", &meta, "hello\n\nthe details")
            .unwrap();
        let (got, description) = store.read_node("node-1").unwrap();
        assert_eq!(got.depends_on, vec!["node-a".parse().unwrap()]);
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
            schema: 1,
            at: 0,
            author: Author::Machine,
            definition: v2.clone(),
            outcome: Outcome::Done,
            project: None,
            consumed: vec![],
            context: vec![],
            output: Some(ArtifactRef {
                scheme: "git-commit".into(),
                repository: String::new(),
                id: "abc".into(),
            }),
            producer: None,
        };
        store
            .write_result("node-1", &result, "did the thing")
            .unwrap();
        let (r, notes) = store.read_result("node-1").unwrap().unwrap();
        assert_eq!(r.output.as_ref().map(|a| a.id.as_str()), Some("abc"));
        assert_eq!(r.outcome, Outcome::Done);
        assert_eq!(notes, "did the thing");
        assert_eq!(store.node_version("node-1").unwrap(), v2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[allow(deprecated)]
    fn legacy_work_log_has_a_read_only_compatibility_reader() {
        let dir = std::env::temp_dir().join(format!("linka-worklog-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".linka")).unwrap();

        let meta = NodeMeta {
            schema: 1,
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
            extensions: Default::default(),
        };
        store.write_node("node-1", &meta, "").unwrap();
        assert_eq!(store.read_work_log("node-1").unwrap(), None);

        // A pre-migration log remains readable but Linka exposes no writer.
        let v = store.node_version("node-1").unwrap();
        std::fs::write(
            store.node_dir("node-1").join("work.jsonl"),
            "{\"event\":\"a\"}\n",
        )
        .unwrap();
        assert_eq!(
            store.read_work_log("node-1").unwrap().unwrap(),
            "{\"event\":\"a\"}\n"
        );
        assert_eq!(store.node_version("node-1").unwrap(), v);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_node_directory_is_rejected_during_discovery() {
        let dir =
            std::env::temp_dir().join(format!("linka-invalid-id-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".linka")).unwrap();
        std::fs::create_dir_all(store.root().join("nodes/.git")).unwrap();
        assert!(store.list_ids().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_lays_out_the_workbench() {
        let dir = std::env::temp_dir().join(format!("linka-layout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".linka")).unwrap();

        // Store and project sit side by side under the workbench root.
        assert_eq!(store.workbench_root(), dir);
        assert_eq!(store.project_root(), dir.join(PROJECT_DIR));
        assert!(dir.join(".linka/nodes").is_dir());
        assert!(store.project_root().is_dir());
        assert_eq!(store.store_name(), ".linka");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
