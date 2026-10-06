//! Derived node state: recorded outcome, currency, staleness, blockers, and
//! integration. None of this is stored.

use super::{NodeId, Outcome, ProjectPath, ResultOutcome, VerificationOutcome};
use serde::Serialize;

/// The result evidence currently recorded for a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedOutcome {
    Open,
    Succeeded,
    Failed,
    Accepted,
    Rejected,
    Abandoned,
}

impl From<ResultOutcome> for RecordedOutcome {
    fn from(outcome: ResultOutcome) -> Self {
        match outcome {
            ResultOutcome::Work(Outcome::Done) => Self::Succeeded,
            ResultOutcome::Work(Outcome::Failed) => Self::Failed,
            ResultOutcome::Verification(VerificationOutcome::Accepted) => Self::Accepted,
            ResultOutcome::Verification(VerificationOutcome::Rejected) => Self::Rejected,
            ResultOutcome::Verification(VerificationOutcome::Abandoned) => Self::Abandoned,
        }
    }
}

/// Whether recorded evidence still covers the current graph and project facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Current,
    Stale,
}

/// A machine-readable reason that recorded evidence is stale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StalenessReason {
    DefinitionChanged { metadata: bool, description: bool },
    ConsumedDefinitionChanged { id: NodeId },
    ConsumedNodeMissing { id: NodeId },
    ConsumedResultChanged { id: NodeId },
    ConsumedOutputChanged { id: NodeId },
    ContextChanged { path: ProjectPath },
    ContextMissing { path: ProjectPath },
    OutputDrifted { artifact: String, detail: String },
}

/// The wording of a staleness reason is part of the graph vocabulary, not of
/// any one interface: every reader — CLI, web, TUI — says the same thing about
/// the same fact. `OutputDrifted` embeds a multi-line detail verbatim; a caller
/// that indents its output re-indents the rendered lines itself.
impl std::fmt::Display for StalenessReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefinitionChanged {
                metadata,
                description,
            } => {
                let mut files = Vec::new();
                if *metadata {
                    files.push("node.toml");
                }
                if *description {
                    files.push("description.md");
                }
                write!(
                    f,
                    "definition changed since the work ({})",
                    files.join(", ")
                )
            }
            Self::ConsumedDefinitionChanged { id } => {
                write!(f, "dependency {id}: definition moved")
            }
            Self::ConsumedNodeMissing { id } => write!(f, "dependency {id}: missing"),
            Self::ConsumedResultChanged { id } => {
                write!(f, "dependency {id}: result changed since it was consumed")
            }
            Self::ConsumedOutputChanged { id } => write!(f, "dependency {id}: output changed"),
            Self::ContextChanged { path } => write!(f, "context {path}: content changed"),
            Self::ContextMissing { path } => write!(f, "context {path}: missing"),
            Self::OutputDrifted { artifact, detail } => {
                write!(f, "output changed since {artifact}:\n{detail}")
            }
        }
    }
}

/// Why one required dependency does not satisfy a node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerReason {
    Missing,
    Open,
    Failed,
    Rejected,
    Abandoned,
    Stale,
    AwaitingIntegration,
}

impl BlockerReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Open => "not complete (open)",
            Self::Failed => "not complete (failed)",
            Self::Rejected => "review rejected",
            Self::Abandoned => "review abandoned",
            Self::Stale => "not complete (stale)",
            Self::AwaitingIntegration => "awaiting candidate integration",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub id: NodeId,
    pub reason: BlockerReason,
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.id, self.reason.as_str())
    }
}

/// The complete derived state of one node at a point in time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeState {
    pub outcome: RecordedOutcome,
    pub currency: Currency,
    pub integration: IntegrationStatus,
    pub staleness: Vec<StalenessReason>,
    pub blockers: Vec<Blocker>,
}

/// Whether the current successful result must be, or has been, integrated
/// into its candidate's target branch. Direct results do not require a
/// separate integration step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    NotRequired,
    Pending,
    Accepted,
    Published,
    Rejected,
}

/// Presentation-neutral precedence for the current derived node state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateClass {
    Complete,
    Ready,
    Blocked,
    AwaitingIntegration,
    Accepted,
    Rejected,
    Abandoned,
}

impl IntegrationStatus {
    pub fn is_done(self) -> bool {
        match self {
            Self::NotRequired => true,
            Self::Pending => false,
            Self::Accepted => false,
            Self::Published => true,
            Self::Rejected => true,
        }
    }
}

impl NodeState {
    pub fn classification(&self) -> StateClass {
        if self.currency == Currency::Current {
            match self.outcome {
                RecordedOutcome::Accepted => return StateClass::Accepted,
                RecordedOutcome::Rejected => return StateClass::Rejected,
                RecordedOutcome::Abandoned => return StateClass::Abandoned,
                _ => {}
            }
        }
        if self.is_complete() {
            StateClass::Complete
        } else if self.is_awaiting_integration() {
            StateClass::AwaitingIntegration
        } else if self.is_ready() {
            StateClass::Ready
        } else {
            StateClass::Blocked
        }
    }
    pub fn is_complete(&self) -> bool {
        self.currency == Currency::Current
            && match self.outcome {
                RecordedOutcome::Accepted
                | RecordedOutcome::Rejected
                | RecordedOutcome::Abandoned => true,
                RecordedOutcome::Succeeded => matches!(
                    self.integration,
                    IntegrationStatus::NotRequired | IntegrationStatus::Published
                ),
                RecordedOutcome::Open | RecordedOutcome::Failed => false,
            }
    }

    pub fn is_ready(&self) -> bool {
        !self.is_complete() && self.blockers.is_empty() && self.integration.is_done()
    }

    pub fn is_awaiting_integration(&self) -> bool {
        !self.integration.is_done()
    }

    pub fn is_blocked(&self) -> bool {
        !self.is_complete() && !self.blockers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_reasons_render_the_same_words_for_every_reader() {
        assert_eq!(
            StalenessReason::ContextChanged {
                path: "src/lib.rs".parse().unwrap(),
            }
            .to_string(),
            "context src/lib.rs: content changed"
        );
        assert_eq!(
            StalenessReason::DefinitionChanged {
                metadata: true,
                description: true,
            }
            .to_string(),
            "definition changed since the work (node.toml, description.md)"
        );
        assert_eq!(
            StalenessReason::OutputDrifted {
                artifact: "artifact-1".into(),
                detail: "first\nsecond".into(),
            }
            .to_string(),
            "output changed since artifact-1:\nfirst\nsecond"
        );
        assert_eq!(
            Blocker {
                id: "node-dependency".parse().unwrap(),
                reason: BlockerReason::Stale,
            }
            .to_string(),
            "node-dependency: not complete (stale)"
        );
        assert_eq!(
            Blocker {
                id: "node-candidate".parse().unwrap(),
                reason: BlockerReason::AwaitingIntegration,
            }
            .to_string(),
            "node-candidate: awaiting candidate integration"
        );
    }

    #[test]
    fn integration_is_done_after_publication_or_rejection() {
        assert!(IntegrationStatus::NotRequired.is_done());
        assert!(!IntegrationStatus::Pending.is_done());
        assert!(!IntegrationStatus::Accepted.is_done());
        assert!(IntegrationStatus::Published.is_done());
        assert!(IntegrationStatus::Rejected.is_done());
    }

    #[test]
    fn state_classification_has_one_frontend_precedence() {
        let state = |outcome, currency, integration, blocked: bool| NodeState {
            outcome,
            currency,
            integration,
            staleness: Vec::new(),
            blockers: blocked
                .then(|| Blocker {
                    id: "dependency".parse().unwrap(),
                    reason: BlockerReason::Open,
                })
                .into_iter()
                .collect(),
        };
        use IntegrationStatus as I;
        use RecordedOutcome as O;
        assert_eq!(
            state(O::Succeeded, Currency::Current, I::Published, false).classification(),
            StateClass::Complete
        );
        assert_eq!(
            state(O::Succeeded, Currency::Current, I::Pending, false).classification(),
            StateClass::AwaitingIntegration
        );
        assert_eq!(
            state(O::Succeeded, Currency::Stale, I::Rejected, false).classification(),
            StateClass::Ready
        );
        assert_eq!(
            state(O::Failed, Currency::Current, I::NotRequired, true).classification(),
            StateClass::Blocked
        );
        assert_eq!(
            state(O::Accepted, Currency::Current, I::NotRequired, false).classification(),
            StateClass::Accepted
        );
        assert_eq!(
            state(O::Rejected, Currency::Current, I::NotRequired, false).classification(),
            StateClass::Rejected
        );
        assert_eq!(
            state(O::Abandoned, Currency::Current, I::NotRequired, false).classification(),
            StateClass::Abandoned
        );
    }
}
