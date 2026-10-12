//! An interaction's checkout diffed against the commit its branch was made
//! at: what `d` shows inside Styra, in the view `styra-diff` provides, and
//! what `D` hands to the configured tool instead (see
//! [`crate::config::Configuration::open_diff`]).

use styra_diff::DiffTarget;
use styra_protocol::InteractionSummary;

/// The diff `interaction` has to offer, as the server last reported it, or
/// why it has none. One outside a repository has no checkout, and one Styra
/// made no branch for has nowhere recorded to measure from; neither is
/// guessed at.
pub fn target(interaction: &InteractionSummary) -> Result<DiffTarget, &'static str> {
    let checkout = interaction
        .checkout
        .as_ref()
        .ok_or("this interaction has no Git checkout to diff")?;
    let branched_from = interaction
        .branched_from
        .as_ref()
        .ok_or("no recorded start commit to diff this checkout against")?;
    Ok(DiffTarget {
        worktree: checkout.worktree.clone(),
        branch: checkout.branch.clone(),
        base_branch: branched_from.branch.clone(),
        base: branched_from.commit.clone(),
        branch_point: true,
    })
}
