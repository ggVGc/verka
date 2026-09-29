//! Completion records (`result.toml` plus optional `result.md`), their
//! outcomes, and the pins recording what the work was built against.

use super::{Author, DefinitionVersion, NodeId, ProjectPath};
use serde::{Deserialize, Serialize};

pub const RESULT_SCHEMA: u32 = 1;
pub const OBSERVATION_SCHEMA: u32 = 1;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub scheme: String,
    pub repository: String,
    pub revision: String,
    pub tree: String,
}

/// A dependency pinned at completion time: which definition and result of it
/// the work was built against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsumedNode {
    pub id: NodeId,
    pub definition: DefinitionVersion,
    pub result: Option<ResultVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ResultOutcome>,
    pub output: Option<ArtifactRef>,
}

/// A consumed file that is no node's output, pinned by content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextPin {
    pub path: ProjectPath,
    pub identity: String,
    pub observed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextObservation {
    pub schema: u32,
    pub result: ResultVersion,
    pub context: Vec<ContextPin>,
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

/// The conclusion of reviewing an exact candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    Accepted,
    Rejected,
    Abandoned,
}
impl VerificationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Abandoned => "abandoned",
        }
    }
}

/// A result has the outcome kind required by its node definition.
///
/// This remains a single `outcome = "..."` value on disk, while the Rust type
/// keeps work completion and review conclusions distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultOutcome {
    Work(Outcome),
    Verification(VerificationOutcome),
}
impl ResultOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Work(outcome) => outcome.as_str(),
            Self::Verification(outcome) => outcome.as_str(),
        }
    }
}
impl From<Outcome> for ResultOutcome {
    fn from(value: Outcome) -> Self {
        Self::Work(value)
    }
}
impl From<VerificationOutcome> for ResultOutcome {
    fn from(value: VerificationOutcome) -> Self {
        Self::Verification(value)
    }
}
impl Serialize for ResultOutcome {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for ResultOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "done" => Ok(Self::Work(Outcome::Done)),
            "failed" => Ok(Self::Work(Outcome::Failed)),
            "accepted" => Ok(Self::Verification(VerificationOutcome::Accepted)),
            "rejected" => Ok(Self::Verification(VerificationOutcome::Rejected)),
            "abandoned" => Ok(Self::Verification(VerificationOutcome::Abandoned)),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["done", "failed", "accepted", "rejected", "abandoned"],
            )),
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
    pub schema: u32,
    /// Unix milliseconds when the result was recorded.
    pub at: i64,
    /// Who recorded the result.
    pub author: Author,
    pub definition: DefinitionVersion,
    pub outcome: ResultOutcome,
    pub project: ProjectSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed: Vec<ConsumedNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerEvidence>,
}
