//! The on-disk store for the core graph namespace, plus workbench paths.
//!
//! Layout (inspectable metadata plus opaque attachment payloads, all meant to
//! live in git):
//!
//! ```text
//! <root>/
//!   pairing.toml      which project repo this store describes (optional; see pairing)
//!   nodes/<id>/       core graph (this module)
//!     node.toml       structured definition metadata
//!     description.md  definition prose
//!     result.toml     structured completion record (optional)
//!     result.md       completion narrative (optional)
//!     attachments/    opaque, node-associated data (never graph state)
//!   candidates/<id>/  output proposal attached to an exact node result
//!     candidate.toml  candidate facts and accept/reject state
//! ```
//!
//! There is no object store or mutable status log: git is the versioning layer.
//! Candidate records pin project refs but never store their contents. A node's
//! version is the pair of Git blob ids for
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
    ContextObservation, DefinitionVersion, NodeAttachment, NodeId, NodeMeta, ResultMeta,
    ResultVersion, ATTACHMENT_SCHEMA, DEFINITION_SCHEMA, OBSERVATION_SCHEMA, RESULT_SCHEMA,
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

/// A definition read once from disk, together with versions of those exact
/// bytes. Callers that need both content and pins should use this rather than
/// coordinating `read_node` and `node_version`.
#[derive(Debug, Clone)]
pub struct LoadedDefinition {
    pub meta: NodeMeta,
    pub description: String,
    pub version: DefinitionVersion,
}

/// A result read once from disk, together with versions of those exact bytes.
/// `None` means both result files are absent; all partial or malformed records
/// are errors.
#[derive(Debug, Clone)]
pub struct LoadedResult {
    pub meta: ResultMeta,
    pub notes: String,
    pub version: ResultVersion,
}

pub struct MutationLock {
    _file: fs::File,
    path: String,
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        // Closing the file also releases the lock, but unlock explicitly so a
        // following mutation in the same process can proceed immediately.
        let _ = self._file.unlock();
    }
}

impl MutationLock {
    /// Commit the one action performed while this lock was held, verify that
    /// the store is clean again, and then release the lock on return.
    pub fn commit(self, vcs: &dyn crate::vcs::Vcs, message: &str) -> Result<()> {
        vcs.commit_store(&self.path, message)?;
        vcs.require_clean_store(&self.path)
            .context("Linka store is still dirty after committing the mutation")?;
        Ok(())
    }
}

impl Store {
    /// Acquire the workbench-wide mutation lock and require the tracked store
    /// to be clean before returning it. The stable lock file lives inside
    /// `.git`, so it is never part of a store commit; the OS lock is released
    /// automatically on drop, including after a crash.
    pub fn mutation_lock(&self, vcs: &dyn crate::vcs::Vcs) -> Result<MutationLock> {
        let git_dir = self.workbench_root().join(".git");
        fs::create_dir_all(&git_dir).with_context(|| format!("creating {}", git_dir.display()))?;
        let lock_path = git_dir.join("linka-mutation.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("opening store mutation lock {}", lock_path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                bail!(
                    "another Linka store mutation is in progress ({})",
                    lock_path.display()
                )
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| {
                    format!("acquiring store mutation lock {}", lock_path.display())
                })
            }
        }
        let path = self.store_name();
        vcs.require_clean_store(&path)
            .context("Linka store must be clean before mutating")?;
        Ok(MutationLock { _file: file, path })
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

    pub fn node_dir(&self, id: &NodeId) -> PathBuf {
        self.root.join("nodes").join(id.as_str())
    }
    fn node_path(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("node.toml")
    }
    fn description_path(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("description.md")
    }
    fn result_meta_path(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("result.toml")
    }
    fn result_path(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("result.md")
    }

    fn attachments_path(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("attachments")
    }

    fn attachment_path(&self, id: &NodeId, namespace: &str, key: &str) -> Result<PathBuf> {
        validate_attachment_identity(namespace, key)?;
        let identity = attachment_identity(namespace, key);
        Ok(self.attachments_path(id).join(identity))
    }

    pub fn exists(&self, id: &NodeId) -> bool {
        self.node_path(id).is_file()
    }

    // --- definition ------------------------------------------------------------

    pub fn write_node(&self, id: &NodeId, meta: &NodeMeta, description: &str) -> Result<()> {
        if meta.schema != DEFINITION_SCHEMA {
            bail!("cannot write unsupported definition schema {}", meta.schema);
        }
        fs::create_dir_all(self.node_dir(id))?;
        let data = toml::to_string_pretty(meta).context("serialising node metadata")?;
        fs::write(self.node_path(id), data).with_context(|| format!("writing node `{id}`"))?;
        fs::write(self.description_path(id), description)
            .with_context(|| format!("writing description for `{id}`"))?;
        Ok(())
    }

    pub fn load_definition(&self, id: &NodeId) -> Result<LoadedDefinition> {
        let data = fs::read(self.node_path(id)).with_context(|| format!("unknown node `{id}`"))?;
        let text =
            std::str::from_utf8(&data).with_context(|| format!("reading node.toml for `{id}`"))?;
        let meta: NodeMeta =
            toml::from_str(text).with_context(|| format!("parsing node.toml for `{id}`"))?;
        if meta.schema != DEFINITION_SCHEMA {
            bail!("node `{id}` uses unsupported schema {}", meta.schema);
        }
        let description_bytes = fs::read(self.description_path(id))
            .with_context(|| format!("reading description.md for `{id}`"))?;
        let description = String::from_utf8(description_bytes.clone())
            .with_context(|| format!("reading description.md for `{id}`"))?;
        Ok(LoadedDefinition {
            meta,
            description,
            version: DefinitionVersion {
                metadata: blob_id(&data),
                description: blob_id(&description_bytes),
            },
        })
    }

    pub fn read_node(&self, id: &NodeId) -> Result<(NodeMeta, String)> {
        let loaded = self.load_definition(id)?;
        Ok((loaded.meta, loaded.description))
    }

    /// The node's version: Git blob ids of its structured metadata and prose.
    pub fn node_version(&self, id: &NodeId) -> Result<DefinitionVersion> {
        Ok(self.load_definition(id)?.version)
    }

    // --- result (structured record plus optional prose) -------------------------

    pub fn write_result(&self, id: &NodeId, meta: &ResultMeta, notes: &str) -> Result<()> {
        if meta.schema != RESULT_SCHEMA {
            bail!("cannot write unsupported result schema {}", meta.schema);
        }
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
    pub fn load_result(&self, id: &NodeId) -> Result<Option<LoadedResult>> {
        let data = match fs::read(self.result_meta_path(id)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs::read(self.result_path(id)) {
                    Ok(_) => bail!("result.md exists without result.toml for `{id}`"),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => {
                        return Err(e).with_context(|| format!("reading result.md for `{id}`"))
                    }
                }
            }
            Err(e) => return Err(e).with_context(|| format!("reading result for `{id}`")),
        };
        let text = std::str::from_utf8(&data)
            .with_context(|| format!("reading result.toml for `{id}`"))?;
        let meta: ResultMeta =
            toml::from_str(text).with_context(|| format!("parsing result.toml for `{id}`"))?;
        if meta.schema != RESULT_SCHEMA {
            bail!("result for `{id}` uses unsupported schema {}", meta.schema);
        }
        let notes_bytes = match fs::read(self.result_path(id)) {
            Ok(notes) => Some(notes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("reading result.md for `{id}`")),
        };
        let notes = match &notes_bytes {
            Some(bytes) => String::from_utf8(bytes.clone())
                .with_context(|| format!("reading result.md for `{id}`"))?,
            None => String::new(),
        };
        Ok(Some(LoadedResult {
            meta,
            notes,
            version: ResultVersion {
                metadata: blob_id(&data),
                notes: notes_bytes.as_deref().map(blob_id),
            },
        }))
    }

    pub fn read_result(&self, id: &NodeId) -> Result<Option<(ResultMeta, String)>> {
        Ok(self
            .load_result(id)?
            .map(|loaded| (loaded.meta, loaded.notes)))
    }

    /// The node's result version, or `None` if it has no result — the pairing
    /// of [`Self::read_result`] and [`Self::result_version`] that pinning and
    /// version checks need, in one pass over the files.
    pub fn current_result_version(&self, id: &NodeId) -> Result<Option<ResultVersion>> {
        Ok(self.load_result(id)?.map(|loaded| loaded.version))
    }

    pub fn result_version(&self, id: &NodeId) -> Result<ResultVersion> {
        self.load_result(id)?
            .map(|loaded| loaded.version)
            .with_context(|| format!("node `{id}` has no result"))
    }

    // --- opaque attachments -------------------------------------------------------

    /// Read one immutable node attachment and its exact payload bytes.
    pub fn read_node_attachment(
        &self,
        id: &NodeId,
        namespace: &str,
        key: &str,
    ) -> Result<Option<(NodeAttachment, Vec<u8>)>> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        let dir = self.attachment_path(id, namespace, key)?;
        if !dir.exists() {
            return Ok(None);
        }
        let metadata_path = dir.join("attachment.toml");
        let data_path = dir.join("data");
        let metadata_text = fs::read_to_string(&metadata_path)
            .with_context(|| format!("reading {}", metadata_path.display()))?;
        let attachment: NodeAttachment = toml::from_str(&metadata_text)
            .with_context(|| format!("parsing {}", metadata_path.display()))?;
        validate_attachment_record(&attachment, namespace, key)?;
        let data =
            fs::read(&data_path).with_context(|| format!("reading {}", data_path.display()))?;
        validate_attachment_data(&attachment, &data)?;
        Ok(Some((attachment, data)))
    }

    /// List attachment metadata in stable namespace/key order.
    pub fn list_node_attachments(&self, id: &NodeId) -> Result<Vec<NodeAttachment>> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        let entries = match fs::read_dir(self.attachments_path(id)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut attachments = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                bail!(
                    "unexpected file in node `{id}` attachments: {}",
                    entry.path().display()
                );
            }
            let metadata_path = entry.path().join("attachment.toml");
            let metadata_text = fs::read_to_string(&metadata_path)
                .with_context(|| format!("reading {}", metadata_path.display()))?;
            let attachment: NodeAttachment = toml::from_str(&metadata_text)
                .with_context(|| format!("parsing {}", metadata_path.display()))?;
            validate_attachment_record(&attachment, &attachment.namespace, &attachment.key)?;
            if entry.file_name().to_string_lossy()
                != attachment_identity(&attachment.namespace, &attachment.key)
            {
                bail!("attachment identity and directory disagree for node `{id}`");
            }
            let data = fs::read(entry.path().join("data"))?;
            validate_attachment_data(&attachment, &data)?;
            attachments.push(attachment);
        }
        attachments.sort_by(|a, b| (&a.namespace, &a.key).cmp(&(&b.namespace, &b.key)));
        Ok(attachments)
    }

    pub(crate) fn write_node_attachment(
        &self,
        id: &NodeId,
        attachment: &NodeAttachment,
        data: &[u8],
    ) -> Result<()> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        validate_attachment_record(attachment, &attachment.namespace, &attachment.key)?;
        validate_attachment_data(attachment, data)?;
        let dir = self.attachment_path(id, &attachment.namespace, &attachment.key)?;
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let metadata =
            toml::to_string_pretty(attachment).context("serialising node attachment metadata")?;
        fs::write(dir.join("data"), data)
            .with_context(|| format!("writing attachment data for node `{id}`"))?;
        fs::write(dir.join("attachment.toml"), metadata)
            .with_context(|| format!("writing attachment metadata for node `{id}`"))?;
        Ok(())
    }

    // --- immutable observations --------------------------------------------------

    pub fn write_context_observation(
        &self,
        id: &NodeId,
        observation: &ContextObservation,
    ) -> Result<()> {
        if observation.schema != OBSERVATION_SCHEMA {
            bail!(
                "cannot write unsupported context observation schema {}",
                observation.schema
            );
        }
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

    pub fn read_context_observations(&self, id: &NodeId) -> Result<Vec<ContextObservation>> {
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
            let observation: ContextObservation =
                toml::from_str(&data).with_context(|| format!("parsing {}", path.display()))?;
            if observation.schema != OBSERVATION_SCHEMA {
                bail!(
                    "context observation {} uses unsupported schema {}",
                    path.display(),
                    observation.schema
                );
            }
            observations.push(observation);
        }
        Ok(observations)
    }

    // --- listing -----------------------------------------------------------------

    pub fn list_ids(&self) -> Result<Vec<NodeId>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.root.join("nodes"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("node directory name is not UTF-8"))?;
                ids.push(
                    name.parse()
                        .map_err(anyhow::Error::msg)
                        .with_context(|| format!("invalid node directory `{name}`"))?,
                );
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

fn validate_attachment_identity(namespace: &str, key: &str) -> Result<()> {
    for (label, value) in [("namespace", namespace), ("key", key)] {
        if value.is_empty() {
            bail!("attachment {label} must not be empty");
        }
        if value.chars().any(char::is_control) {
            bail!("attachment {label} must not contain control characters");
        }
    }
    Ok(())
}

fn attachment_identity(namespace: &str, key: &str) -> String {
    blob_id(format!("{namespace}\0{key}").as_bytes())
}

fn validate_attachment_record(
    attachment: &NodeAttachment,
    namespace: &str,
    key: &str,
) -> Result<()> {
    validate_attachment_identity(namespace, key)?;
    if attachment.schema != ATTACHMENT_SCHEMA {
        bail!(
            "attachment `{namespace}/{key}` uses unsupported schema {}",
            attachment.schema
        );
    }
    if attachment.namespace != namespace || attachment.key != key {
        bail!("attachment identity does not match its recorded namespace and key");
    }
    Ok(())
}

fn validate_attachment_data(attachment: &NodeAttachment, data: &[u8]) -> Result<()> {
    if attachment.size != data.len() as u64 {
        bail!(
            "attachment `{}/{}` size does not match its payload",
            attachment.namespace,
            attachment.key
        );
    }
    if attachment.content != blob_id(data) {
        bail!(
            "attachment `{}/{}` content identity does not match its payload",
            attachment.namespace,
            attachment.key
        );
    }
    Ok(())
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
        let node: NodeId = "node-1".parse().unwrap();

        let meta = NodeMeta {
            schema: 1,
            author: Author::Human,
            assignee: None,
            depends_on: vec!["node-a".parse().unwrap()],
            derived_from: vec![],
            verifies: None,
            extensions: Default::default(),
        };
        store
            .write_node(&node, &meta, "hello\n\nthe details")
            .unwrap();
        let (got, description) = store.read_node(&node).unwrap();
        assert_eq!(got.depends_on, vec!["node-a".parse().unwrap()]);
        assert_eq!(description, "hello\n\nthe details");
        assert_eq!(crate::model::title_of(&description), "hello");

        // The version changes exactly when the definition changes.
        let v1 = store.node_version(&node).unwrap();
        store.write_node(&node, &meta, "other description").unwrap();
        assert_ne!(v1, store.node_version(&node).unwrap());

        // No result yet; then one round-trips, without touching the version.
        assert!(store.read_result(&node).unwrap().is_none());
        let v2 = store.node_version(&node).unwrap();
        let result = ResultMeta {
            schema: 1,
            at: 0,
            author: Author::Machine,
            definition: v2.clone(),
            outcome: Outcome::Done.into(),
            project: crate::ProjectSnapshot {
                scheme: "git".into(),
                repository: String::new(),
                revision: String::new(),
                tree: String::new(),
            },
            consumed: vec![],
            context: vec![],
            output: Some(ArtifactRef {
                scheme: "git-commit".into(),
                repository: String::new(),
                id: "abc".into(),
            }),
            producer: None,
        };
        store.write_result(&node, &result, "did the thing").unwrap();
        let (r, notes) = store.read_result(&node).unwrap().unwrap();
        assert_eq!(r.output.as_ref().map(|a| a.id.as_str()), Some("abc"));
        assert_eq!(r.outcome, Outcome::Done.into());
        assert_eq!(notes, "did the thing");
        assert_eq!(store.node_version(&node).unwrap(), v2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loaded_records_hash_exact_bytes_and_reject_partial_results() {
        let dir = std::env::temp_dir().join(format!("linka-loader-test-{}", ulid::Ulid::new()));
        let store = Store::init(dir.join(".linka")).unwrap();
        let node: NodeId = "node-1".parse().unwrap();
        let meta = NodeMeta {
            schema: DEFINITION_SCHEMA,
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
            verifies: None,
            extensions: Default::default(),
        };
        store.write_node(&node, &meta, "naïve 🧪\n").unwrap();
        let node_path = store.node_path(&node);
        let mut definition_bytes = b"# formatting is versioned\n".to_vec();
        definition_bytes.extend(std::fs::read(&node_path).unwrap());
        std::fs::write(&node_path, &definition_bytes).unwrap();
        let loaded = store.load_definition(&node).unwrap();
        assert_eq!(loaded.version.metadata, blob_id(&definition_bytes));
        assert_eq!(loaded.version.description, blob_id("naïve 🧪\n".as_bytes()));

        let result = ResultMeta {
            schema: RESULT_SCHEMA,
            at: 0,
            author: Author::Machine,
            definition: loaded.version,
            outcome: Outcome::Failed.into(),
            project: crate::ProjectSnapshot {
                scheme: "git".into(),
                repository: String::new(),
                revision: String::new(),
                tree: String::new(),
            },
            consumed: vec![],
            context: vec![],
            output: None,
            producer: None,
        };
        store.write_result(&node, &result, "").unwrap();
        assert_eq!(
            store.load_result(&node).unwrap().unwrap().version.notes,
            None
        );
        std::fs::write(store.result_path(&node), b"").unwrap();
        assert_eq!(
            store.load_result(&node).unwrap().unwrap().version.notes,
            Some(blob_id(b""))
        );
        std::fs::remove_file(store.result_meta_path(&node)).unwrap();
        assert!(store
            .load_result(&node)
            .unwrap_err()
            .to_string()
            .contains("without"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opaque_node_attachments_round_trip_without_changing_node_versions() {
        let dir =
            std::env::temp_dir().join(format!("linka-attachment-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".linka")).unwrap();
        let node: NodeId = "node-1".parse().unwrap();
        let meta = NodeMeta {
            schema: 1,
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
            verifies: None,
            extensions: Default::default(),
        };
        store.write_node(&node, &meta, "attached").unwrap();
        let version = store.node_version(&node).unwrap();
        let data = [0, 1, 2, 255];
        let attachment = NodeAttachment {
            schema: ATTACHMENT_SCHEMA,
            namespace: "test.tool".into(),
            key: "arbitrary report".into(),
            created_at_ms: 42,
            media_type: Some("application/octet-stream".into()),
            content: blob_id(&data),
            size: data.len() as u64,
        };

        store
            .write_node_attachment(&node, &attachment, &data)
            .unwrap();

        assert_eq!(
            store
                .read_node_attachment(&node, "test.tool", "arbitrary report")
                .unwrap(),
            Some((attachment.clone(), data.to_vec()))
        );
        assert_eq!(
            store.list_node_attachments(&node).unwrap(),
            vec![attachment]
        );
        assert_eq!(store.node_version(&node).unwrap(), version);
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
