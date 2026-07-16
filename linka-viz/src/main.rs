//! `linka-viz` — a small web server that visualises the node graph.
//!
//! It serves a single self-contained page (no external assets) showing every
//! node and its `depends_on` / `derived_from` connections, laid out as a
//! left-to-right dependency graph, plus a JSON endpoint the page polls so the
//! view tracks the store live. Read-only, with one exception: a human can
//! answer a node assigned to them (`POST /api/respond/<id>`), which completes
//! it with their response as the result notes.
//!
//! The server is a deliberately tiny `std::net` loop — a handful of routes
//! doesn't justify an HTTP framework dependency.

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use linka::{ops, title_of, Author, Blocker, BlockerReason, GitVcs, StalenessReason, Store, Vcs};

const PAGE: &str = include_str!("viz.html");

#[derive(Parser)]
#[command(
    name = "linka-viz",
    version,
    about = "Serve an interactive view of the node graph"
)]
struct Cli {
    /// Path to the store directory.
    #[arg(long, env = "LINKA_DIR", default_value = ".linka")]
    store: std::path::PathBuf,
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7710")]
    addr: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::open(cli.store)?;
    let vcs = GitVcs::for_store(&store);
    let shared = Arc::new((store, vcs));

    let listener =
        TcpListener::bind(&cli.addr).with_context(|| format!("cannot listen on {}", cli.addr))?;
    println!("linka-viz: serving http://{}", listener.local_addr()?);

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let (store, vcs) = &*shared;
            if let Err(e) = handle(stream, store, vcs) {
                eprintln!("linka-viz: request failed: {e:#}");
            }
        });
    }
    Ok(())
}

/// Serve one request: parse the request line and headers, read the body if
/// one is declared, route, respond, close.
fn handle(mut stream: TcpStream, store: &Store, vcs: &dyn Vcs) -> Result<()> {
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
        ("GET", "/api/graph") => match graph_json(store, vcs) {
            Ok(v) => ("200 OK", "application/json", v.to_string()),
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                json!({ "error": format!("{e:#}") }).to_string(),
            ),
        },
        ("POST", p) if p.starts_with("/api/respond/") => {
            let id = &p["/api/respond/".len()..];
            match respond(store, vcs, id, &request_body) {
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

/// The whole graph with everything derived, in one payload: per node its
/// definition, derived status/readiness, blockers, staleness reasons, and the
/// completion record if any — so the page needs no other endpoint.
fn graph_json(store: &Store, vcs: &dyn Vcs) -> Result<Value> {
    let mut nodes = Vec::new();
    for id in store.list_ids()? {
        let (meta, description) = match store.read_node(&id) {
            Ok(x) => x,
            Err(e) => {
                // Surface a broken node instead of hiding the whole graph.
                nodes.push(
                    json!({ "id": id, "title": "(unreadable node)", "error": format!("{e:#}") }),
                );
                continue;
            }
        };
        let state = match ops::node_state(store, vcs, &id) {
            Ok(state) => state,
            Err(error) => {
                nodes.push(json!({
                    "id": id,
                    "title": title_of(&description),
                    "error": format!("{error:#}"),
                }));
                continue;
            }
        };
        let status = if state.is_complete() {
            "complete"
        } else if state.is_ready() {
            "ready"
        } else {
            "blocked"
        };
        let result = store.read_result(&id).ok().flatten().map(|(r, notes)| {
            json!({
                "at": r.at,
                "author": r.author.as_str(),
                "outcome": r.outcome.as_str(),
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
            "outcome": state.outcome,
            "currency": state.currency,
            "stale": state.staleness.iter().map(format_staleness).collect::<Vec<_>>(),
            "blockers": state.blockers.iter().map(format_blocker).collect::<Vec<_>>(),
            "result": result,
        }));
    }
    Ok(json!({ "nodes": nodes, "problems": ops::check(store)? }))
}

fn format_blocker(blocker: &Blocker) -> String {
    let reason = match blocker.reason {
        BlockerReason::Missing => "missing",
        BlockerReason::Open => "not complete (open)",
        BlockerReason::Failed => "not complete (failed)",
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
    }
}
