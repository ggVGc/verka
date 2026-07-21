//! `orka-web` — a local web interface for an Orka workbench.
//!
//! It serves a single self-contained page (no external assets) combining the
//! Linka graph Orka orchestrates with Orka's ready queue, durable attempts,
//! transcripts, candidates, and active reviews. The page polls one JSON
//! endpoint so it follows the workbench live. A human can also answer ready
//! human work through Linka's public operation.
//!
//! The server is a deliberately tiny `std::net` loop — a handful of routes
//! doesn't justify an HTTP framework dependency.

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use linka::{
    ops, title_of, Author, Blocker, BlockerReason, CandidateStore, GitVcs, NodeId, ResultMeta,
    StalenessReason, Store, Vcs,
};
use orka::attempt::{AttemptId, AttemptPhase, FsAttemptStore, SealedState};
use orka::candidate::Candidates;
use orka::events::{read_work_log, transcript_blocks};
use orka::linka_work::LinkaWork;
use orka::review::Reviews;

const PAGE: &str = include_str!("index.html");

#[derive(Parser)]
#[command(
    name = "orka-web",
    version,
    about = "Serve a local web interface for an Orka workbench"
)]
struct Cli {
    /// Workbench root (holds .linka/, .orka/, project/, and orka.toml).
    /// Defaults to the nearest ancestor containing .linka/.
    #[arg(long, env = "ORKA_WORKBENCH")]
    workbench: Option<PathBuf>,
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7710")]
    addr: String,
}

struct App {
    root: PathBuf,
    store: Store,
    vcs: GitVcs,
    attempts: FsAttemptStore,
}

impl App {
    fn open(given: Option<PathBuf>) -> Result<Self> {
        let root = locate_workbench(given)?;
        let store = Store::open(root.join(".linka"))?;
        let vcs = GitVcs::for_store(&store);
        let attempts = FsAttemptStore::new(root.join(".orka"));
        Ok(Self {
            root,
            store,
            vcs,
            attempts,
        })
    }
}

fn locate_workbench(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = given {
        if !root.join(".linka").is_dir() {
            bail!("no .linka store under {}", root.display());
        }
        return Ok(root);
    }
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join(".linka").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("no Orka workbench found: no ancestor contains .linka/");
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let app = Arc::new(App::open(cli.workbench)?);

    let listener =
        TcpListener::bind(&cli.addr).with_context(|| format!("cannot listen on {}", cli.addr))?;
    println!(
        "orka-web: serving {} at http://{}",
        app.root.display(),
        listener.local_addr()?
    );

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            if let Err(e) = handle(stream, &app) {
                eprintln!("orka-web: request failed: {e:#}");
            }
        });
    }
    Ok(())
}

/// Serve one request: parse the request line and headers, read the body if
/// one is declared, route, respond, close.
fn handle(mut stream: TcpStream, app: &App) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/");
    let path = path.split('?').next().unwrap_or(path).to_string();

    // Drain the headers, keeping only Content-Length (capped: the only
    // expected body is a short JSON response payload).
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length > 1 << 20 {
        bail!("request body too large ({content_length} bytes)");
    }
    let mut request_body = vec![0u8; content_length];
    reader.read_exact(&mut request_body)?;

    let (status, content_type, body) = match (method.as_str(), path.as_str()) {
        ("GET", "/" | "/index.html") => ("200 OK", "text/html; charset=utf-8", PAGE.to_string()),
        ("GET", "/api/state") => match state_json(app) {
            Ok(v) => ("200 OK", "application/json", v.to_string()),
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                json!({ "error": format!("{e:#}") }).to_string(),
            ),
        },
        ("POST", p) if p.starts_with("/api/respond/") => {
            let id = &p["/api/respond/".len()..];
            match respond(&app.store, &app.vcs, id, &request_body) {
                Ok(()) => (
                    "200 OK",
                    "application/json",
                    json!({ "id": id }).to_string(),
                ),
                Err(e) => (
                    "400 Bad Request",
                    "application/json",
                    json!({ "error": format!("{e:#}") }).to_string(),
                ),
            }
        }
        ("GET", p) if p.starts_with("/api/transcript/") => {
            let id = &p["/api/transcript/".len()..];
            match transcript_json(&app.attempts, id) {
                Ok(v) => ("200 OK", "application/json", v.to_string()),
                Err(e) => (
                    "404 Not Found",
                    "application/json",
                    json!({ "error": format!("{e:#}") }).to_string(),
                ),
            }
        }
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found".into(),
        ),
    };

    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

/// Complete a human-assigned node with the human's typed response as its
/// result notes — the one write the page offers. Refuses nodes not assigned
/// to a human, so the browser cannot close machine work. Goes through
/// [`ops::respond`], which does not gate on project-tree cleanliness.
fn respond(store: &Store, vcs: &dyn Vcs, id: &str, body: &[u8]) -> Result<()> {
    let payload: Value = serde_json::from_slice(body).context("request body is not JSON")?;
    let notes = payload["notes"].as_str().unwrap_or_default();
    let (meta, _) = store.read_node(id)?;
    if meta.assignee != Some(Author::Human) {
        bail!("`{id}` is not assigned to a human");
    }
    ops::respond(store, vcs, id, notes, Author::Human)?;
    Ok(())
}

/// The orchestration state in one payload: Orka's dispatchable work, durable
/// attempts, candidates and reviews, plus the derived Linka graph those Orka
/// records refer to.
fn state_json(app: &App) -> Result<Value> {
    let store = &app.store;
    let vcs = &app.vcs;
    let attempts = attempts_json(&app.attempts)?;
    let candidates = candidates_json(store, &app.attempts)?;
    let reviews = reviews_json(store, app.attempts.root())?;
    let ready = LinkaWork::new(store).ready_for_machine()?;
    let ready_ids = ready
        .iter()
        .map(|item| item.node.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut nodes = Vec::new();
    for id in store.list_ids()? {
        let (meta, description) = match store.read_node(&id) {
            Ok(x) => x,
            Err(e) => {
                // Surface a broken node instead of hiding the whole graph.
                nodes.push(json!({
                    "id": id,
                    "title": "(unreadable node)",
                    "status": "error",
                    "error": format!("{e:#}"),
                }));
                continue;
            }
        };
        let state = match ops::node_state(store, vcs, &id) {
            Ok(state) => state,
            Err(error) => {
                nodes.push(json!({
                    "id": id,
                    "title": title_of(&description),
                    "status": "error",
                    "error": format!("{error:#}"),
                }));
                continue;
            }
        };
        let status = workability(&state);
        let stored_result = store.read_result(&id)?;
        let candidate = stored_result
            .as_ref()
            .map(|(result, _)| current_candidate_json(store, vcs, &id, result))
            .transpose()?
            .flatten();
        let result = stored_result.map(|(r, notes)| {
            json!({
                "at": r.at,
                "author": r.author.as_str(),
                // A successful attempt is evidence, not necessarily a complete
                // node: candidate output still has to be integrated.
                "outcome": match r.outcome {
                    linka::Outcome::Done => "succeeded",
                    linka::Outcome::Failed => "failed",
                },
                "output_commit": ops::output_commit(&r),
                // Producer evidence is namespaced application data (e.g. an
                // execution harness records backend/model); pass it through.
                "worked_by": r.producer.as_ref().and_then(|p| {
                    p.data.get("backend").map(|backend| json!({
                        "backend": backend, "model": p.data.get("model"),
                    }))
                }),
                "built_against": r.consumed.iter().map(|ba| json!({
                    "id": ba.id,
                    "pin": ops::short_definition(&ba.definition),
                    "result": ba.result.as_ref().map(ops::short_result),
                    "output": ba.output.as_ref().map(|o| ops::short(&o.id)),
                })).collect::<Vec<_>>(),
                "context": r.context.iter().map(|c| json!({
                    "path": c.path, "blob": ops::short(&c.identity),
                })).collect::<Vec<_>>(),
                "notes": notes,
            })
        });
        nodes.push(json!({
            "id": id,
            "title": title_of(&description),
            "description": description,
            "author": meta.author.as_str(),
            "assignee": meta.assignee.map(|a| a.as_str()),
            "depends_on": meta.depends_on,
            "derived_from": meta.derived_from,
            "status": status,
            "ready": state.is_ready(),
            "orka_ready": ready_ids.contains(id.as_str()),
            "outcome": state.outcome,
            "currency": state.currency,
            "integration": state.integration,
            "candidate": candidate,
            "stale": state.staleness.iter().map(format_staleness).collect::<Vec<_>>(),
            "blockers": state.blockers.iter().map(format_blocker).collect::<Vec<_>>(),
            "result": result,
            "attempts": attempts.iter()
                .filter(|attempt| attempt["node"].as_str() == Some(id.as_str()))
                .cloned()
                .collect::<Vec<_>>(),
        }));
    }
    Ok(json!({
        "workbench": app.root,
        "nodes": nodes,
        "ready": ready.iter().map(|item| json!({
            "node": item.node,
            "title": item.title,
        })).collect::<Vec<_>>(),
        "attempts": attempts,
        "candidates": candidates,
        "reviews": reviews,
        "problems": ops::check(store)?,
    }))
}

fn attempts_json(store: &FsAttemptStore) -> Result<Vec<Value>> {
    store
        .list()?
        .into_iter()
        .map(|id| {
            let snapshot = store.load(&id)?;
            let transcript = store.transcript_path(&id).is_file();
            let evidence = snapshot.evidence.as_ref().map(|e| {
                json!({
                    "backend": e.backend,
                    "exit_code": e.exit_code,
                    "started_at_ms": e.started_at_ms,
                    "finished_at_ms": e.finished_at_ms,
                })
            });
            let seal = snapshot.seal.as_ref().map(|seal| {
                json!({
                    "state": sealed_state_label(&seal.state),
                    "detail": sealed_state_detail(&seal.state),
                    "sealed_at_ms": seal.sealed_at_ms,
                })
            });
            Ok(json!({
                "id": id.0,
                "node": snapshot.record.input.node(),
                "title": title_of(&snapshot.record.input.description),
                "created_at_ms": snapshot.record.created_at_ms,
                "phase": attempt_phase_label(snapshot.phase()),
                "input_commit": snapshot.record.input.input_commit(),
                "target": snapshot.record.input.target_branch,
                "workspace": snapshot.workspace.as_ref().map(|workspace| json!({
                    "path": workspace.path,
                    "branch": workspace.branch,
                })),
                "evidence": evidence,
                "seal": seal,
                "has_transcript": transcript,
            }))
        })
        .collect()
}

fn candidates_json(store: &Store, attempts: &FsAttemptStore) -> Result<Vec<Value>> {
    Candidates::new(store, attempts)
        .list()?
        .into_iter()
        .map(|candidate| {
            let status = candidate.status();
            Ok(json!({
                "id": candidate.id,
                "attempt": candidate.attempt.as_ref().map(|attempt| &attempt.0),
                "node": candidate.node,
                "branch": candidate.branch,
                "target": candidate.target,
                "input_commit": candidate.input_commit,
                "head_commit": candidate.head_commit,
                "status": status,
            }))
        })
        .collect()
}

fn reviews_json(store: &Store, orka_root: &Path) -> Result<Vec<Value>> {
    Reviews::new(store, orka_root)
        .list()?
        .into_iter()
        .map(|review| {
            Ok(json!({
                "candidate": review.candidate,
                "verification": review.verification,
                "branch": review.branch,
                "subject": review.subject,
            }))
        })
        .collect()
}

fn transcript_json(store: &FsAttemptStore, raw_id: &str) -> Result<Value> {
    let id = AttemptId(raw_id.to_string());
    // Loading first both validates the id against durable Orka state and keeps
    // arbitrary paths out of this read-only endpoint.
    let snapshot = store.load(&id)?;
    let path = store.transcript_path(&id);
    let transcript = std::fs::read_to_string(&path)
        .with_context(|| format!("attempt `{id}` has no readable transcript"))?;
    let events = store.events_path(&id);
    let blocks = if events.is_file() {
        read_work_log(&events)?
    } else {
        transcript_blocks(&transcript)
    };
    Ok(json!({
        "attempt": id.0,
        "node": snapshot.record.input.node(),
        "blocks": blocks,
        // Retained for read-only API clients written before structured work
        // logs were introduced. The page consumes `blocks`.
        "transcript": transcript,
    }))
}

fn attempt_phase_label(phase: AttemptPhase) -> &'static str {
    match phase {
        AttemptPhase::Created => "created",
        AttemptPhase::WorkspacePlanned => "workspace_planned",
        AttemptPhase::Prepared => "prepared",
        AttemptPhase::Requested => "requested",
        AttemptPhase::Executed => "executed",
        AttemptPhase::Sealed => "sealed",
    }
}

fn sealed_state_label(state: &SealedState) -> &'static str {
    match state {
        SealedState::Submitted { .. } => "submitted",
        SealedState::StaleAtSubmit { .. } => "stale_at_submit",
        SealedState::FailureRecorded => "failure_recorded",
        SealedState::Interrupted { .. } => "interrupted",
        SealedState::ContractViolation { .. } => "contract_violation",
    }
}

fn sealed_state_detail(state: &SealedState) -> Option<Value> {
    match state {
        SealedState::Submitted { output_commit } => output_commit.clone().map(Value::String),
        SealedState::StaleAtSubmit { conflicts } => Some(json!(conflicts)),
        SealedState::Interrupted { reason } | SealedState::ContractViolation { reason } => {
            Some(Value::String(reason.clone()))
        }
        SealedState::FailureRecorded => None,
    }
}

/// Present Linka's mutually meaningful work states without collapsing
/// integration waits into dependency blocking.
fn workability(state: &linka::NodeState) -> &'static str {
    if state.is_complete() {
        "complete"
    } else if state.is_blocked() {
        "blocked"
    } else if state.is_awaiting_integration() {
        "awaiting_integration"
    } else if state.is_ready() {
        "ready"
    } else {
        "error"
    }
}

fn current_candidate_json(
    store: &Store,
    vcs: &dyn Vcs,
    id: &str,
    result: &ResultMeta,
) -> Result<Option<Value>> {
    let Some(artifact) = &result.output else {
        return Ok(None);
    };
    let node: NodeId = id.parse().map_err(anyhow::Error::msg)?;
    let version = store.result_version(id)?;
    let Some(candidate) = CandidateStore::new(store).for_result(&node, &version, artifact)? else {
        return Ok(None);
    };
    let integration = candidate.integration(vcs)?;
    Ok(Some(json!({
        "id": candidate.id.to_string(),
        "branch": candidate.branch,
        "target": candidate.target,
        "integration": integration,
    })))
}

fn format_blocker(blocker: &Blocker) -> String {
    let reason = match blocker.reason {
        BlockerReason::Missing => "missing",
        BlockerReason::Open => "not complete (open)",
        BlockerReason::Failed => "not complete (failed)",
        BlockerReason::AwaitingIntegration => "awaiting candidate integration",
        BlockerReason::Stale => "not complete (stale)",
    };
    format!("{}: {reason}", blocker.id)
}

fn format_staleness(reason: &StalenessReason) -> String {
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
        StalenessReason::ConsumedOutputChanged { id } => {
            format!("dependency {id}: output changed")
        }
        StalenessReason::ContextChanged { path } => format!("context {path}: content changed"),
        StalenessReason::ContextMissing { path } => format!("context {path}: missing"),
        StalenessReason::OutputDrifted { artifact, detail } => {
            format!("output changed since {artifact}:\n{detail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linka::{Currency, IntegrationStatus, NodeState, RecordedOutcome};

    fn state(integration: IntegrationStatus, blockers: Vec<Blocker>) -> NodeState {
        NodeState {
            outcome: RecordedOutcome::Succeeded,
            currency: Currency::Current,
            integration,
            staleness: Vec::new(),
            blockers,
        }
    }

    #[test]
    fn integration_wait_is_not_presented_as_blocked() {
        let awaiting = state(IntegrationStatus::Pending, Vec::new());
        assert_eq!(workability(&awaiting), "awaiting_integration");

        let mut blocked = state(
            IntegrationStatus::NotRequired,
            vec![Blocker {
                id: "node-dependency".into(),
                reason: BlockerReason::Open,
            }],
        );
        blocked.outcome = RecordedOutcome::Open;
        assert_eq!(workability(&blocked), "blocked");

        let complete = state(IntegrationStatus::Published, Vec::new());
        assert_eq!(workability(&complete), "complete");
    }

    #[test]
    fn formats_structured_reasons_for_the_page() {
        assert_eq!(
            format_staleness(&StalenessReason::ContextChanged {
                path: "src/lib.rs".into(),
            }),
            "context src/lib.rs: content changed"
        );
        assert_eq!(
            format_blocker(&Blocker {
                id: "node-dependency".into(),
                reason: BlockerReason::Stale,
            }),
            "node-dependency: not complete (stale)"
        );
        assert_eq!(
            format_blocker(&Blocker {
                id: "node-candidate".into(),
                reason: BlockerReason::AwaitingIntegration,
            }),
            "node-candidate: awaiting candidate integration"
        );
    }
}
