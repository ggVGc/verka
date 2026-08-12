//! The on-disk store: layout, record I/O, blob ids, and the mutation lock.
//!
//! ```text
//! .linka/
//!   pairing.toml                        which project repo this store describes
//!   nodes/<node-id>/
//!     node.toml                         definition metadata
//!     description.md                    definition prose
//!     result.toml                       the one result, if any
//!     result.md                         result prose, if any
//!     observed-context.toml             post-hoc context pins for that result
//!     attachments/<namespace>/<key>/
//!       meta.toml
//!       data
//!   candidates/<candidate-id>.toml      immutable
//! ```
//!
//! Every path component is a literal, validated name, so a record and its
//! directory can never disagree about which node or attachment it is.
//!
//! The store lives in a *workbench*: an outer git repository holding the store
//! next to the project, which is an ordinary, entirely separate git repository.
//!
//! ```text
//! <workbench>/       outer repo — store history
//!   .linka/          the store
//!   project/         inner repo — the actual project
//! ```

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{
    Attachment, AttachmentKey, Candidate, CandidateId, DefinitionVersion, Namespace, NodeId,
    NodeMeta, ObservedContext, ResultMeta, ResultVersion, ATTACHMENT_SCHEMA, CANDIDATE_SCHEMA,
    DEFINITION_SCHEMA, OBSERVATION_SCHEMA, RESULT_SCHEMA,
};
use crate::vcs::Vcs;

/// Git's blob id for `bytes`, computed locally so version identity needs no
/// git invocation. This assumes git's SHA-1 object format.
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

/// The workbench-wide mutation lock. Held from the clean-store precondition
/// through the single commit that records the whole action.
pub struct MutationLock {
    file: fs::File,
    path: String,
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        // Closing the file also releases the lock, but unlock explicitly so a
        // following mutation in the same process can proceed immediately.
        let _ = self.file.unlock();
    }
}

impl MutationLock {
    /// Commit the one action performed while this lock was held, verify that
    /// the store is clean again, and release the lock on return.
    pub fn commit(self, vcs: &dyn Vcs, message: &str) -> Result<()> {
        vcs.commit_store(&self.path, message)?;
        vcs.require_clean_store(&self.path)
            .context("Linka store is still dirty after committing the mutation")?;
        Ok(())
    }
}

impl Store {
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

    /// Acquire the workbench-wide mutation lock and require the tracked store
    /// to be clean before returning it. The lock file lives inside `.git`, so
    /// it never enters a store commit, and the OS lock is released when the
    /// process exits, including after a crash.
    pub fn mutation_lock(&self, vcs: &dyn Vcs) -> Result<MutationLock> {
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
            Err(fs::TryLockError::WouldBlock) => bail!(
                "another Linka store mutation is in progress ({})",
                lock_path.display()
            ),
            Err(fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| {
                    format!("acquiring store mutation lock {}", lock_path.display())
                })
            }
        }
        let path = self.store_name();
        vcs.require_clean_store(&path)
            .context("Linka store must be clean before mutating")?;
        Ok(MutationLock { file, path })
    }

    // --- paths -------------------------------------------------------------------

    /// The store's root directory (holds `nodes/`, `candidates/`, `pairing.toml`).
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
    fn result_notes_path(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("result.md")
    }
    fn observed_context_path(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("observed-context.toml")
    }
    fn attachments_dir(&self, id: &NodeId) -> PathBuf {
        self.node_dir(id).join("attachments")
    }
    fn attachment_dir(&self, id: &NodeId, namespace: &Namespace, key: &AttachmentKey) -> PathBuf {
        self.attachments_dir(id)
            .join(namespace.as_str())
            .join(key.as_str())
    }
    fn candidates_dir(&self) -> PathBuf {
        self.root.join("candidates")
    }
    fn candidate_path(&self, id: &CandidateId) -> PathBuf {
        self.candidates_dir().join(format!("{id}.toml"))
    }

    pub fn exists(&self, id: &NodeId) -> bool {
        self.node_path(id).is_file()
    }

    // --- definition ----------------------------------------------------------------

    pub fn write_node(&self, id: &NodeId, meta: &NodeMeta, description: &str) -> Result<()> {
        if meta.schema != DEFINITION_SCHEMA {
            bail!("cannot write unsupported definition schema {}", meta.schema);
        }
        fs::create_dir_all(self.node_dir(id))?;
        write_toml(&self.node_path(id), meta)?;
        write_atomically(&self.description_path(id), description.as_bytes())
            .with_context(|| format!("writing description for `{id}`"))?;
        Ok(())
    }

    pub fn read_node(&self, id: &NodeId) -> Result<(NodeMeta, String)> {
        let meta: NodeMeta =
            read_toml(&self.node_path(id)).with_context(|| format!("unknown node `{id}`"))?;
        if meta.schema != DEFINITION_SCHEMA {
            bail!("node `{id}` uses unsupported schema {}", meta.schema);
        }
        let description = fs::read_to_string(self.description_path(id))
            .with_context(|| format!("reading description.md for `{id}`"))?;
        Ok((meta, description))
    }

    /// The node's definition version: git blob ids of its metadata and prose.
    pub fn node_version(&self, id: &NodeId) -> Result<DefinitionVersion> {
        let metadata =
            fs::read(self.node_path(id)).with_context(|| format!("unknown node `{id}`"))?;
        let description = fs::read(self.description_path(id))
            .with_context(|| format!("reading description.md for `{id}`"))?;
        Ok(DefinitionVersion {
            metadata: blob_id(&metadata),
            description: blob_id(&description),
        })
    }

    // --- result ---------------------------------------------------------------------

    pub fn write_result(&self, id: &NodeId, meta: &ResultMeta, notes: &str) -> Result<()> {
        if meta.schema != RESULT_SCHEMA {
            bail!("cannot write unsupported result schema {}", meta.schema);
        }
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        write_toml(&self.result_meta_path(id), meta)?;
        if notes.is_empty() {
            remove_if_present(&self.result_notes_path(id))?;
        } else {
            write_atomically(&self.result_notes_path(id), notes.as_bytes())
                .with_context(|| format!("writing result notes for `{id}`"))?;
        }
        // Post-hoc context belongs to the result it was observed for; a new
        // result leaves no observations behind.
        remove_if_present(&self.observed_context_path(id))
    }

    /// The node's result, or `None` if it has not been worked yet.
    pub fn read_result(&self, id: &NodeId) -> Result<Option<(ResultMeta, String)>> {
        if !self.result_meta_path(id).exists() {
            if self.result_notes_path(id).exists() {
                bail!("result.md exists without result.toml for `{id}`");
            }
            return Ok(None);
        }
        let meta: ResultMeta = read_toml(&self.result_meta_path(id))?;
        if meta.schema != RESULT_SCHEMA {
            bail!("result for `{id}` uses unsupported schema {}", meta.schema);
        }
        let notes = read_optional_string(&self.result_notes_path(id))?.unwrap_or_default();
        Ok(Some((meta, notes)))
    }

    /// The node's result version, or `None` if it has no result.
    pub fn current_result_version(&self, id: &NodeId) -> Result<Option<ResultVersion>> {
        if !self.result_meta_path(id).exists() {
            return Ok(None);
        }
        self.result_version(id).map(Some)
    }

    pub fn result_version(&self, id: &NodeId) -> Result<ResultVersion> {
        let metadata = fs::read(self.result_meta_path(id))
            .with_context(|| format!("node `{id}` has no result"))?;
        let notes = match fs::read(self.result_notes_path(id)) {
            Ok(bytes) => Some(blob_id(&bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("reading result.md for `{id}`"))
            }
        };
        Ok(ResultVersion {
            metadata: blob_id(&metadata),
            notes,
        })
    }

    // --- observed context ----------------------------------------------------------

    /// Replace the node's post-hoc context pins wholesale.
    pub fn write_observed_context(&self, id: &NodeId, observed: &ObservedContext) -> Result<()> {
        if observed.schema != OBSERVATION_SCHEMA {
            bail!(
                "cannot write unsupported observed-context schema {}",
                observed.schema
            );
        }
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        write_toml(&self.observed_context_path(id), observed)
    }

    /// The node's post-hoc context pins, if any were recorded.
    pub fn read_observed_context(&self, id: &NodeId) -> Result<Option<ObservedContext>> {
        let path = self.observed_context_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let observed: ObservedContext = read_toml(&path)?;
        if observed.schema != OBSERVATION_SCHEMA {
            bail!(
                "observed context for `{id}` uses unsupported schema {}",
                observed.schema
            );
        }
        Ok(Some(observed))
    }

    // --- attachments ------------------------------------------------------------------

    pub fn read_attachment(
        &self,
        id: &NodeId,
        namespace: &Namespace,
        key: &AttachmentKey,
    ) -> Result<Option<(Attachment, Vec<u8>)>> {
        let dir = self.attachment_dir(id, namespace, key);
        if !dir.is_dir() {
            return Ok(None);
        }
        let attachment: Attachment = read_toml(&dir.join("meta.toml"))?;
        let data = fs::read(dir.join("data"))
            .with_context(|| format!("reading attachment `{namespace}/{key}` of `{id}`"))?;
        validate_attachment(&attachment, namespace, key, &data)?;
        Ok(Some((attachment, data)))
    }

    /// Attachment metadata in stable namespace/key order.
    pub fn list_attachments(&self, id: &NodeId) -> Result<Vec<Attachment>> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        let mut attachments = Vec::new();
        for namespace in read_dir_names(&self.attachments_dir(id))? {
            let namespace: Namespace = namespace.parse().map_err(anyhow::Error::msg)?;
            let dir = self.attachments_dir(id).join(namespace.as_str());
            for key in read_dir_names(&dir)? {
                let key: AttachmentKey = key.parse().map_err(anyhow::Error::msg)?;
                let (attachment, _) =
                    self.read_attachment(id, &namespace, &key)?
                        .with_context(|| {
                            format!("attachment `{namespace}/{key}` of `{id}` is empty")
                        })?;
                attachments.push(attachment);
            }
        }
        attachments.sort_by(|a, b| (&a.namespace, &a.key).cmp(&(&b.namespace, &b.key)));
        Ok(attachments)
    }

    pub(crate) fn write_attachment(
        &self,
        id: &NodeId,
        attachment: &Attachment,
        data: &[u8],
    ) -> Result<()> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        validate_attachment(attachment, &attachment.namespace, &attachment.key, data)?;
        let dir = self.attachment_dir(id, &attachment.namespace, &attachment.key);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        write_atomically(&dir.join("data"), data)
            .with_context(|| format!("writing attachment data for `{id}`"))?;
        write_toml(&dir.join("meta.toml"), attachment)
    }

    // --- candidates ---------------------------------------------------------------------

    pub fn read_candidate(&self, id: &CandidateId) -> Result<Candidate> {
        let candidate: Candidate = read_toml(&self.candidate_path(id))
            .with_context(|| format!("unknown or unreadable candidate `{id}`"))?;
        validate_candidate(&candidate, id)?;
        Ok(candidate)
    }

    pub fn candidate_exists(&self, id: &CandidateId) -> bool {
        self.candidate_path(id).is_file()
    }

    /// A candidate is immutable, so writing one that already exists is a
    /// programming error rather than an update.
    pub(crate) fn write_candidate(&self, candidate: &Candidate) -> Result<()> {
        if self.candidate_exists(&candidate.id) {
            bail!("candidate `{}` already exists", candidate.id);
        }
        validate_candidate(candidate, &candidate.id)?;
        fs::create_dir_all(self.candidates_dir())?;
        write_toml(&self.candidate_path(&candidate.id), candidate)
    }

    /// Every candidate in the store, with one problem report per record that
    /// could not be read — a bad record must not hide the good ones.
    pub fn load_candidates(&self) -> Result<(Vec<Candidate>, Vec<String>)> {
        let mut candidates = Vec::new();
        let mut problems = Vec::new();
        let mut names = read_dir_entries(&self.candidates_dir())?;
        names.sort();
        for name in names {
            let Some(stem) = name.strip_suffix(".toml") else {
                problems.push(format!("candidates/{name}: not a candidate record"));
                continue;
            };
            let id: CandidateId = match stem.parse() {
                Ok(id) => id,
                Err(error) => {
                    problems.push(format!("candidates/{name}: invalid candidate id ({error})"));
                    continue;
                }
            };
            match self.read_candidate(&id) {
                Ok(candidate) => candidates.push(candidate),
                Err(error) => problems.push(format!("{id}: unreadable candidate ({error:#})")),
            }
        }
        Ok((candidates, problems))
    }

    // --- listing --------------------------------------------------------------------------

    /// Every node id in the store, plus one problem report per directory whose
    /// name is not a usable node id. Discovery reports bad names rather than
    /// failing, so `check` can diagnose exactly the corruption it exists to find.
    pub fn list_nodes(&self) -> Result<(Vec<NodeId>, Vec<String>)> {
        let mut ids = Vec::new();
        let mut problems = Vec::new();
        for name in read_dir_names(&self.root.join("nodes"))? {
            match name.parse() {
                Ok(id) => ids.push(id),
                Err(error) => {
                    problems.push(format!("nodes/{name}: invalid node directory ({error})"))
                }
            }
        }
        ids.sort();
        Ok((ids, problems))
    }

    /// The node ids alone, for callers that only need to iterate the graph.
    pub fn node_ids(&self) -> Result<Vec<NodeId>> {
        Ok(self.list_nodes()?.0)
    }

    // --- workbench geometry -----------------------------------------------------------------

    /// The workbench root: the directory containing the store. Its git
    /// repository holds the store's history.
    pub fn workbench_root(&self) -> PathBuf {
        match self.root.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        }
    }

    /// The project root that output commits and pinned paths resolve against.
    pub fn project_root(&self) -> PathBuf {
        self.workbench_root().join(PROJECT_DIR)
    }

    /// The store directory relative to the workbench root, for use as a git
    /// pathspec when committing store changes (e.g. `.linka`).
    pub fn store_name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }
}

fn validate_candidate(candidate: &Candidate, id: &CandidateId) -> Result<()> {
    if candidate.schema != CANDIDATE_SCHEMA {
        bail!(
            "candidate `{id}` uses unsupported schema {}",
            candidate.schema
        );
    }
    if candidate.id != *id {
        bail!(
            "candidate `{id}` records a different identity `{}`",
            candidate.id
        );
    }
    Ok(())
}

fn validate_attachment(
    attachment: &Attachment,
    namespace: &Namespace,
    key: &AttachmentKey,
    data: &[u8],
) -> Result<()> {
    if attachment.schema != ATTACHMENT_SCHEMA {
        bail!(
            "attachment `{namespace}/{key}` uses unsupported schema {}",
            attachment.schema
        );
    }
    if attachment.namespace != *namespace || attachment.key != *key {
        bail!("attachment `{namespace}/{key}` records a different identity");
    }
    if attachment.size != data.len() as u64 || attachment.content != blob_id(data) {
        bail!("attachment `{namespace}/{key}` does not match its payload");
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

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

fn read_optional_string(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Directory entry names, or an empty list if the directory does not exist.
fn read_dir_entries(dir: &Path) -> Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        names.push(
            entry.file_name().into_string().map_err(|name| {
                anyhow::anyhow!("{}/{:?} is not valid UTF-8", dir.display(), name)
            })?,
        );
    }
    Ok(names)
}

/// Names of subdirectories, sorted; an empty list if the directory is absent.
fn read_dir_names(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for name in read_dir_entries(dir)? {
        if dir.join(&name).is_dir() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text =
        toml::to_string_pretty(value).with_context(|| format!("serialising {}", path.display()))?;
    write_atomically(path, text.as_bytes())
}

/// Write `bytes` to `path` by writing a neighbouring temporary file and
/// renaming it over the target.
///
/// A record is either its old contents or its new ones, never half of either:
/// a write interrupted by a full disk or a killed process would otherwise
/// leave a truncated file, which every later read evaluates as a corrupt
/// record rather than as the good record that was there a moment ago. The
/// temporary file is deliberately left behind on failure — the store is then
/// dirty, which is exactly what blocks the next mutation until someone looks.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .with_context(|| format!("{} names no file", path.display()))?;
    let temporary = directory.join(format!(".{}.tmp", name.to_string_lossy()));
    fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("committing {}", path.display()))
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtifactRef, Author, ContextPin, Outcome, ProjectSnapshot};

    fn temp_store(tag: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!(
            "linka-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let store = Store::init(dir.join(".linka")).unwrap();
        (dir, store)
    }

    fn node_meta() -> NodeMeta {
        NodeMeta {
            schema: DEFINITION_SCHEMA,
            author: Author::Human,
            assignee: None,
            depends_on: Vec::new(),
            derived_from: Vec::new(),
            verifies: None,
        }
    }

    fn result_meta(definition: DefinitionVersion) -> ResultMeta {
        ResultMeta {
            schema: RESULT_SCHEMA,
            at: 0,
            author: Author::Machine,
            definition,
            outcome: Outcome::Done,
            project: ProjectSnapshot {
                scheme: "git".into(),
                repository: String::new(),
                revision: String::new(),
                tree: String::new(),
            },
            consumed: Vec::new(),
            context: Vec::new(),
            output: Some(ArtifactRef {
                scheme: "git-commit".into(),
                repository: String::new(),
                id: "abc".into(),
            }),
            producer: None,
        }
    }

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
    fn node_and_result_round_trip_and_move_only_their_own_version() {
        let (dir, store) = temp_store("roundtrip");
        let node: NodeId = "node-1".parse().unwrap();
        store
            .write_node(&node, &node_meta(), "hello\n\nthe details")
            .unwrap();
        let (got, description) = store.read_node(&node).unwrap();
        assert_eq!(got, node_meta());
        assert_eq!(description, "hello\n\nthe details");

        let first = store.node_version(&node).unwrap();
        store.write_node(&node, &node_meta(), "other").unwrap();
        let second = store.node_version(&node).unwrap();
        assert_ne!(first, second);

        assert!(store.read_result(&node).unwrap().is_none());
        store
            .write_result(&node, &result_meta(second.clone()), "did the thing")
            .unwrap();
        let (result, notes) = store.read_result(&node).unwrap().unwrap();
        assert_eq!(result.outcome, Outcome::Done);
        assert_eq!(notes, "did the thing");
        // Recording a result does not move the definition version.
        assert_eq!(store.node_version(&node).unwrap(), second);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_result_drops_the_observations_made_for_the_previous_one() {
        let (dir, store) = temp_store("observed");
        let node: NodeId = "node-1".parse().unwrap();
        store.write_node(&node, &node_meta(), "work").unwrap();
        let version = store.node_version(&node).unwrap();
        store
            .write_result(&node, &result_meta(version.clone()), "")
            .unwrap();
        let observed = ObservedContext {
            schema: OBSERVATION_SCHEMA,
            result: store.result_version(&node).unwrap(),
            pins: vec![ContextPin {
                path: "src/lib.rs".parse().unwrap(),
                identity: blob_id(b"x"),
                observed: true,
            }],
        };
        store.write_observed_context(&node, &observed).unwrap();
        assert_eq!(store.read_observed_context(&node).unwrap(), Some(observed));

        store
            .write_result(&node, &result_meta(version), "again")
            .unwrap();
        assert_eq!(store.read_observed_context(&node).unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn attachments_are_addressed_by_namespace_and_key_and_never_move_the_version() {
        let (dir, store) = temp_store("attachments");
        let node: NodeId = "node-1".parse().unwrap();
        store.write_node(&node, &node_meta(), "attached").unwrap();
        let version = store.node_version(&node).unwrap();
        let data = [0u8, 1, 2, 255];
        let attachment = Attachment {
            schema: ATTACHMENT_SCHEMA,
            namespace: "test.tool".parse().unwrap(),
            key: "report".parse().unwrap(),
            created_at_ms: 42,
            media_type: Some("application/octet-stream".into()),
            content: blob_id(&data),
            size: data.len() as u64,
        };
        store.write_attachment(&node, &attachment, &data).unwrap();

        assert!(dir
            .join(".linka/nodes/node-1/attachments/test.tool/report/data")
            .is_file());
        assert_eq!(
            store
                .read_attachment(&node, &attachment.namespace, &attachment.key)
                .unwrap(),
            Some((attachment.clone(), data.to_vec()))
        );
        assert_eq!(store.list_attachments(&node).unwrap(), vec![attachment]);
        assert_eq!(store.node_version(&node).unwrap(), version);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_payload_that_disagrees_with_its_metadata_is_an_error() {
        let (dir, store) = temp_store("corrupt-attachment");
        let node: NodeId = "node-1".parse().unwrap();
        store.write_node(&node, &node_meta(), "attached").unwrap();
        let attachment = Attachment {
            schema: ATTACHMENT_SCHEMA,
            namespace: "test.tool".parse().unwrap(),
            key: "report".parse().unwrap(),
            created_at_ms: 0,
            media_type: None,
            content: blob_id(b"one"),
            size: 3,
        };
        store.write_attachment(&node, &attachment, b"one").unwrap();
        fs::write(
            dir.join(".linka/nodes/node-1/attachments/test.tool/report/data"),
            b"two!",
        )
        .unwrap();
        assert!(store
            .read_attachment(&node, &attachment.namespace, &attachment.key)
            .is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_is_replaced_whole_and_leaves_no_scratch_file_behind() {
        let (dir, store) = temp_store("atomic");
        let node: NodeId = "node-1".parse().unwrap();
        store.write_node(&node, &node_meta(), "first").unwrap();
        store.write_node(&node, &node_meta(), "second").unwrap();

        let entries: Vec<String> = fs::read_dir(store.node_dir(&node))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !entries.iter().any(|name| name.ends_with(".tmp")),
            "{entries:?}"
        );
        assert_eq!(store.read_node(&node).unwrap().1, "second");

        // A scratch file left by an interrupted write is evidence, not a
        // record: readers and discovery both ignore it.
        fs::write(store.node_dir(&node).join(".node.toml.tmp"), "half a fi").unwrap();
        assert_eq!(store.read_node(&node).unwrap().0, node_meta());
        assert_eq!(store.list_nodes().unwrap(), (vec![node], Vec::new()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovery_reports_bad_names_instead_of_failing() {
        let (dir, store) = temp_store("discovery");
        let node: NodeId = "node-1".parse().unwrap();
        store.write_node(&node, &node_meta(), "fine").unwrap();
        fs::create_dir_all(store.root().join("nodes/.git")).unwrap();
        fs::create_dir_all(store.root().join("candidates")).unwrap();
        fs::write(store.root().join("candidates/nonsense.toml"), "x = ").unwrap();

        let (ids, problems) = store.list_nodes().unwrap();
        assert_eq!(ids, vec![node]);
        assert_eq!(problems.len(), 1, "{problems:?}");

        let (candidates, problems) = store.load_candidates().unwrap();
        assert!(candidates.is_empty());
        assert_eq!(problems.len(), 1, "{problems:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn init_lays_out_the_workbench() {
        let (dir, store) = temp_store("layout");
        assert_eq!(store.workbench_root(), dir);
        assert_eq!(store.project_root(), dir.join(PROJECT_DIR));
        assert!(dir.join(".linka/nodes").is_dir());
        assert!(store.project_root().is_dir());
        assert_eq!(store.store_name(), ".linka");
        let _ = fs::remove_dir_all(&dir);
    }
}
