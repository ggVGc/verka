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
    if !PROVIDERS.contains(&selection.provider) {
        anyhow::bail!(
            "agent provider {:?} is not interactive; Styra supports: {}",
            selection.provider.as_str(),
            PROVIDERS.map(|provider| provider.as_str()).join(", ")
        );
    }
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
mod tests {
    use super::*;

    #[test]
    fn styra_only_accepts_interactive_providers() {
        validate_selection(&Selection::new(Provider::Codex)).unwrap();
        validate_selection(&Selection::new(Provider::Claude)).unwrap();
        let error = validate_selection(&Selection::new(Provider::CodexExec)).unwrap_err();
        assert!(error.to_string().contains("not interactive"));
    }

    /// A rung one model has and another does not is accepted on the first and
    /// refused on the second, under the same provider, and the refusal names
    /// the model.
    #[test]
    fn an_effort_is_judged_against_the_model_not_the_agent() {
        validate_selection(&Selection::parse("claude:claude-opus-5/xhigh").unwrap()).unwrap();
        let error = validate_selection(&Selection::parse("claude:claude-opus-4-6/xhigh").unwrap())
            .unwrap_err();
        assert!(
            error.to_string().contains("claude-opus-4-6"),
            "the error names the model that refused the rung: {error}"
        );

        validate_selection(&Selection::parse("codex:gpt-5.6-sol/max").unwrap()).unwrap();
        assert!(validate_selection(&Selection::parse("codex:gpt-5.5/max").unwrap()).is_err());
    }

    /// Every model a picker can reach, at every rung of its ladder, validates —
    /// so no reachable row of the picker names a launch Styra would refuse.
    #[test]
    fn every_offered_model_and_rung_is_launchable() {
        for provider in PROVIDERS {
            for entry in provider.models() {
                for effort in entry.efforts {
                    validate_selection(&Selection {
                        provider,
                        model: entry.id.to_owned(),
                        effort: *effort,
                    })
                    .unwrap();
                }
            }
        }
    }

    /// A model that takes no effort has nothing to validate, so a selection
    /// carrying the placeholder is not an error.
    #[test]
    fn a_model_without_an_effort_parameter_validates_with_its_placeholder() {
        for model in ["claude-sonnet-4-5-20250929", "claude-haiku-4-5-20251001"] {
            validate_selection(&Selection {
                provider: Provider::Claude,
                model: model.to_owned(),
                effort: Provider::Claude.default_effort_for(model),
            })
            .unwrap();
        }
    }

    /// An id newer than the catalog is under-constrained rather than refused.
    #[test]
    fn an_unknown_model_validates_against_the_widest_ladder() {
        validate_selection(&Selection::parse("codex:gpt-7-nova/max").unwrap()).unwrap();
    }
}
