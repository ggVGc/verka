//! On-disk data types.
//!
//! A node separates structured data from prose: `node.toml` and
//! `description.md` form its definition, while `result.toml` and the optional
//! `result.md` form its completion record.
//!
//! Status is never stored. It is derived from whether `result.toml` exists,
//! what its `outcome` says, and whether its definition version still matches.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
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
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
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

/// A result's version: the Git blob ids of `result.toml` and `result.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultVersion {
    pub metadata: String,
    pub notes: Option<String>,
}

/// A reference to content in an artifact system (for git: a commit).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub scheme: String,
    pub repository: String,
    pub id: String,
}

/// A dependency pinned at completion time: which definition and result of it
/// the work was built against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumedNode {
    pub id: String,
    pub definition: DefinitionVersion,
    pub result: Option<ResultVersion>,
    pub output: Option<ArtifactRef>,
}

/// A consumed file that is no node's output, pinned by content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPin {
    pub path: String,
    pub identity: String,
    #[serde(default)]
    pub observed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
}
impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Namespaced evidence about what produced a result. Written by external
/// harnesses (e.g. an execution driver); preserved but never interpreted here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProducerEvidence {
    pub namespace: String,
    pub data: serde_json::Value,
}

/// Contents of `result.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultMeta {
    /// Unix milliseconds when the result was recorded.
    pub at: i64,
    /// Who recorded the result.
    pub author: Author,
    pub definition: DefinitionVersion,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed: Vec<ConsumedNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerEvidence>,
}

/// A node's derived status — never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    Done,
    Failed,
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Derive a node's status from its current definition version and its result.
/// `done` holds only while the result's definition version still matches:
/// editing the definition after completion reopens the node.
pub fn status(current: &DefinitionVersion, result: Option<&ResultMeta>) -> Status {
    match result {
        None => Status::Open,
        Some(result) if result.outcome == Outcome::Failed => Status::Failed,
        Some(result) if &result.definition == current => Status::Done,
        Some(_) => Status::Open,
    }
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

    #[test]
    fn status_follows_outcome_and_definition_version() {
        let version = DefinitionVersion {
            metadata: "m".into(),
            description: "d".into(),
        };
        let result = ResultMeta {
            at: 0,
            author: Author::Human,
            definition: version.clone(),
            outcome: Outcome::Done,
            consumed: vec![],
            context: vec![],
            output: None,
            producer: None,
        };
        assert_eq!(status(&version, None), Status::Open);
        assert_eq!(status(&version, Some(&result)), Status::Done);
        let moved = DefinitionVersion {
            metadata: "m2".into(),
            description: "d".into(),
        };
        assert_eq!(status(&moved, Some(&result)), Status::Open);
        let failed = ResultMeta {
            outcome: Outcome::Failed,
            ..result
        };
        assert_eq!(status(&version, Some(&failed)), Status::Failed);
    }
}
