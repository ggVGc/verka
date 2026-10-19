//! An interaction's checkout diffed against where its branch will be
//! integrated, or against its first recorded snapshot when it has no target.

use styra_diff::DiffTarget;
use styra_protocol::InteractionSummary;
use styra_server::Client;

/// Resolve the baseline when diff mode opens: the server's merge base with
/// the branch the checkout targets, which follows rebases, or else the first
/// snapshot, including any work that was uncommitted when it was recorded.
pub fn target(client: &Client, interaction: &InteractionSummary) -> anyhow::Result<DiffTarget> {
    let checkout = interaction
        .checkout
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("this interaction has no Git checkout to diff"))?;
    let base = client.diff_base(&interaction.id)?.ok_or_else(|| {
        anyhow::anyhow!("this interaction has no target branch or snapshot to diff against")
    })?;
    Ok(DiffTarget {
        worktree: checkout.worktree.clone(),
        branch: checkout.branch.clone(),
        branch_point: base.branch.is_some(),
        base_branch: Some(base.branch.unwrap_or_else(|| "first snapshot".into())),
        base: base.base,
    })
}
