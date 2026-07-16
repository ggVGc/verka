use super::*;
use anyhow::{bail, Context, Result};

impl CandidateStore<'_> {
    /// Fast-forward the accepted target. Git history is the publication record,
    /// so retrying after a crash is sufficient and needs no Linka journal.
    pub fn publish(&self, vcs: &dyn Vcs, id: &CandidateId) -> Result<()> {
        let view = self.load(id)?;
        let decision = view
            .decision
            .as_ref()
            .filter(|decision| decision.kind == DecisionKind::Accepted)
            .with_context(|| format!("candidate `{id}` has not been accepted"))?;
        if view.integration(vcs)? == IntegrationStatus::Published {
            return Ok(());
        }
        self.require_candidate_ref(vcs, &view.candidate)?;
        self.require_current_candidate(vcs, &view.candidate, IntegrationStatus::Accepted)?;
        let target_ref = decision
            .target_ref
            .as_deref()
            .context("acceptance has no target")?;
        let target_previous = decision
            .target_previous
            .as_deref()
            .context("acceptance has no target baseline")?;
        if !vcs.publish_fast_forward(target_ref, target_previous, &view.candidate.artifact.id)? {
            bail!("candidate `{id}` cannot fast-forward its target branch");
        }
        Ok(())
    }
}
