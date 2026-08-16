//! Human formatting for the `linka` CLI.
//!
//! Everything the CLI prints is built here, so the dispatch in `main.rs` stays
//! a thin shell over the library and the wording lives in one place. The
//! library hands back records and derived values only; every word below —
//! including the one-word names for the model's own enums and the abbreviation
//! of a hash — is this crate's choice, not the graph's.

use anyhow::Result;
use std::fmt::Write;

use linka::graph::Graph;
use linka::model::{
    Author, Blocker, BlockerReason, Candidate, CandidateDecision, Currency, DefinitionVersion,
    DepKind, IntegrationStatus, NodeState, RecordedOutcome, ResultVersion, StalenessReason,
    Unsettled, UnsettledReason, Workability,
};
use linka::{title_of, NodeId, Pairing, Store};

// --- words and abbreviations -------------------------------------------------

/// First 12 characters of a hash, for compact display.
///
/// Hashes are ASCII, but this is handed strings that came off disk — a
/// hand-edited artifact id is still something to display, not something to
/// panic on — so it cuts on a character boundary rather than a byte index.
pub fn short(hash: &str) -> &str {
    match hash.char_indices().nth(12) {
        Some((boundary, _)) => &hash[..boundary],
        None => hash,
    }
}

pub fn short_definition(version: &DefinitionVersion) -> String {
    format!(
        "{}/{}",
        short(&version.metadata),
        short(&version.description)
    )
}

pub fn short_result(version: &ResultVersion) -> String {
    format!(
        "{}/{}",
        short(&version.metadata),
        version.notes.as_deref().map_or("none", short)
    )
}

pub fn author_word(author: Author) -> &'static str {
    match author {
        Author::Human => "human",
        Author::Machine => "machine",
    }
}

pub fn rel_word(rel: DepKind) -> &'static str {
    match rel {
        DepKind::DependsOn => "depends_on",
        DepKind::DerivedFrom => "derived_from",
    }
}

pub fn integration_word(integration: IntegrationStatus) -> &'static str {
    match integration {
        IntegrationStatus::NotRequired => "not-required",
        IntegrationStatus::Pending => "pending",
        IntegrationStatus::Accepted => "accepted",
        IntegrationStatus::Published => "published",
        IntegrationStatus::Rejected => "rejected",
    }
}

pub fn recorded_outcome_word(outcome: RecordedOutcome) -> &'static str {
    match outcome {
        RecordedOutcome::Open => "open",
        RecordedOutcome::Succeeded => "succeeded",
        RecordedOutcome::Failed => "failed",
        RecordedOutcome::Accepted => "accepted",
        RecordedOutcome::Rejected => "rejected",
        RecordedOutcome::Abandoned => "abandoned",
    }
}

fn blocker_reason_word(reason: BlockerReason) -> &'static str {
    match reason {
        BlockerReason::Missing => "missing",
        BlockerReason::Open => "open",
        BlockerReason::Failed => "failed",
        BlockerReason::Rejected => "rejected",
        BlockerReason::Abandoned => "abandoned",
        BlockerReason::Stale => "stale",
        BlockerReason::AwaitingIntegration => "awaiting integration",
        BlockerReason::Error => "error",
    }
}

/// One short phrase for a node's state: what it is, and why.
pub fn state_summary(state: &NodeState) -> String {
    let (outcome, currency, integration) = match state {
        NodeState::Error { message } => return format!("error ({message})"),
        NodeState::Known {
            outcome,
            currency,
            integration,
            ..
        } => (*outcome, *currency, *integration),
    };
    match state.workability() {
        Workability::Error => unreachable!("an error state has no dimensions"),
        Workability::Complete => match outcome {
            RecordedOutcome::Accepted => "review accepted".into(),
            RecordedOutcome::Rejected => "review rejected".into(),
            RecordedOutcome::Abandoned => "review abandoned".into(),
            _ => "complete".into(),
        },
        Workability::AwaitingIntegration => match integration {
            IntegrationStatus::Pending => "awaiting a candidate decision".into(),
            _ => "accepted; awaiting publication".into(),
        },
        Workability::Ready => {
            if currency == Currency::Stale {
                let reason = state
                    .staleness()
                    .first()
                    .map(format_staleness)
                    .unwrap_or_else(|| "recorded evidence changed".into());
                format!("ready (previous result stale: {reason})")
            } else if outcome == RecordedOutcome::Failed {
                "ready (previous attempt failed)".into()
            } else if integration == IntegrationStatus::Rejected {
                "ready (candidate rejected)".into()
            } else {
                "ready".into()
            }
        }
        Workability::Blocked => match state.blockers().first() {
            Some(blocker) => format!("blocked by {}", format_blocker(blocker)),
            None => "blocked".into(),
        },
    }
}

/// A blocker in a listing, where there is room to say why it blocks rather
/// than just name the reason.
pub fn format_blocker(blocker: &Blocker) -> String {
    let reason = match blocker.reason {
        BlockerReason::Missing => "missing",
        BlockerReason::Open => "not complete (open)",
        BlockerReason::Failed => "not complete (failed)",
        BlockerReason::Rejected => "review rejected",
        BlockerReason::Abandoned => "review abandoned",
        BlockerReason::Stale => "not complete (stale)",
        BlockerReason::AwaitingIntegration => "awaiting candidate integration",
        BlockerReason::Error => "unreadable records",
    };
    format!("{}: {reason}", blocker.id)
}

pub fn format_staleness(reason: &StalenessReason) -> String {
    match reason {
        StalenessReason::DefinitionChanged {
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
            format!("definition changed since the work ({})", files.join(", "))
        }
        StalenessReason::ConsumedDefinitionChanged { id } => {
            format!("dependency {id}: definition moved")
        }
        StalenessReason::ConsumedNodeMissing { id } => format!("dependency {id}: missing"),
        StalenessReason::ConsumedResultChanged { id } => {
            format!("dependency {id}: result changed since it was consumed")
        }
        StalenessReason::ConsumedOutputChanged { id } => format!("dependency {id}: output changed"),
        StalenessReason::ContextChanged { path } => format!("context {path}: content changed"),
        StalenessReason::ContextMissing { path } => format!("context {path}: missing"),
        StalenessReason::OutputDrifted { artifact, detail } => format!(
            "output changed since {artifact}:\n      {}",
            detail.replace('\n', "\n      ")
        ),
    }
}

/// One reason a branch of work is not settled: which node, and what about it.
pub fn format_unsettled(unsettled: &Unsettled) -> String {
    let id = &unsettled.id;
    match &unsettled.reason {
        UnsettledReason::Error { message } => format!("{id}: error ({message})"),
        UnsettledReason::AwaitingIntegration => format!("{id}: awaiting candidate integration"),
        UnsettledReason::Blocked { blockers } => format!(
            "{id}: blocked by {}",
            blockers
                .iter()
                .map(|blocker| format!(
                    "{} ({})",
                    blocker.id,
                    blocker_reason_word(blocker.reason)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        UnsettledReason::Open { outcome, stale } => {
            let outcome = outcome.map_or("open", recorded_outcome_word);
            if *stale {
                format!("{id}: {outcome} but stale")
            } else {
                format!("{id}: not done ({outcome})")
            }
        }
    }
}

/// One listing line: id, state, and the node's title.
pub fn node_line(store: &Store, graph: &Graph, id: &NodeId) -> String {
    let title = match store.read_node(id) {
        Ok((_, description)) => title_of(&description).to_string(),
        Err(_) => "(unreadable definition)".into(),
    };
    format!(
        "{:<32} {:<8} {title}",
        id.as_str(),
        state_summary(graph.state(id))
    )
}

/// One line describing a candidate in a listing.
pub fn candidate_line(graph: &Graph, candidate: &Candidate) -> String {
    format!(
        "{}  node {}  {}  {} -> {}",
        candidate.id,
        candidate.node,
        decision_word(graph, candidate),
        candidate.branch,
        candidate.target
    )
}

/// What the current verifications say about a candidate.
pub fn decision_word(graph: &Graph, candidate: &Candidate) -> String {
    match graph.decision(&candidate.id) {
        Ok(CandidateDecision::Pending) => "pending".into(),
        Ok(CandidateDecision::Accepted) => "accepted".into(),
        Ok(CandidateDecision::Rejected) => "rejected".into(),
        Err(problem) => format!("contested ({problem})"),
    }
}

/// The `candidate` view: the record's facts, then what was derived from them.
pub fn candidate(graph: &Graph, candidate: &Candidate) -> Result<String> {
    let mut out = String::new();
    writeln!(out, "candidate {}", candidate.id)?;
    writeln!(out, "node      {}", candidate.node)?;
    writeln!(out, "decision  {}", decision_word(graph, candidate))?;
    writeln!(out, "branch    {}", candidate.branch)?;
    writeln!(out, "target    {}", candidate.target)?;
    writeln!(out, "artifact  {}", candidate.artifact.id)?;
    writeln!(out, "result    {}", short_result(&candidate.result))?;
    if let Some(external) = &candidate.external {
        writeln!(out, "external  {}/{}", external.namespace, external.id)?;
    }
    if let Some(integration) = graph.state(&candidate.node).integration() {
        writeln!(out, "source    {}", integration_word(integration))?;
    }
    for verification in graph.verifications_of(&candidate.id) {
        writeln!(
            out,
            "review    {verification}  {}",
            state_summary(graph.state(verification))
        )?;
    }
    Ok(out)
}

/// The `show` view: definition, derived state, result, and staleness.
pub fn show(store: &Store, graph: &Graph, id: &NodeId) -> Result<String> {
    let (meta, description) = store.read_node(id)?;
    let state = graph.state(id);
    let mut out = String::new();

    writeln!(out, "id:      {id}")?;
    writeln!(out, "status:  {}", state_summary(state))?;
    writeln!(out, "author:  {}", author_word(meta.author))?;
    if let Some(assignee) = meta.assignee {
        writeln!(out, "assignee: {}", author_word(assignee))?;
    }
    writeln!(
        out,
        "version: {}",
        short_definition(&store.node_version(id)?)
    )?;
    for dependency in &meta.depends_on {
        writeln!(out, "depends_on:   {dependency}")?;
    }
    for source in &meta.derived_from {
        writeln!(out, "derived_from: {source}")?;
    }
    if let Some(candidate) = &meta.verifies {
        writeln!(out, "verifies:     {candidate}")?;
    }
    for candidate in graph.candidates_of(id) {
        writeln!(
            out,
            "candidate: {} ({}, {} -> {})",
            candidate.id,
            decision_word(graph, candidate),
            candidate.branch,
            candidate.target
        )?;
    }

    if let Some((result, notes)) = store.read_result(id)? {
        writeln!(out, "result:")?;
        writeln!(out, "  outcome: {}", result.outcome.as_str())?;
        writeln!(out, "  author:  {}", author_word(result.author))?;
        if let Some(producer) = &result.producer {
            writeln!(out, "  producer: {} {}", producer.namespace, producer.data)?;
        }
        if let Some(output) = &result.output {
            writeln!(out, "  output:  commit {}", short(&output.id))?;
        }
        for consumed in &result.consumed {
            let result_pin = consumed
                .result
                .as_ref()
                .map_or_else(|| "none".into(), short_result);
            match &consumed.output {
                Some(output) => writeln!(
                    out,
                    "  built against {} @ {} (result {result_pin}, output {})",
                    consumed.id,
                    short_definition(&consumed.definition),
                    short(&output.id)
                )?,
                None => writeln!(
                    out,
                    "  built against {} @ {} (result {result_pin})",
                    consumed.id,
                    short_definition(&consumed.definition)
                )?,
            }
        }
        let current = store.current_result_version(id)?;
        let observed = store
            .read_observed_context(id)?
            .filter(|observed| Some(&observed.result) == current.as_ref())
            .map(|observed| observed.pins)
            .unwrap_or_default();
        for pin in result.context.iter().chain(&observed) {
            let tag = if pin.observed { " (observed)" } else { "" };
            writeln!(
                out,
                "  context {} @ {}{tag}",
                pin.path,
                short(&pin.identity)
            )?;
        }
        let notes = notes.trim_end();
        if !notes.is_empty() {
            writeln!(out, "  notes:")?;
            for line in notes.lines() {
                writeln!(out, "    {line}")?;
            }
        }
    }

    if !state.staleness().is_empty() {
        writeln!(out, "stale:")?;
        for reason in state.staleness() {
            writeln!(out, "  {}", format_staleness(reason))?;
        }
    }
    let description = description.trim_end();
    if !description.is_empty() {
        writeln!(out, "\n{description}")?;
    }
    Ok(out)
}

/// One line describing a pairing: the checked root, then its informational
/// fields.
pub fn pairing_line(pairing: &Pairing) -> String {
    let mut line = format!("paired to project root {}", short(&pairing.root_commit));
    if let Some(name) = &pairing.name {
        line.push_str(&format!(" ({name})"));
    }
    if let Some(remote) = &pairing.remote {
        line.push_str(&format!(", remote {remote}"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use linka::model::Blocker;

    fn known(
        outcome: RecordedOutcome,
        currency: Currency,
        integration: IntegrationStatus,
    ) -> NodeState {
        NodeState::Known {
            outcome,
            currency,
            integration,
            staleness: match currency {
                Currency::Stale => vec![StalenessReason::ContextMissing {
                    path: "input".parse().unwrap(),
                }],
                Currency::Current => Vec::new(),
            },
            blockers: Vec::new(),
        }
    }

    #[test]
    fn the_summary_says_what_to_do_about_the_node() {
        assert_eq!(
            state_summary(&known(
                RecordedOutcome::Succeeded,
                Currency::Current,
                IntegrationStatus::NotRequired
            )),
            "complete"
        );
        assert!(state_summary(&known(
            RecordedOutcome::Succeeded,
            Currency::Stale,
            IntegrationStatus::NotRequired
        ))
        .starts_with("ready (previous result stale:"));
        assert_eq!(
            state_summary(&known(
                RecordedOutcome::Failed,
                Currency::Current,
                IntegrationStatus::NotRequired
            )),
            "ready (previous attempt failed)"
        );
        assert_eq!(
            state_summary(&known(
                RecordedOutcome::Succeeded,
                Currency::Current,
                IntegrationStatus::Accepted
            )),
            "accepted; awaiting publication"
        );
        assert_eq!(
            state_summary(&NodeState::Error {
                message: "unparseable node.toml".into()
            }),
            "error (unparseable node.toml)"
        );

        let blocked = NodeState::Known {
            outcome: RecordedOutcome::Open,
            currency: Currency::Current,
            integration: IntegrationStatus::NotRequired,
            staleness: Vec::new(),
            blockers: vec![Blocker {
                id: "node-dependency".parse().unwrap(),
                reason: BlockerReason::Stale,
            }],
        };
        assert_eq!(
            state_summary(&blocked),
            "blocked by node-dependency: not complete (stale)"
        );
    }

    #[test]
    fn shortening_a_hand_edited_hash_cuts_on_a_character_boundary() {
        assert_eq!(short("6c3fd7d79e588fbcc0ad3f2ddd959f76"), "6c3fd7d79e58");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
        // Not a hash at all, because a hand edit put something else there.
        assert_eq!(short("𝄞𝄞𝄞"), "𝄞𝄞𝄞");
        assert_eq!(short("𝄞").len(), "𝄞".len());
    }

    #[test]
    fn every_unsettled_reason_says_which_node_and_why() {
        let id: NodeId = "node-1".parse().unwrap();
        let unsettled = |reason| format_unsettled(&Unsettled { id: id.clone(), reason });
        assert_eq!(
            unsettled(UnsettledReason::Error {
                message: "unparseable node.toml".into()
            }),
            "node-1: error (unparseable node.toml)"
        );
        assert_eq!(
            unsettled(UnsettledReason::AwaitingIntegration),
            "node-1: awaiting candidate integration"
        );
        assert_eq!(
            unsettled(UnsettledReason::Blocked {
                blockers: vec![
                    Blocker {
                        id: "node-2".parse().unwrap(),
                        reason: BlockerReason::Open,
                    },
                    Blocker {
                        id: "node-3".parse().unwrap(),
                        reason: BlockerReason::AwaitingIntegration,
                    },
                ],
            }),
            "node-1: blocked by node-2 (open), node-3 (awaiting integration)"
        );
        assert_eq!(
            unsettled(UnsettledReason::Open {
                outcome: Some(RecordedOutcome::Failed),
                stale: false,
            }),
            "node-1: not done (failed)"
        );
        assert_eq!(
            unsettled(UnsettledReason::Open {
                outcome: Some(RecordedOutcome::Succeeded),
                stale: true,
            }),
            "node-1: succeeded but stale"
        );
        assert_eq!(
            unsettled(UnsettledReason::Open {
                outcome: None,
                stale: false,
            }),
            "node-1: not done (open)"
        );
    }
}
