use super::*;
use crate::Vcs;
use anyhow::{bail, Context, Result};

impl CandidateStore<'_> {
    pub fn publish(&self, vcs: &dyn Vcs, id: &CandidateId) -> Result<PublicationRecord> {
        let view = self.load(id)?;
        let decision = view
            .decision
            .as_ref()
            .filter(|decision| decision.kind == DecisionKind::Accepted)
            .with_context(|| format!("candidate `{id}` has not been accepted"))?;
        if let Some(publication) = &view.publication {
            if publication.completed_at_ms.is_some() {
                return Ok(publication.clone());
            }
        }
        self.require_candidate_ref(vcs, &view.candidate)?;
        self.require_current_candidate(vcs, &view.candidate, IntegrationStatus::Accepted)?;
        if view.publication.is_none() {
            let mutation = self.store.mutation_lock(vcs)?;
            if self.load(id)?.publication.is_none() {
                let publication = PublicationRecord {
                    schema: PUBLICATION_SCHEMA,
                    candidate: id.clone(),
                    candidate_commit: view.candidate.artifact.id.clone(),
                    target_ref: decision
                        .target_ref
                        .clone()
                        .context("acceptance has no target")?,
                    target_previous: decision
                        .target_previous
                        .clone()
                        .context("acceptance has no target baseline")?,
                    prepared_at_ms: now_millis(),
                    completed_at_ms: None,
                };
                storage::write_toml(&self.publication_path(id), &publication)?;
                mutation.commit(vcs, &format!("linka: prepare publication {id}"))?;
            }
        }
        self.recover_publication(vcs, id)
    }

    pub fn recover_publication(
        &self,
        vcs: &dyn Vcs,
        id: &CandidateId,
    ) -> Result<PublicationRecord> {
        let view = self.load(id)?;
        let mut publication = view
            .publication
            .context("candidate has no prepared publication")?;
        if publication.completed_at_ms.is_some() {
            return Ok(publication);
        }
        if publication.candidate_commit != view.candidate.artifact.id {
            bail!("publication no longer matches candidate `{id}`");
        }
        self.require_candidate_ref(vcs, &view.candidate)?;
        let target_now = vcs
            .ref_commit(&publication.target_ref)?
            .context("publication target disappeared")?;
        if target_now == publication.target_previous {
            self.require_current_candidate(vcs, &view.candidate, IntegrationStatus::Accepted)?;
            if !vcs.publish_fast_forward(
                &publication.target_ref,
                &publication.target_previous,
                &publication.candidate_commit,
            )? {
                bail!("candidate `{id}` cannot fast-forward its target branch");
            }
        } else if target_now != publication.candidate_commit {
            bail!(
                "publication target moved from {} to {} while candidate `{id}` was pending",
                publication.target_previous,
                target_now
            );
        }
        publication.completed_at_ms = Some(now_millis());
        let mutation = self.store.mutation_lock(vcs)?;
        if let Some(completed) = self.load(id)?.publication {
            if completed.completed_at_ms.is_some() {
                return Ok(completed);
            }
        }
        storage::write_toml(&self.publication_path(id), &publication)?;
        mutation.commit(vcs, &format!("linka: complete publication {id}"))?;
        Ok(publication)
    }
}
