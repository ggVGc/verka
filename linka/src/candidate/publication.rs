use super::*;
use anyhow::{bail, Result};

impl CandidateStore<'_> {
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
}
