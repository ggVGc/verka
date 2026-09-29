//! Node definitions (`node.toml` plus `description.md`) and attachments.

use super::{CandidateId, NodeId};
use serde::{Deserialize, Serialize};

pub const DEFINITION_SCHEMA: u32 = 1;
pub const ATTACHMENT_SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Human,
    Machine,
}
impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Machine => "machine",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepKind {
    DependsOn,
    DerivedFrom,
}
impl DepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on",
            Self::DerivedFrom => "derived_from",
        }
    }
}

/// Contents of `node.toml`. Dependencies are *ids only*: which versions the
/// work was actually built against is a fact about the work, recorded in the
/// result's consumed pins at completion, so that updating a pin never counts
/// as a definition change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeMeta {
    pub schema: u32,
    pub author: Author,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Author>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<NodeId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<NodeId>,
    /// Exact candidate whose output this review node verifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifies: Option<CandidateId>,
    /// Namespaced application metadata (e.g. from an execution harness) is
    /// preserved but never interpreted here.
    #[serde(default, flatten)]
    pub extensions: std::collections::BTreeMap<String, toml::Value>,
}

/// A definition's version: the Git blob ids of `node.toml` and `description.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionVersion {
    pub metadata: String,
    pub description: String,
}

/// Metadata for opaque data associated with a node.
///
/// Attachments are deliberately outside the node definition and result: Linka
/// stores and versions their bytes, but never uses them to derive graph state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAttachment {
    pub schema: u32,
    /// Application-owned namespace (for example `orka`).
    pub namespace: String,
    /// Stable identity within the namespace.
    pub key: String,
    /// Unix milliseconds when the attachment was first recorded.
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Git blob identity of the payload bytes.
    pub content: String,
    pub size: u64,
}

/// Caller-supplied data for one immutable node attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewNodeAttachment {
    pub namespace: String,
    pub key: String,
    pub media_type: Option<String>,
    pub data: Vec<u8>,
}

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
    use super::*;

    #[test]
    fn title_is_the_first_non_empty_line_of_the_description() {
        assert_eq!(title_of("Parse config\n\nDetails follow."), "Parse config");
        assert_eq!(title_of("\n  \n  Leading blanks\nrest"), "Leading blanks");
        assert_eq!(title_of("one-liner"), "one-liner");
        assert_eq!(title_of(""), "(no description)");
        assert_eq!(title_of("  \n\t\n"), "(no description)");
    }
}
