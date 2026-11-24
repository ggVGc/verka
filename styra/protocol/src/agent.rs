//! Styra's view of Genta's agent vocabulary.
//!
//! Genta owns what an agent *is* — the command line, the wire protocol, the
//! [`Selection`] that pins a provider, a model, and a reasoning effort — and
//! what each agent offers: the model catalog and every model's own effort
//! ladder ([`Provider::models`], [`Provider::efforts_for`]). Styra reads those
//! rather than keeping a copy. What belongs here is Styra's own policy: which
//! providers it launches interactively, and what makes a selection launchable
//! by it.

pub use genta::agent::*;

/// The interactive providers Styra can launch, in picker order.
pub const PROVIDERS: [Provider; 2] = [Provider::Codex, Provider::Claude];

/// Validate an interactive Styra launch selection.
pub fn validate_selection(selection: &Selection) -> anyhow::Result<()> {
    let model = selection.model.trim();
    if model.is_empty() {
        anyhow::bail!("the agent model cannot be empty");
    }
    let efforts = selection.provider.efforts_for(model);
    // A model that takes no effort setting has nothing to validate: the
    // selection's effort is a placeholder no launch should be passing on.
    if !efforts.is_empty() && !efforts.contains(&selection.effort) {
        anyhow::bail!(
            "reasoning effort {:?} is not supported by {} {}; it accepts: {}",
            selection.effort.as_str(),
            selection.provider.as_str(),
            model,
            efforts
                .iter()
                .map(|effort| effort.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
