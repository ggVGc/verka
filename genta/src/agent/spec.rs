//! The per-provider fact table, and the behaviour a fact cannot express.
//!
//! Every provider-specific constant lives in one [`ProviderSpec`] literal in
//! that provider's own module, rather than as one arm in each of a dozen
//! matches on [`Provider`](super::Provider). The compiler's exhaustiveness
//! guarantee moves with it rather than being given up: a new provider fails the
//! single match in `Provider::spec`, and a new *field* fails every literal that
//! omits it.
//!
//! That second half is why no literal may be written with struct-update syntax
//! (`..OTHER_SPEC`) and why this struct has no `Default`: either one is exactly
//! the hole worth avoiding, since a field added later would silently inherit
//! some other provider's answer instead of failing to compile. Values two
//! providers genuinely share are shared by naming a const, so the sharing is
//! visible at both sites and every provider still states every field.

use super::{Effort, Profile, SandboxLayout};
use crate::event::Protocol;
use anyhow::Result;
use std::path::Path;

/// One model a provider offers, and the reasoning-effort rungs it accepts.
///
/// The effort ladder is a per-model property, not a per-provider one: a picker
/// built on one ladder for the whole agent offers rungs a given model rejects —
/// `xhigh` on Claude Opus 4.6 (that rung arrived with 4.7), `max` on GPT-5.5,
/// an effort of any kind on Haiku 4.5. So each catalog entry states its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// The id passed to the agent and recorded in a [`Selection`](super::Selection).
    pub id: &'static str,
    /// Other ids this entry speaks for: an undated alias of a dated id, or a
    /// model kept out of the picker that shares this one's ladder. Recognised,
    /// never offered.
    pub aliases: &'static [&'static str],
    /// The rungs this model accepts, lowest first. Empty means the model takes
    /// no effort setting at all, which is a different thing from a short
    /// ladder.
    pub efforts: &'static [Effort],
}

impl ModelSpec {
    /// Whether `model` names this entry, by its id or one of its aliases.
    pub fn answers_to(&self, model: &str) -> bool {
        self.id == model || self.aliases.contains(&model)
    }
}

/// Everything one provider declares about itself.
///
/// Reached only through `Provider::spec`; the accessors on
/// [`Provider`](super::Provider) are the public shape of these fields, so the
/// struct itself stays crate-private. This is the one catalog of models and
/// effort ladders: hosts keep only their own policy (styra's choice of which
/// providers are interactive, say) and read the catalog from here.
pub(crate) struct ProviderSpec {
    /// The provider's name in a profile string; see
    /// [`Selection`](super::Selection).
    pub name: &'static str,
    /// The agent's own executable name, as located on the host's `PATH`.
    pub executable: &'static str,
    /// The wire protocol this provider speaks, and thus the decoder and
    /// presentation rules its journal is read with.
    pub protocol: Protocol,
    /// Models worth offering in a picker, most capable first, each with its own
    /// effort ladder.
    pub models: &'static [ModelSpec],
    /// The provider's widest effort ladder, lowest first: every listed model's
    /// ladder lies within it, and a model the catalog does not list is assumed
    /// to have all of it.
    pub efforts: &'static [Effort],
    /// The model an unpinned selection takes.
    pub default_model: &'static str,
    /// The reasoning effort an unpinned selection takes.
    pub default_effort: Effort,
    /// The least expensive model that takes an effort setting, for incidental
    /// one-shot errands.
    pub cheapest_model: &'static str,
    /// The parts that build or mutate rather than state a constant.
    pub ops: &'static dyn ProviderOps,
}

/// The provider behaviour a constant cannot hold: building the launchable
/// profile, and reopening an existing conversation.
///
/// A private trait behind the enum, in the shape
/// [`Protocol::presenter`](crate::event::Protocol) already uses for
/// presentation. It is reached only through `Provider::spec`, never named in
/// the public API, so none of the reasons the provider set is an enum are
/// given up: `Provider` stays a `Copy`, serializable, matchable value that
/// downstream crates can extend with their own operations.
pub(crate) trait ProviderOps: Sync {
    /// Build the launchable profile for `model` at `effort`, with the agent
    /// binary already located on the host's `PATH`.
    fn profile(
        &self,
        layout: &SandboxLayout,
        executable: &Path,
        model: &str,
        effort: Effort,
    ) -> Profile;

    /// Reopen an existing provider conversation, by whatever mechanism this
    /// agent offers — a launch flag, a protocol handshake the host performs
    /// afterwards, or not at all. `provider_session_id` is already known to be
    /// non-empty.
    fn resume(&self, profile: &mut Profile, provider_session_id: &str) -> Result<()>;
}
