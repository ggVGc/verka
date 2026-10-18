//! An interaction's checkout diffed against its first recorded snapshot.

use styra_diff::DiffTarget;
use styra_protocol::InteractionSummary;
use styra_server::Client;

/// Resolve the durable baseline when diff mode opens, including any work
/// that was uncommitted when the first snapshot was recorded.
pub fn target(client: &Client, interaction: &InteractionSummary) -> anyhow::Result<DiffTarget> {
    let checkout = interaction
        .checkout
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("this interaction has no Git checkout to diff"))?;
    let base = client.snapshot_base(&interaction.id)?.ok_or_else(|| {
        anyhow::anyhow!("this interaction has no recorded snapshot to diff against")
    })?;
    Ok(DiffTarget {
        worktree: checkout.worktree.clone(),
        branch: checkout.branch.clone(),
        base_branch: Some("first snapshot".into()),
        base,
        branch_point: false,
    })
}
