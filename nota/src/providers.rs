use crate::git::{repository_root, resolve_commit};
use crate::{ReviewProvider, ReviewSubject};
use anyhow::Result;
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
