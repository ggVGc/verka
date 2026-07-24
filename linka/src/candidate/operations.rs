use super::*;
use crate::Vcs;
use anyhow::{bail, Context, Result};

impl CandidateStore<'_> {
    pub(crate) fn prepare_verification_decision(
        &self,
        vcs: &dyn Vcs,
        id: &CandidateId,
        verification: &crate::NodeId,
        result: &crate::ResultMeta,
        author: Author,
        notes: String,
    ) -> Result<CandidateRecord> {
        let outcome = match result.outcome {
            crate::ResultOutcome::Verification(outcome @ crate::VerificationOutcome::Accepted)
            | crate::ResultOutcome::Verification(outcome @ crate::VerificationOutcome::Rejected) => {
                outcome
            }
            crate::ResultOutcome::Verification(crate::VerificationOutcome::Abandoned) => {
                bail!("an abandoned verification cannot decide candidate `{id}`")
            }
            crate::ResultOutcome::Work(_) => {
                bail!("a work result cannot decide candidate `{id}`")
            }
        };
        if outcome == crate::VerificationOutcome::Rejected && notes.trim().is_empty() {
            bail!("rejected verification requires notes");
        }
        let mut candidate = self.load(id)?;
        if !matches!(candidate.state, CandidateState::Pending) {
            bail!("candidate `{id}` already has a decision");
        }
        self.require_current(vcs, &candidate, IntegrationStatus::Pending)?;
        require_exact_candidate_pin(&candidate, verification, result, outcome)?;
        candidate.state = match outcome {
            crate::VerificationOutcome::Accepted => {
                let target_ref = branch_ref(&candidate.target);
                let target_previous = vcs.ref_commit(&target_ref)?.with_context(|| {
                    format!("target branch `{}` does not exist", candidate.target)
                })?;
                CandidateState::Accepted {
                    decided_at_ms: now_millis(),
                    author,
                    notes,
                    verification: Some(verification.clone()),
                    target_previous,
                }
            }
            crate::VerificationOutcome::Rejected => CandidateState::Rejected {
                decided_at_ms: now_millis(),
                author,
                notes,
                verification: Some(verification.clone()),
            },
            crate::VerificationOutcome::Abandoned => unreachable!(),
        };
        Ok(candidate)
    }

    pub(crate) fn write_prepared_decision(&self, candidate: &CandidateRecord) -> Result<()> {
        storage::write_toml(&self.record_path(&candidate.id), candidate)
    }

    pub fn register(&self, vcs: &dyn Vcs, new: NewCandidate) -> Result<CandidateRecord> {
        validate_external(new.external.as_ref())?;
        validate_branch_name(&new.branch)?;
        validate_branch_name(&new.target)?;
        let mutation = self.store.mutation_lock(vcs)?;
        if let Some(external) = &new.external {
            if let Some(existing) = self.by_external(external)? {
                if existing.node != new.node
                    || existing.branch != new.branch
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
        if result.outcome != crate::ResultOutcome::Work(crate::Outcome::Done) {
            bail!("node `{}` does not have a successful result", new.node);
        }
        let artifact = result
            .output
            .clone()
            .with_context(|| format!("node `{}` result has no project output", new.node))?;
        let result_version = self.store.result_version(new.node.as_str())?;
        if let Some(existing) = self.for_result(&new.node, &result_version, &artifact)? {
            if existing.branch == new.branch
                && existing.target == new.target
                && existing.external == new.external
            {
                return Ok(existing);
            }
            bail!(
                "node `{}` result already has candidate `{}` with different facts",
                new.node,
                existing.id
            );
        }
        let candidate = CandidateRecord {
            schema: CANDIDATE_SCHEMA,
            id: CandidateId::new(),
            result: result_version,
            node: new.node,
            artifact,
            branch: new.branch,
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
        verification: &crate::NodeId,
        author: Author,
        notes: String,
    ) -> Result<CandidateRecord> {
        let mutation = self.store.mutation_lock(vcs)?;
        let mut candidate = self.load(id)?;
        match &candidate.state {
            CandidateState::Accepted {
                verification: existing,
                ..
            } if existing
                .as_ref()
                .is_none_or(|existing| existing == verification) =>
            {
                return Ok(candidate)
            }
            CandidateState::Accepted { .. } => {
                bail!("candidate `{id}` was accepted by a different verification")
            }
            CandidateState::Rejected { .. } => bail!("candidate `{id}` was already rejected"),
            CandidateState::Pending => {}
        }
        self.require_current(vcs, &candidate, IntegrationStatus::Pending)?;
        self.require_verification(
            vcs,
            &candidate,
            verification,
            crate::VerificationOutcome::Accepted,
        )?;
        let target_ref = branch_ref(&candidate.target);
        let target_previous = vcs
            .ref_commit(&target_ref)?
            .with_context(|| format!("target branch `{}` does not exist", candidate.target))?;
        candidate.state = CandidateState::Accepted {
            decided_at_ms: now_millis(),
            author,
            notes,
            verification: Some(verification.clone()),
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
        verification: &crate::NodeId,
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
            } if existing == &notes
                && matches!(
                    &candidate.state,
                    CandidateState::Rejected {
                        verification: existing,
                        ..
                    } if existing.as_ref().is_none_or(|existing| existing == verification)
                ) =>
            {
                return Ok(candidate)
            }
            CandidateState::Pending => {}
            _ => bail!("candidate `{id}` already has a different decision"),
        }
        self.require_current(vcs, &candidate, IntegrationStatus::Pending)?;
        self.require_verification(
            vcs,
            &candidate,
            verification,
            crate::VerificationOutcome::Rejected,
        )?;
        candidate.state = CandidateState::Rejected {
            decided_at_ms: now_millis(),
            author,
            notes,
            verification: Some(verification.clone()),
        };
        storage::write_toml(&self.record_path(id), &candidate)?;
        mutation.commit(vcs, &format!("linka: reject candidate {id}"))?;
        Ok(candidate)
    }

    /// Fast-forward the accepted target. Git history is the publication record,
    /// so retrying after a crash is sufficient and needs no Linka journal.
    pub fn publish(&self, vcs: &dyn Vcs, id: &CandidateId) -> Result<()> {
        let candidate = self.load(id)?;
        let CandidateState::Accepted {
            target_previous, ..
        } = &candidate.state
        else {
            bail!("candidate `{id}` has not been accepted");
        };
        if candidate.integration(vcs)? == IntegrationStatus::Published {
            return Ok(());
        }
        self.require_current(vcs, &candidate, IntegrationStatus::Accepted)?;
        if !vcs.publish_fast_forward(
            &branch_ref(&candidate.target),
            target_previous,
            &candidate.artifact.id,
        )? {
            bail!("candidate `{id}` cannot fast-forward its target branch");
        }
        Ok(())
    }

    pub(super) fn require_current(
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
        let state = crate::ops::node_state(self.store, vcs, candidate.node.as_str())?;
        if state.integration != expected || state.currency != crate::Currency::Current {
            bail!(
                "candidate `{}` is not the current {:?} candidate for node `{}`",
                candidate.id,
                expected,
                candidate.node
            );
        }
        Ok(())
    }

    fn require_verification(
        &self,
        vcs: &dyn Vcs,
        candidate: &CandidateRecord,
        verification: &crate::NodeId,
        expected: crate::VerificationOutcome,
    ) -> Result<()> {
        let (meta, _) = self.store.read_node(verification.as_str())?;
        if meta.verifies.as_ref() != Some(&candidate.id) {
            bail!(
                "verification `{verification}` does not verify candidate `{}`",
                candidate.id
            );
        }
        let (result, _) = self
            .store
            .read_result(verification.as_str())?
            .with_context(|| format!("verification `{verification}` has no result"))?;
        if result.outcome != crate::ResultOutcome::Verification(expected) {
            bail!(
                "verification `{verification}` is {}, not {}",
                result.outcome.as_str(),
                expected.as_str()
            );
        }
        require_exact_candidate_pin(candidate, verification, &result, expected)?;
        let state = crate::ops::node_state(self.store, vcs, verification.as_str())?;
        if state.currency != crate::Currency::Current {
            bail!("verification `{verification}` is stale");
        }
        Ok(())
    }
}

fn require_exact_candidate_pin(
    candidate: &CandidateRecord,
    verification: &crate::NodeId,
    result: &crate::ResultMeta,
    expected: crate::VerificationOutcome,
) -> Result<()> {
    if result.outcome != crate::ResultOutcome::Verification(expected) {
        bail!(
            "verification `{verification}` is {}, not {}",
            result.outcome.as_str(),
            expected.as_str()
        );
    }
    let source_pin = result
        .consumed
        .iter()
        .find(|pin| pin.id == candidate.node)
        .with_context(|| {
            format!(
                "verification `{verification}` does not pin candidate source `{}`",
                candidate.node
            )
        })?;
    if source_pin.result.as_ref() != Some(&candidate.result)
        || source_pin.output.as_ref() != Some(&candidate.artifact)
    {
        bail!("verification `{verification}` did not review the exact candidate artifact");
    }
    Ok(())
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
