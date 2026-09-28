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

/// Everything one provider declares about itself.
///
/// Reached only through `Provider::spec`; the accessors on
/// [`Provider`](super::Provider) are the public shape of these fields, so the
/// struct itself stays crate-private. Hosts that keep their own per-provider
/// policy (styra's model and effort catalogs, say) key it off the `Provider`
/// enum rather than extending this table, which is what keeps their policy
/// theirs.
pub(crate) struct ProviderSpec {
    /// The provider's name in a profile string; see
    /// [`Selection`](super::Selection).
    pub name: &'static str,
    /// The agent's own executable name, as located on the host's `PATH`.
    pub executable: &'static str,
    /// The wire protocol this provider speaks, and thus the decoder and
    /// presentation rules its journal is read with.
    pub protocol: Protocol,
    /// Models worth offering in a picker, most capable first.
    pub models: &'static [&'static str],
    /// The reasoning-effort levels this provider accepts, lowest first.
    pub efforts: &'static [Effort],
    /// The model an unpinned selection takes.
    pub default_model: &'static str,
    /// The reasoning effort an unpinned selection takes.
    pub default_effort: Effort,
    /// The least expensive model, for incidental one-shot errands.
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
