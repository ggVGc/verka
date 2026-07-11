use crate::{
    Author, DefinitionVersion, GraphView, ResultRecord, ResultVersion, WorkGraph,
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeDefinition {
    pub schema: u32,
    pub author: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Author>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// Namespaced application metadata is preserved but never interpreted by
    /// core.
    #[serde(default, flatten)]
    pub extensions: std::collections::BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeRecord {
    pub definition: NodeDefinition,
    pub description: String,
}

pub struct FsGraphStore {
    root: PathBuf,
}

impl FsGraphStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.join("nodes").is_dir() {
            bail!("no llaundry graph at {}", root.display());
        }
        Ok(Self { root })
    }
    pub fn init(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("nodes"))?;
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn dir(&self, id: &str) -> PathBuf {
        self.root.join("nodes").join(id)
    }
    pub fn exists(&self, id: &str) -> bool {
        self.dir(id).join("node.toml").is_file()
    }
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
    pub fn read_node(&self, id: &str) -> Result<NodeRecord> {
        let dir = self.dir(id);
        Ok(NodeRecord {
            definition: toml::from_str(&fs::read_to_string(dir.join("node.toml"))?)?,
            description: fs::read_to_string(dir.join("description.md"))?,
        })
    }
    pub fn write_node(&self, id: &str, node: &NodeRecord) -> Result<()> {
        let dir = self.dir(id);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("node.toml"),
            toml::to_string_pretty(&node.definition)?,
        )?;
        fs::write(dir.join("description.md"), &node.description)?;
        Ok(())
    }
    pub fn add(&self, definition: NodeDefinition, description: String) -> Result<String> {
        if description.trim().is_empty() {
            bail!("a node needs a description");
        }
        for id in definition.depends_on.iter().chain(&definition.derived_from) {
            if !self.exists(id) {
                bail!("unknown related node `{id}`");
            }
        }
        let id = format!("node-{}", ulid::Ulid::new());
        self.write_node(
            &id,
            &NodeRecord {
                definition,
                description,
            },
        )?;
        Ok(id)
    }
    pub fn link(&self, from: &str, to: &str, blocking: bool) -> Result<()> {
        if from == to {
            bail!("cannot link a node to itself");
        }
        if !self.exists(to) {
            bail!("unknown related node `{to}`");
        }
        let mut node = self.read_node(from)?;
        let edges = if blocking {
            &mut node.definition.depends_on
        } else {
            &mut node.definition.derived_from
        };
        if edges.iter().any(|id| id == to) {
            bail!("duplicate edge");
        }
        edges.push(to.into());
        self.write_node(from, &node)
    }
    pub fn edit(&self, id: &str, description: String) -> Result<()> {
        if description.trim().is_empty() {
            bail!("a node needs a description");
        }
        let mut node = self.read_node(id)?;
        node.description = description;
        self.write_node(id, &node)
    }
    pub fn definition_version(&self, id: &str) -> Result<DefinitionVersion> {
        let dir = self.dir(id);
        Ok(DefinitionVersion {
            metadata: blob_id(&fs::read(dir.join("node.toml"))?),
            description: blob_id(&fs::read(dir.join("description.md"))?),
        })
    }
    pub fn read_result(&self, id: &str) -> Result<Option<ResultRecord>> {
        let path = self.dir(id).join("result.toml");
        let data = match fs::read_to_string(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(toml::from_str(&data)?))
    }
    pub fn write_result(
        &self,
        id: &str,
        result: &ResultRecord,
        notes: &str,
    ) -> Result<ResultVersion> {
        if !self.exists(id) {
            bail!("unknown node `{id}`");
        }
        let dir = self.dir(id);
        fs::write(dir.join("result.toml"), toml::to_string_pretty(result)?)?;
        let notes_hash = if notes.is_empty() {
            let _ = fs::remove_file(dir.join("result.md"));
            None
        } else {
            fs::write(dir.join("result.md"), notes)?;
            Some(blob_id(notes.as_bytes()))
        };
        Ok(ResultVersion {
            metadata: blob_id(&fs::read(dir.join("result.toml"))?),
            notes: notes_hash,
        })
    }
    pub fn result_version(&self, id: &str) -> Result<Option<ResultVersion>> {
        if self.read_result(id)?.is_none() {
            return Ok(None);
        }
        let dir = self.dir(id);
        Ok(Some(ResultVersion {
            metadata: blob_id(&fs::read(dir.join("result.toml"))?),
            notes: fs::read(dir.join("result.md"))
                .ok()
                .map(|bytes| blob_id(&bytes)),
        }))
    }
}

impl GraphView for FsGraphStore {
    type Error = anyhow::Error;
    fn definition_version(&self, id: &str) -> Result<DefinitionVersion> {
        self.definition_version(id)
    }
    fn result(&self, id: &str) -> Result<Option<ResultRecord>> {
        self.read_result(id)
    }
    fn result_version(&self, id: &str) -> Result<Option<ResultVersion>> {
        self.result_version(id)
    }
}

impl crate::DependencyView for FsGraphStore {
    fn exists(&self, id: &str) -> bool {
        self.exists(id)
    }
    fn dependencies(&self, id: &str) -> Result<Vec<String>> {
        Ok(self.read_node(id)?.definition.depends_on)
    }
}

impl WorkGraph for FsGraphStore {
    type Error = anyhow::Error;
    fn definition_version(&self, id: &str) -> Result<DefinitionVersion> {
        self.definition_version(id)
    }
    fn result(&self, id: &str) -> Result<Option<ResultRecord>> {
        self.read_result(id)
    }
    fn submit(&self, id: &str, result: &ResultRecord, notes: &str) -> Result<ResultVersion> {
        self.write_result(id, result, notes)
    }
}

/// Git's blob id for `bytes`, computed locally so version identity needs no
/// git invocation.
pub fn blob_id(bytes: &[u8]) -> String {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()).as_bytes());
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArtifactRef, Outcome};

    #[test]
    fn node_extensions_survive_edits_and_results_round_trip() {
        let root = std::env::temp_dir().join(format!("llaundry-core-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("nodes/node-1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("node.toml"),
            "schema = 1\nauthor = \"human\"\n[some-app]\ndata = \"preserved\"\n",
        )
        .unwrap();
        fs::write(dir.join("description.md"), "work").unwrap();
        let store = FsGraphStore::open(&root).unwrap();
        store.edit("node-1", "edited".into()).unwrap();
        assert!(fs::read_to_string(dir.join("node.toml"))
            .unwrap()
            .contains("[some-app]"));
        let version = store.definition_version("node-1").unwrap();
        let result = ResultRecord {
            at: 1,
            author: Author::Human,
            definition: version.clone(),
            outcome: Outcome::Done,
            consumed: vec![],
            context: vec![],
            output: Some(ArtifactRef {
                scheme: "git-commit".into(),
                repository: String::new(),
                id: "abc".into(),
            }),
            producer: None,
        };
        store.write_result("node-1", &result, "done").unwrap();
        let read = store.read_result("node-1").unwrap().unwrap();
        assert_eq!(read, result);
        assert_eq!(read.output.unwrap().id, "abc");
        let _ = fs::remove_dir_all(root);
    }
}
