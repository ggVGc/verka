//! `llaundry-viz` — a small web server that visualises the node graph.
//!
//! It serves a single self-contained page (no external assets) showing every
//! node and its `depends_on` / `derived_from` connections, laid out as a
//! left-to-right dependency graph, plus a JSON endpoint the page polls so the
//! view tracks the store live. Read-only: it never mutates the store or the
//! repository.
//!
//! The server is a deliberately tiny `std::net` loop — the two routes (`/` and
//! `/api/graph`) don't justify an HTTP framework dependency.

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use llaundry::{ops, GitVcs, Store, Vcs};

const PAGE: &str = include_str!("viz.html");

#[derive(Parser)]
#[command(name = "llaundry-viz", version, about = "Serve an interactive view of the node graph")]
struct Cli {
    /// Path to the store directory.
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry")]
    store: std::path::PathBuf,
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7710")]
    addr: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store::open(cli.store)?;
    let vcs = GitVcs::new(store.project_root());
    let shared = Arc::new((store, vcs));

    let listener = TcpListener::bind(&cli.addr)
        .with_context(|| format!("cannot listen on {}", cli.addr))?;
    println!("llaundry-viz: serving http://{}", listener.local_addr()?);

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let (store, vcs) = &*shared;
            if let Err(e) = handle(stream, store, vcs) {
                eprintln!("llaundry-viz: request failed: {e:#}");
            }
        });
    }
    Ok(())
}

/// Serve one request: parse just the request line, route, respond, close.
fn handle(mut stream: TcpStream, store: &Store, vcs: &dyn Vcs) -> Result<()> {
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or(path);

    let (status, content_type, body) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", PAGE.to_string()),
        "/api/graph" => match graph_json(store, vcs) {
            Ok(v) => ("200 OK", "application/json", v.to_string()),
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                json!({ "error": format!("{e:#}") }).to_string(),
            ),
        },
        _ if path.starts_with("/api/log/") => {
            let id = &path["/api/log/".len()..];
            match store.read_work_log(id) {
                Ok(Some(log)) => ("200 OK", "application/json", json!({ "id": id, "log": log }).to_string()),
                Ok(None) => (
                    "404 Not Found",
                    "application/json",
                    json!({ "error": format!("no work log recorded for `{id}`") }).to_string(),
                ),
                Err(e) => (
                    "500 Internal Server Error",
                    "application/json",
                    json!({ "error": format!("{e:#}") }).to_string(),
                ),
            }
        }
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found".into()),
    };

    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

/// The whole graph with everything derived, in one payload: per node its
/// definition, derived status/readiness, blockers, staleness reasons, and the
/// completion record if any — so the page needs no other endpoint.
fn graph_json(store: &Store, vcs: &dyn Vcs) -> Result<Value> {
    let mut nodes = Vec::new();
    for id in store.list_ids()? {
        let (meta, body) = match store.read_node(&id) {
            Ok(x) => x,
            Err(e) => {
                // Surface a broken node instead of hiding the whole graph.
                nodes.push(json!({ "id": id, "title": "(unreadable node)", "error": format!("{e:#}") }));
                continue;
            }
        };
        let status = ops::current_status(store, &id);
        let stale = ops::staleness(store, vcs, &id);
        let blockers = ops::blockers(store, vcs, &id);
        let result = store.read_result(&id).ok().flatten().map(|(r, notes)| {
            json!({
                "at": r.at,
                "author": r.author.as_str(),
                "outcome": r.outcome.as_str(),
                "output_commit": r.output_commit,
                "built_against": r.built_against.iter().map(|ba| json!({
                    "id": ba.id, "pin": ops::short(&ba.pin), "output": ba.output.as_deref().map(ops::short),
                })).collect::<Vec<_>>(),
                "context": r.context.iter().map(|c| json!({
                    "path": c.path, "blob": ops::short(&c.blob),
                })).collect::<Vec<_>>(),
                "notes": notes,
            })
        });
        nodes.push(json!({
            "id": id,
            "title": meta.title,
            "body": body,
            "author": meta.author.as_str(),
            "assignee": meta.assignee.map(|a| a.as_str()),
            "depends_on": meta.depends_on,
            "derived_from": meta.derived_from,
            "status": status.as_str(),
            "ready": ops::is_ready(store, vcs, &id),
            "has_log": matches!(store.read_work_log(&id), Ok(Some(_))),
            "stale": stale,
            "blockers": blockers,
            "result": result,
        }));
    }
    Ok(json!({ "nodes": nodes, "problems": ops::check(store)? }))
}
