use super::*;
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

impl CandidateStore<'_> {
    pub fn list(&self) -> Result<Vec<CandidateId>> {
        let root = self.root();
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("reading {}", root.display())),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let text = entry.file_name().to_string_lossy().into_owned();
                if text.starts_with("candidate-") {
                    ids.push(CandidateId(text));
                }
            }
        }
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(ids)
    }

    pub fn load(&self, id: &CandidateId) -> Result<CandidateRecord> {
        validate_candidate_id(id)?;
        let candidate: CandidateRecord = read_toml(&self.record_path(id))
            .with_context(|| format!("unknown or unreadable candidate `{id}`"))?;
        if candidate.schema != CANDIDATE_SCHEMA {
            bail!(
                "candidate `{id}` uses unsupported schema {}",
                candidate.schema
            );
        }
        Ok(candidate)
    }

    pub fn for_node(&self, node: &NodeId) -> Result<Vec<CandidateRecord>> {
        self.list()?
            .into_iter()
            .map(|id| self.load(&id))
            .filter(|view| match view {
                Ok(candidate) => candidate.node == *node,
                Err(_) => true,
            })
            .collect()
    }

    pub fn for_result(
        &self,
        node: &NodeId,
        result: &ResultVersion,
        artifact: &ArtifactRef,
    ) -> Result<Option<CandidateRecord>> {
        let mut matches = self
            .for_node(node)?
            .into_iter()
            .filter(|candidate| candidate.result == *result && candidate.artifact == *artifact)
            .collect::<Vec<_>>();
        Ok(matches.pop())
    }

    pub fn by_external(&self, external: &ExternalIdentity) -> Result<Option<CandidateRecord>> {
        for id in self.list()? {
            let candidate = self.load(&id)?;
            if candidate.external.as_ref() == Some(external) {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    pub(super) fn record_path(&self, id: &CandidateId) -> PathBuf {
        self.dir(id).join("candidate.toml")
    }

    fn root(&self) -> PathBuf {
        self.store.root().join("candidates")
    }

    fn dir(&self, id: &CandidateId) -> PathBuf {
        self.root().join(&id.0)
    }
}

pub(super) fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("candidate record has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let text =
        toml::to_string_pretty(value).with_context(|| format!("serialising {}", path.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("committing {}", path.display()))
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn validate_candidate_id(id: &CandidateId) -> Result<()> {
    if !id.0.starts_with("candidate-")
        || id.0.contains('/')
        || id.0.contains('\\')
        || id.0.chars().any(char::is_control)
    {
        bail!("invalid candidate id `{id}`");
    }
    Ok(())
}
