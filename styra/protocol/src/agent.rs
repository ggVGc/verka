//! Styra's view of Genta's agent vocabulary.
//!
//! Genta owns what an agent *is*: the command line, the wire protocol, the
//! [`Selection`] that pins a provider, a model, and a reasoning effort. What
//! belongs here is the part that is a question about the agents' own catalogs
//! rather than about launching them — which providers Styra offers
//! interactively, which models are worth putting in a picker, and, per model,
//! which reasoning-effort rungs that model actually accepts.
//!
//! The effort ladder is a per-model property, not a per-provider one, which is
//! the whole reason these functions exist. Genta's [`Provider::efforts`] states
//! one ladder for the whole agent, so a picker built on it offers rungs that a
//! given model rejects — `xhigh` on Claude Opus 4.6 (that rung arrived with
//! 4.7), `max` on GPT-5.5, an effort of any kind on Haiku 4.5. Every function
//! here therefore takes the model as well as the provider.
//!
//! Catalogs are not closed sets: both agents accept any model id they know, and
//! a [`Selection`] still carries a free-form string. A model none of these
//! tables list falls back to the provider's widest ladder, so an id newer than
//! this file is under-constrained rather than rejected.

pub use genta::agent::*;

/// The interactive providers Styra can launch, in picker order.
pub const PROVIDERS: [Provider; 2] = [Provider::Codex, Provider::Claude];

/// Models worth offering in a picker, most capable first.
///
/// The codex ids are the models the installed codex catalog marks
/// `visibility: list` (read out of codex-cli 0.156.1 on 2026-09-24), newest
/// tier first. The hidden ones are deliberately absent: `gpt-5.4` and
/// `codex-auto-review` are not offered to operators at all, and the
/// `gpt-daybreak-*` pair are cyber-specialty variants rather than general
/// coding models.
///
/// The Claude ids are every model listed `Active` in Anthropic's model-status
/// table, read on 2026-09-24, tier by tier, newest first. The `claude-mythos-*`
/// tier is knowingly excluded: it is reachable only through Project Glasswing,
/// so offering it to every operator would suggest an agent most cannot launch.
/// Full ids rather than the `opus`/`sonnet` aliases, so a journal records the
/// exact model a session ran on even after an alias moves to a newer release.
///
/// A superset of [`Provider::models`]: Genta's catalog is what its own defaults
/// are drawn from, and this is what Styra shows. Both are advisory — see the
/// module docs on closed sets.
pub fn models_for(provider: Provider) -> &'static [&'static str] {
    match provider {
        Provider::Codex | Provider::CodexExec => &[
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
        ],
        Provider::Claude => &[
            "claude-fable-5-1",
            "claude-fable-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-opus-4-5-20251101",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5-20250929",
            "claude-haiku-4-5-20251001",
        ],
    }
}

/// The current codex ladder, as every `gpt-6-*` and `gpt-5.6-*` model declares
/// it.
///
/// Incomplete in one known way: the catalog gives `gpt-6-astra`, `gpt-6-sol`,
/// `gpt-5.6-sol`, and `gpt-5.6-terra` a sixth rung above `max`, `ultra`, which
/// [`Effort`] cannot spell. Adding `Effort::Ultra` in Genta and a second
/// constant here is all that is missing; until then those four models are
/// offered one rung short rather than offered a rung they would reject.
const CODEX_LADDER: &[Effort] = &[
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

/// The ladder the legacy codex models kept: no `max`.
const CODEX_LEGACY_LADDER: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High, Effort::XHigh];

/// The full Claude ladder, as every model from Opus 4.7 onwards accepts it.
const CLAUDE_LADDER: &[Effort] = &[
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

/// The 4.6-generation Claude ladder. `xhigh` arrived with Opus 4.7 and sits
/// between `high` and `max`, so these models have the two ends but not it.
const CLAUDE_LADDER_WITHOUT_XHIGH: &[Effort] =
    &[Effort::Low, Effort::Medium, Effort::High, Effort::Max];

/// The 4.5-generation Claude ladder, from before `xhigh` and `max`.
const CLAUDE_LADDER_CLASSIC: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High];

/// The reasoning-effort rungs `model` accepts under `provider`, lowest first.
///
/// Empty means the model takes no effort setting at all — Claude Sonnet 4.5 and
/// Haiku 4.5 predate the parameter and reject it — which is a different thing
/// from a short ladder and is why this returns a slice rather than a range. See
/// [`supports_effort`].
///
/// A model outside the tables gets the provider's widest ladder: an unknown id
/// is nobody's in particular, and the agent itself is the authority that will
/// reject a rung it does not have.
pub fn efforts_for(provider: Provider, model: &str) -> &'static [Effort] {
    match provider {
        Provider::Codex | Provider::CodexExec => match model {
            "gpt-5.5" | "gpt-5.4" => CODEX_LEGACY_LADDER,
            _ => CODEX_LADDER,
        },
        Provider::Claude => match model {
            "claude-opus-4-6" | "claude-sonnet-4-6" => CLAUDE_LADDER_WITHOUT_XHIGH,
            "claude-opus-4-5-20251101" | "claude-opus-4-5" => CLAUDE_LADDER_CLASSIC,
            "claude-sonnet-4-5-20250929"
            | "claude-sonnet-4-5"
            | "claude-haiku-4-5-20251001"
            | "claude-haiku-4-5" => &[],
            _ => CLAUDE_LADDER,
        },
    }
}

/// Whether `model` takes a reasoning effort at all.
pub fn supports_effort(provider: Provider, model: &str) -> bool {
    !efforts_for(provider, model).is_empty()
}

/// Where `effort` sits on the shared ladder, lowest first. The vocabulary is one
/// ordered scale across providers; [`efforts_for`] is what narrows it.
fn rank(effort: Effort) -> u8 {
    match effort {
        Effort::Minimal => 0,
        Effort::Low => 1,
        Effort::Medium => 2,
        Effort::High => 3,
        Effort::XHigh => 4,
        Effort::Max => 5,
    }
}

/// The effort a launch of `model` takes when nothing named one.
///
/// The provider's declared default ([`Provider::default_effort`]) where the
/// model has that rung; otherwise the highest rung below it, so a model with a
/// shorter ladder is stepped down rather than pushed to an end of the scale it
/// did not ask for. A model that takes no effort at all still needs a value to
/// put in a [`Selection`], and the provider default is that placeholder.
pub fn default_effort_for(provider: Provider, model: &str) -> Effort {
    let declared = provider.default_effort();
    let efforts = efforts_for(provider, model);
    if efforts.is_empty() || efforts.contains(&declared) {
        return declared;
    }
    efforts
        .iter()
        .copied()
        .rfind(|effort| rank(*effort) < rank(declared))
        .or_else(|| efforts.last().copied())
        .unwrap_or(declared)
}

/// The least expensive model to put an errand on — a sentence of text in and a
/// few words out ([`Errand`](../../styra_server/errand/struct.Errand.html)).
///
/// Deliberately not the cheapest model the agent runs. Claude's is Haiku 4.5,
/// which takes no effort setting, and every Styra launch pins one; until a
/// launch can omit the flag, the cheapest model that can be launched *correctly*
/// is the one to use.
pub fn cheapest_model_for(provider: Provider) -> &'static str {
    let declared = provider.cheapest_model();
    if supports_effort(provider, declared) {
        return declared;
    }
    match provider {
        Provider::Codex | Provider::CodexExec => "gpt-5.6-luna",
        Provider::Claude => "claude-sonnet-5",
    }
}

/// The lowest effort `model` accepts, which is what an errand asks for.
///
/// [`efforts_for`] is ordered lowest first, so a ladder that gains a rung below
/// the current floor moves this with it. A model with no ladder falls back to
/// its default placeholder — but [`cheapest_model_for`] does not pick one.
pub fn cheapest_effort_for(provider: Provider, model: &str) -> Effort {
    efforts_for(provider, model)
        .first()
        .copied()
        .unwrap_or_else(|| default_effort_for(provider, model))
}

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
    let efforts = efforts_for(selection.provider, model);
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

    /// The point of a per-model ladder: a rung one model has and another does
    /// not is accepted on the first and refused on the second, under the same
    /// provider.
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

    /// Genta's `minimal` rung is not on any current codex model, so nothing may
    /// launch on it — including the errand path, which used to take it as the
    /// cheapest rung there is.
    #[test]
    fn codex_no_longer_offers_a_minimal_effort() {
        for model in models_for(Provider::Codex) {
            assert!(
                !efforts_for(Provider::Codex, model).contains(&Effort::Minimal),
                "{model} was offered the minimal rung"
            );
        }
        assert_eq!(
            cheapest_effort_for(Provider::Codex, "gpt-5.6-luna"),
            Effort::Low
        );
    }

    /// Every model a picker can reach states a ladder, and every rung of it
    /// validates — so no reachable row of the picker names a launch the agent
    /// would reject.
    #[test]
    fn every_offered_model_and_rung_is_launchable() {
        for provider in PROVIDERS {
            for model in models_for(provider) {
                let efforts = efforts_for(provider, model);
                for effort in efforts {
                    validate_selection(&Selection {
                        provider,
                        model: (*model).to_owned(),
                        effort: *effort,
                    })
                    .unwrap();
                }
                // And the default the picker opens on is one of those rungs.
                let default = default_effort_for(provider, model);
                assert!(
                    efforts.is_empty() || efforts.contains(&default),
                    "{provider:?} {model} opens on a rung it does not have"
                );
            }
        }
    }

    /// A shorter ladder is stepped down to, not jumped past: Opus 4.5 has no
    /// `xhigh`, so Claude's declared `high` default still stands, while a model
    /// whose ladder stops below the default takes the highest rung it has.
    #[test]
    fn a_default_steps_down_to_the_nearest_rung_the_model_has() {
        assert_eq!(
            default_effort_for(Provider::Claude, "claude-opus-4-5-20251101"),
            Effort::High
        );
        assert_eq!(
            default_effort_for(Provider::Claude, "claude-opus-5"),
            Provider::Claude.default_effort()
        );
        assert_eq!(
            default_effort_for(Provider::Codex, "gpt-5.5"),
            Provider::Codex.default_effort()
        );
    }

    /// The two Claude models that predate the effort parameter are offered, but
    /// nothing may put an effort on them — least of all the errand path, which
    /// would otherwise take Haiku as its cheapest model.
    #[test]
    fn the_models_without_an_effort_parameter_are_never_given_one() {
        for model in ["claude-sonnet-4-5-20250929", "claude-haiku-4-5-20251001"] {
            assert!(!supports_effort(Provider::Claude, model), "{model}");
            // Nothing to validate against, so a selection carrying the
            // placeholder is not an error.
            validate_selection(&Selection {
                provider: Provider::Claude,
                model: model.to_owned(),
                effort: default_effort_for(Provider::Claude, model),
            })
            .unwrap();
        }
        assert!(supports_effort(
            Provider::Claude,
            cheapest_model_for(Provider::Claude)
        ));
        assert!(supports_effort(
            Provider::Codex,
            cheapest_model_for(Provider::Codex)
        ));
    }

    /// An id newer than these tables is under-constrained rather than refused:
    /// the agent itself is the authority on its own catalog.
    #[test]
    fn an_unknown_model_falls_back_to_the_widest_ladder() {
        assert_eq!(
            efforts_for(Provider::Claude, "claude-opus-9"),
            CLAUDE_LADDER
        );
        validate_selection(&Selection::parse("codex:gpt-7-nova/max").unwrap()).unwrap();
    }
}
