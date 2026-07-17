use super::*;
use crate::Vcs;
use anyhow::{bail, Context, Result};

impl CandidateStore<'_> {
    pub fn register(&self, vcs: &dyn Vcs, new: NewCandidate) -> Result<CandidateRecord> {
        validate_external(new.external.as_ref())?;
        validate_branch_name(&new.branch)?;
        validate_branch_name(&new.target)?;
        let mutation = self.store.mutation_lock(vcs)?;
        if let Some(external) = &new.external {
            if let Some(existing) = self.by_external(external)? {
                if existing.node != new.node
                    || existing.branch != new.branch
                    || existing.input_commit != new.input_commit
                    || existing.target != new.target
                {
                    bail!(
                        "external candidate identity `{}/{}` is already attached to different facts",
                        external.namespace,
                        external.id
                    );
                }
                return Ok(existing);
            }
        }

        let (result, _) = self
            .store
            .read_result(new.node.as_str())?
            .with_context(|| format!("node `{}` has no successful result to register", new.node))?;
        if result.outcome != crate::Outcome::Done {
            bail!("node `{}` does not have a successful result", new.node);
        }
        let artifact = result
            .output
            .clone()
            .with_context(|| format!("node `{}` result has no project output", new.node))?;
        if vcs.ref_commit(&branch_ref(&new.branch))?.as_deref() != Some(artifact.id.as_str()) {
            bail!(
                "candidate branch `{}` does not point to recorded output {}",
                new.branch,
                artifact.id
            );
        }
        let candidate = CandidateRecord {
            schema: CANDIDATE_SCHEMA,
            id: CandidateId::new(),
            result: self.store.result_version(new.node.as_str())?,
            node: new.node,
            artifact,
            branch: new.branch,
            input_commit: new.input_commit,
            target: new.target,
            external: new.external,
            state: CandidateState::Pending,
        };
        storage::write_toml(&self.record_path(&candidate.id), &candidate)?;
        mutation.commit(vcs, &format!("linka: register candidate {}", candidate.id))?;
        Ok(candidate)
    }

    pub fn accept(
        &self,
        vcs: &dyn Vcs,
        id: &CandidateId,
        author: Author,
        notes: String,
    ) -> Result<CandidateRecord> {
        let mutation = self.store.mutation_lock(vcs)?;
        let mut candidate = self.load(id)?;
        match candidate.state {
            CandidateState::Accepted { .. } => return Ok(candidate),
            CandidateState::Rejected { .. } => bail!("candidate `{id}` was already rejected"),
            CandidateState::Pending => {}
        }
        self.require_candidate_ref(vcs, &candidate)?;
        self.require_current_candidate(vcs, &candidate, IntegrationStatus::Pending)?;
        let target_ref = branch_ref(&candidate.target);
        let target_previous = vcs
            .ref_commit(&target_ref)?
            .with_context(|| format!("target branch `{}` does not exist", candidate.target))?;
        candidate.state = CandidateState::Accepted {
            decided_at_ms: now_millis(),
            author,
            notes,
            target_previous,
        };
        storage::write_toml(&self.record_path(id), &candidate)?;
        mutation.commit(vcs, &format!("linka: accept candidate {id}"))?;
        Ok(candidate)
    }

    pub fn reject(
        &self,
        vcs: &dyn Vcs,
        id: &CandidateId,
        author: Author,
        notes: String,
    ) -> Result<CandidateRecord> {
        if notes.trim().is_empty() {
            bail!("rejection requires notes");
        }
        let mutation = self.store.mutation_lock(vcs)?;
        let mut candidate = self.load(id)?;
        match &candidate.state {
            CandidateState::Rejected {
                notes: existing, ..
            } if existing == &notes => return Ok(candidate),
            CandidateState::Pending => {}
            _ => bail!("candidate `{id}` already has a different decision"),
        }
        self.require_current_candidate(vcs, &candidate, IntegrationStatus::Pending)?;
        candidate.state = CandidateState::Rejected {
            decided_at_ms: now_millis(),
            author,
            notes,
        };
        storage::write_toml(&self.record_path(id), &candidate)?;
        mutation.commit(vcs, &format!("linka: reject candidate {id}"))?;
        Ok(candidate)
    }

    pub(super) fn require_candidate_ref(
        &self,
        vcs: &dyn Vcs,
        candidate: &CandidateRecord,
    ) -> Result<()> {
        if vcs.ref_commit(&branch_ref(&candidate.branch))?.as_deref()
            != Some(candidate.artifact.id.as_str())
        {
            bail!(
                "candidate branch `{}` moved from accepted artifact {}",
                candidate.branch,
                candidate.artifact.id
            );
        }
        Ok(())
    }

    pub(super) fn require_current_candidate(
        &self,
        vcs: &dyn Vcs,
        candidate: &CandidateRecord,
        expected: IntegrationStatus,
    ) -> Result<()> {
        let Some((result, _)) = self.store.read_result(candidate.node.as_str())? else {
            bail!("candidate `{}` source result disappeared", candidate.id);
        };
        if self.store.result_version(candidate.node.as_str())? != candidate.result
            || result.output.as_ref() != Some(&candidate.artifact)
        {
            bail!(
                "candidate `{}` is no longer the current result for node `{}`",
                candidate.id,
                candidate.node
            );
        }
        let current = self
            .for_result(&candidate.node, &candidate.result, &candidate.artifact)?
            .with_context(|| format!("candidate `{}` is no longer current", candidate.id))?;
        let state = crate::ops::node_state(self.store, vcs, candidate.node.as_str())?;
        if current.id != candidate.id
            || current.integration(vcs)? != expected
            || state.currency != crate::Currency::Current
        {
            bail!(
                "candidate `{}` is not the current {:?} candidate for node `{}`",
                candidate.id,
                expected,
                candidate.node
            );
        }
        Ok(())
    }
}

fn validate_branch_name(branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch.starts_with("refs/")
        || branch.contains("..")
        || branch.contains(' ')
        || branch.chars().any(char::is_control)
    {
        bail!("invalid branch name `{branch}`");
    }
    Ok(())
}

fn validate_external(external: Option<&ExternalIdentity>) -> Result<()> {
    if let Some(external) = external {
        if external.namespace.trim().is_empty() || external.id.trim().is_empty() {
            bail!("external candidate identity needs a namespace and id");
        }
    }
    Ok(())
}
