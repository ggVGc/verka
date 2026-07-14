use crate::git::{repository_root, resolve_commit};
use crate::{ReviewProvider, ReviewSubject};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Resolves ordinary Git revisions.
pub struct GitProvider {
    repository: PathBuf,
}

impl GitProvider {
    pub fn new(repository: impl Into<PathBuf>) -> Self {
        Self {
            repository: repository.into(),
        }
    }
}

impl ReviewProvider for GitProvider {
    fn resolve_subject(&self, reference: &str) -> Result<ReviewSubject> {
        let repository = repository_root(&self.repository)?;
        let revision = resolve_commit(&repository, reference)?;
        Ok(ReviewSubject {
            repository,
            revision,
            title: format!("Git revision {reference}"),
        })
    }
}

/// Resolves Linka nodes to their current successful Git output.
pub struct LinkaProvider<'a> {
    store: &'a linka::Store,
}

impl<'a> LinkaProvider<'a> {
    pub fn new(store: &'a linka::Store) -> Self {
        Self { store }
    }
}

impl ReviewProvider for LinkaProvider<'_> {
    fn resolve_subject(&self, reference: &str) -> Result<ReviewSubject> {
        let _: linka::NodeId = reference
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid Linka node id: {error}"))?;
        let (_, description) = self.store.read_node(reference)?;
        let vcs = linka::GitVcs::for_store(self.store);
        if !linka::ops::node_state(self.store, &vcs, reference)?.is_complete() {
            bail!("Linka node `{reference}` does not have a current successful result");
        }
        let (result, _) = self
            .store
            .read_result(reference)?
            .ok_or_else(|| anyhow::anyhow!("Linka node `{reference}` has no result to review"))?;
        let artifact = result.output.ok_or_else(|| {
            anyhow::anyhow!("Linka node `{reference}` has no output artifact to review")
        })?;
        if artifact.scheme != "git-commit" {
            bail!(
                "Linka node `{reference}` output uses unsupported `{}` artifacts",
                artifact.scheme
            );
        }
        let repository = repository_root(&self.store.project_root())?;
        let revision = resolve_commit(&repository, &artifact.id).with_context(|| {
            format!("Linka node `{reference}` output commit is not in the paired project")
        })?;
        Ok(ReviewSubject {
            repository,
            revision,
            title: linka::title_of(&description).to_string(),
        })
    }
}
