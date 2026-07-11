//! On-disk data types, re-exported from the owning application crates.
//!
//! A node separates structured data from prose: `node.toml` and
//! `description.md` form its definition, while `result.toml` and the optional
//! `result.md` form its completion record.
//!
//! Status is never stored. It is derived from whether `result.toml` exists,
//! what its `outcome` says, and whether its definition version still matches.

pub use llaundry_core::{
    ArtifactRef, Author, ConsumedNode, ContextPin, DefinitionVersion, DepKind, Outcome,
    ResultRecord as ResultMeta, ResultVersion, Status,
};
pub use llaundry_review::{Candidate, Decision, NodeState, PublicationIntent, ReviewDecision};
pub use llaundry_work::{Attempt, AttemptFinished, ExecutionIdentity, WorkEvidence, WorkedBy};

/// Contents of `node.toml`. Dependencies are *ids only*: which versions the
/// work was actually built against is a fact about the work, recorded in the
/// result's consumed pins at completion, so that updating a pin never counts
/// as a definition change.
pub type NodeMeta = llaundry_core::NodeDefinition;

/// A node's display title: the first non-empty line of its description. There
/// is no stored title — the description is the definition, and its opening
/// line names the node wherever a one-liner is needed.
pub fn title_of(description: &str) -> &str {
    description
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(no description)")
}

#[cfg(test)]
mod tests {
    use super::title_of;

    #[test]
    fn title_is_the_first_non_empty_line_of_the_description() {
        assert_eq!(title_of("Parse config\n\nDetails follow."), "Parse config");
        assert_eq!(title_of("\n  \n  Leading blanks\nrest"), "Leading blanks");
        assert_eq!(title_of("one-liner"), "one-liner");
        assert_eq!(title_of(""), "(no description)");
        assert_eq!(title_of("  \n\t\n"), "(no description)");
    }
}
