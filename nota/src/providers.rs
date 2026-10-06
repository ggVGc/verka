use crate::git::Git;
use crate::{ReviewProvider, ReviewSubject};
use anyhow::Result;
use std::path::PathBuf;

/// Resolves ordinary Git revisions.
pub struct GitProvider<'a> {
    git: &'a dyn Git,
    repository: PathBuf,
}

impl<'a> GitProvider<'a> {
    pub fn new(git: &'a dyn Git, repository: impl Into<PathBuf>) -> Self {
        Self {
            git,
            repository: repository.into(),
        }
    }
}

impl ReviewProvider for GitProvider<'_> {
    fn resolve_subject(&self, reference: &str) -> Result<ReviewSubject> {
        let repository = self.git.repository_root(&self.repository)?;
        let revision = self.git.resolve_commit(&repository, reference)?;
        Ok(ReviewSubject {
            repository,
            revision,
            title: format!("Git revision {reference}"),
        })
    }
}
