//! `llaundry-mcp` — a Model Context Protocol server over the llaundry library.
//!
//! It speaks JSON-RPC 2.0 over stdio, the way MCP's stdio transport expects:
//! one JSON message per line, requests answered on stdout, everything else (logs)
//! on stderr. It is deliberately synchronous and dependency-light, matching the
//! rest of the project — the loop reads a line, dispatches, writes a line.
//!
//! The tools are a thin wrapper over `llaundry::ops`, so an agent gets the same
//! operations as the CLI: create/link/edit/complete nodes, move status, and the
//! derived read-only queries (show, list, stale, ready, blocked, origin).
//!
//! Each call opens the store fresh, so `initialize`/`tools/list` work even before a
//! store exists and an agent can create one with `init_store`.

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use llaundry::ops::{self, NewNode};
use llaundry::{Author, GitVcs, NodeType, Status, Store};

/// The MCP protocol revision we advertise (a client may negotiate its own; we echo
/// back whatever it asks for when present).
const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Parser)]
#[command(name = "llaundry-mcp", version, about = "MCP server for a llaundry store")]
struct Cli {
    /// Path to the store directory.
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry")]
    store: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    Server {
        store_path: cli.store,
    }
    .serve()
}

struct Server {
    store_path: PathBuf,
}

impl Server {
    /// Read requests line by line from stdin, answer each on stdout.
    fn serve(&self) -> Result<()> {
        eprintln!(
            "llaundry-mcp: serving on stdio (store {})",
            self.store_path.display()
        );
        let stdin = io::stdin();
        let mut out = io::stdout().lock();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let msg: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    write_msg(&mut out, &rpc_err(Value::Null, -32700, &format!("parse error: {e}")))?;
                    continue;
                }
            };
            // Notifications (no id) get no reply.
            if let Some(response) = self.handle(&msg) {
                write_msg(&mut out, &response)?;
            }
        }
        Ok(())
    }

    /// Turn one JSON-RPC message into an optional response (`None` for notifications).
    fn handle(&self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let is_request = matches!(&id, Some(v) if !v.is_null());
        if !is_request {
            return None;
        }
        let id = id.unwrap();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        Some(match method {
            "initialize" => rpc_ok(id, initialize_result(&params)),
            "ping" => rpc_ok(id, json!({})),
            "tools/list" => rpc_ok(id, json!({ "tools": tools() })),
            "tools/call" => rpc_ok(id, self.call_tool(&params)),
            other => rpc_err(id, -32601, &format!("method not found: {other}")),
        })
    }

    /// Run a `tools/call`. Tool *execution* failures are reported in-band as an MCP
    /// result with `isError: true` (per the spec), not as JSON-RPC protocol errors.
    fn call_tool(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        match self.dispatch(name, &args) {
            Ok(text) => json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            }),
            Err(e) => json!({
                "content": [{ "type": "text", "text": format!("error: {e:#}") }],
                "isError": true,
            }),
        }
    }

    /// Open the store and wire up the real git seam for one operation.
    fn open(&self) -> Result<(Store, GitVcs)> {
        let store = Store::open(self.store_path.clone())?;
        let vcs = GitVcs::new(store.project_root());
        Ok((store, vcs))
    }

    /// Map a tool name + JSON arguments to a library call, returning display text.
    fn dispatch(&self, name: &str, args: &Value) -> Result<String> {
        match name {
            "init_store" => {
                Store::init(self.store_path.clone())?;
                Ok(format!("initialised store at {}", self.store_path.display()))
            }

            "add_node" => {
                let (store, vcs) = self.open()?;
                let new = NewNode {
                    node_type: enum_or(args, "type", NodeType::Task)?,
                    title: req_str(args, "title")?,
                    body: opt_str(args, "body").unwrap_or_default(),
                    author: enum_or(args, "author", Author::Human)?,
                    depends_on: str_list(args, "depends_on"),
                    derived_from: str_list(args, "derived_from"),
                    inputs: str_list(args, "inputs"),
                };
                let (id, hash) = ops::add(&store, &vcs, new)?;
                Ok(format!("created {id}  ({})", ops::short(&hash)))
            }

            "link_nodes" => {
                let (store, vcs) = self.open()?;
                let from = req_str(args, "from")?;
                let to = req_str(args, "to")?;
                let rel = opt_str(args, "rel").unwrap_or_else(|| "depends_on".into());
                let author = enum_or(args, "author", Author::Human)?;
                let hash = ops::link(&store, &vcs, &from, &to, &rel, author)?;
                Ok(format!("{from} +{rel} -> {to}  (new version {})", ops::short(&hash)))
            }

            "edit_node" => {
                let (store, vcs) = self.open()?;
                let id = req_str(args, "id")?;
                let title = opt_str(args, "title");
                let body = opt_str(args, "body");
                let author = enum_or(args, "author", Author::Human)?;
                let hash = ops::edit(&store, &vcs, &id, title, body, author)?;
                Ok(format!("edited {id}  (new version {})", ops::short(&hash)))
            }

            "complete_node" => {
                let (store, vcs) = self.open()?;
                let id = req_str(args, "id")?;
                let outputs = str_list(args, "outputs");
                if outputs.is_empty() {
                    bail!("`outputs` must list at least one produced file");
                }
                let context = str_list(args, "context");
                let message = opt_str(args, "message");
                let author = enum_or(args, "author", Author::Machine)?;
                let (hash, commit) =
                    ops::complete(&store, &vcs, &id, &outputs, &context, message, author)?;
                Ok(format!(
                    "completed {id}  (version {}, output commit {})",
                    ops::short(&hash),
                    ops::short(&commit)
                ))
            }

            "set_status" => {
                let (store, vcs) = self.open()?;
                let id = req_str(args, "id")?;
                let status: Status = enum_req(args, "status")?;
                let author = enum_or(args, "author", Author::Human)?;
                ops::set_status(&store, &vcs, &id, status, author)?;
                Ok(format!("{id} -> {}", status.as_str()))
            }

            "show_node" => {
                let (store, vcs) = self.open()?;
                let id = req_str(args, "id")?;
                let hash = store.get_ref(&id)?;
                let (meta, body) = store.get_object(&hash)?;
                let log = store.status_log(&id)?;
                let status = log.events.last().map_or("(none)", |e| e.status.as_str());

                let mut lines = vec![
                    format!("id:      {}", meta.logical_id),
                    format!("type:    {}", meta.node_type.as_str()),
                    format!("title:   {}", meta.title),
                    format!("status:  {status}"),
                    format!("author:  {}", meta.author.as_str()),
                    format!("version: {hash}"),
                ];
                if let Some(parent) = &meta.parent {
                    lines.push(format!("parent:  {}", ops::short(parent)));
                }
                if !meta.edges.is_empty() {
                    lines.push("edges:".into());
                    for e in &meta.edges {
                        lines.push(format!("  {} -> {} @ {}", e.rel, e.to, ops::short(&e.pin)));
                    }
                }
                for (label, pins) in [("inputs", &meta.inputs), ("context", &meta.context)] {
                    if !pins.is_empty() {
                        lines.push(format!("{label}:"));
                        for p in pins {
                            lines.push(format!("  {} @ {}", p.path, ops::short(&p.content)));
                        }
                    }
                }
                if let Some(commit) = &meta.output_commit {
                    lines.push(format!("output:  commit {}", ops::short(commit)));
                }
                let reasons = ops::staleness(&store, &vcs, &meta);
                if !reasons.is_empty() {
                    lines.push("stale:".into());
                    lines.extend(reasons.iter().map(|r| format!("  {r}")));
                }
                let body = body.trim_end();
                if !body.is_empty() {
                    lines.push(String::new());
                    lines.push(body.to_string());
                }
                Ok(lines.join("\n"))
            }

            "list_nodes" => {
                let (store, _) = self.open()?;
                let mut lines = Vec::new();
                for id in store.list_refs()? {
                    let hash = store.get_ref(&id)?;
                    let (meta, _) = store.get_object(&hash)?;
                    let log = store.status_log(&id)?;
                    let status = log.events.last().map_or("-", |e| e.status.as_str());
                    lines.push(format!(
                        "{id}  [{status}]  {}  {}",
                        meta.node_type.as_str(),
                        meta.title
                    ));
                }
                Ok(joined(lines, "(no nodes)"))
            }

            "node_history" => {
                let (store, _) = self.open()?;
                let id = req_str(args, "id")?;
                let mut hash = store.get_ref(&id)?;
                let mut lines = Vec::new();
                loop {
                    let (meta, _) = store.get_object(&hash)?;
                    lines.push(format!("{}  {} {}", ops::short(&hash), meta.author.as_str(), meta.title));
                    match meta.parent {
                        Some(parent) => hash = parent,
                        None => break,
                    }
                }
                Ok(lines.join("\n"))
            }

            "stale_nodes" => {
                let (store, vcs) = self.open()?;
                let mut lines = Vec::new();
                for id in store.list_refs()? {
                    let hash = store.get_ref(&id)?;
                    let (meta, _) = store.get_object(&hash)?;
                    let reasons = ops::staleness(&store, &vcs, &meta);
                    if !reasons.is_empty() {
                        lines.push(format!("{id}:"));
                        lines.extend(reasons.iter().map(|r| format!("  {r}")));
                    }
                }
                Ok(joined(lines, "all nodes up to date"))
            }

            "ready_nodes" => {
                let (store, vcs) = self.open()?;
                let mut lines = Vec::new();
                for id in store.list_refs()? {
                    let hash = store.get_ref(&id)?;
                    let (meta, _) = store.get_object(&hash)?;
                    if ops::is_ready(&store, &vcs, &meta) {
                        lines.push(format!("{id}  {}", meta.title));
                    }
                }
                Ok(joined(lines, "(nothing ready)"))
            }

            "blocked_nodes" => {
                let (store, vcs) = self.open()?;
                let mut lines = Vec::new();
                for id in store.list_refs()? {
                    let hash = store.get_ref(&id)?;
                    let (meta, _) = store.get_object(&hash)?;
                    let blockers = ops::blockers(&store, &vcs, &meta);
                    if !blockers.is_empty() {
                        lines.push(format!("{id}:"));
                        lines.extend(blockers.iter().map(|b| format!("  blocked by {b}")));
                    }
                }
                Ok(joined(lines, "nothing blocked"))
            }

            "node_origin" => {
                let (store, _) = self.open()?;
                let commit = req_str(args, "commit")?;
                Ok(match ops::producer(&store, &commit)? {
                    Some((id, version)) => format!("{id}  {}", ops::short(&version)),
                    None => format!("no node produced {}", ops::short(&commit)),
                })
            }

            "node_outputs" => {
                let (store, _) = self.open()?;
                let id = req_str(args, "id")?;
                let hash = store.get_ref(&id)?;
                let (meta, _) = store.get_object(&hash)?;
                Ok(match meta.output_commit {
                    Some(commit) => commit,
                    None => format!("{id} has produced no output"),
                })
            }

            other => bail!("unknown tool `{other}`"),
        }
    }
}

/// The advertised tool catalogue, mirroring `llaundry::ops`.
fn tools() -> Value {
    let enum_prop = |values: &[&str], desc: &str| json!({"type": "string", "enum": values, "description": desc});
    let author = enum_prop(&["human", "machine"], "Author of the change.");
    let paths = |desc: &str| json!({"type": "array", "items": {"type": "string"}, "description": desc});

    json!([
        {
            "name": "init_store",
            "description": "Initialise a new llaundry store (creates the store directory skeleton).",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "add_node",
            "description": "Create a new node in the graph. Returns its logical id.",
            "inputSchema": {"type": "object", "properties": {
                "type": enum_prop(&["task", "implementation", "build", "verification", "info"], "Node type (default task)."),
                "title": {"type": "string", "description": "Short title."},
                "body": {"type": "string", "description": "Prose body (markdown)."},
                "author": author,
                "depends_on": paths("Logical ids this node depends on (pinned to their current versions)."),
                "derived_from": paths("Logical ids this node is derived from."),
                "inputs": paths("Declared input file paths, pinned by content; changing one invalidates the node.")
            }, "required": ["title"], "additionalProperties": false}
        },
        {
            "name": "link_nodes",
            "description": "Add a typed edge from one node to another. Produces a new version of the source node.",
            "inputSchema": {"type": "object", "properties": {
                "from": {"type": "string", "description": "Source node logical id (gains the edge)."},
                "to": {"type": "string", "description": "Target node logical id."},
                "rel": {"type": "string", "description": "Relationship kind (default depends_on)."},
                "author": author
            }, "required": ["from", "to"], "additionalProperties": false}
        },
        {
            "name": "edit_node",
            "description": "Edit a node's title and/or body, producing a new immutable version.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string"},
                "title": {"type": "string"},
                "body": {"type": "string"},
                "author": author
            }, "required": ["id"], "additionalProperties": false}
        },
        {
            "name": "complete_node",
            "description": "Complete a node: git-commit the produced files, store that commit on a new version, record used context, and mark it done. Requires a clean tree apart from the declared outputs.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string"},
                "outputs": paths("Produced files to commit, relative to the project root."),
                "context": paths("Files used while working but not declared inputs (pinned by content)."),
                "message": {"type": "string", "description": "Output commit message (defaults to the node's type and title)."},
                "author": author
            }, "required": ["id", "outputs"], "additionalProperties": false}
        },
        {
            "name": "set_status",
            "description": "Append a status event to a node.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string"},
                "status": enum_prop(&["open", "in_progress", "done", "failed"], "New status."),
                "author": author
            }, "required": ["id", "status"], "additionalProperties": false}
        },
        {
            "name": "show_node",
            "description": "Show a node: current version, edges, inputs, context, output, and any staleness reasons.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"], "additionalProperties": false}
        },
        {
            "name": "list_nodes",
            "description": "List every node with its current status, type, and title.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "node_history",
            "description": "Show a node's version history, newest first.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"], "additionalProperties": false}
        },
        {
            "name": "stale_nodes",
            "description": "List nodes that are stale, with explicit reasons (moved edges, changed inputs/outputs, superseded completions).",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "ready_nodes",
            "description": "List unfinished nodes whose dependencies are all satisfied (done and not stale).",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "blocked_nodes",
            "description": "List nodes blocked by an unsatisfied dependency, with reasons.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}
        },
        {
            "name": "node_origin",
            "description": "Find which node version produced a given output commit.",
            "inputSchema": {"type": "object", "properties": {"commit": {"type": "string"}}, "required": ["commit"], "additionalProperties": false}
        },
        {
            "name": "node_outputs",
            "description": "Show the output commit a node produced, if any.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"], "additionalProperties": false}
        }
    ])
}

fn initialize_result(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "llaundry-mcp", "version": env!("CARGO_PKG_VERSION") },
    })
}

// --- JSON-RPC helpers -------------------------------------------------------

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn write_msg(out: &mut impl Write, msg: &Value) -> Result<()> {
    writeln!(out, "{}", serde_json::to_string(msg)?)?;
    out.flush()?;
    Ok(())
}

// --- argument extraction ----------------------------------------------------

fn req_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("missing required string argument `{key}`"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn str_list(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// Parse an enum-valued argument, falling back to `default` when absent or null.
fn enum_or<T: DeserializeOwned>(args: &Value, key: &str, default: T) -> Result<T> {
    match args.get(key) {
        Some(v) if !v.is_null() => {
            serde_json::from_value(v.clone()).with_context(|| format!("invalid value for `{key}`"))
        }
        _ => Ok(default),
    }
}

/// Parse a required enum-valued argument.
fn enum_req<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T> {
    let v = args
        .get(key)
        .filter(|v| !v.is_null())
        .with_context(|| format!("missing required argument `{key}`"))?;
    serde_json::from_value(v.clone()).with_context(|| format!("invalid value for `{key}`"))
}

/// Join lines, or return `empty` when there are none.
fn joined(lines: Vec<String>, empty: &str) -> String {
    if lines.is_empty() {
        empty.to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_server() -> (TempDir, Server) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("llaundry-mcp-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let server = Server {
            store_path: root.join(".llaundry"),
        };
        (TempDir(root), server)
    }

    #[test]
    fn initialize_reports_server_and_protocol() {
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                         "params": {"protocolVersion": "2025-06-18"}});
        let (_t, server) = temp_server();
        let resp = server.handle(&req).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(resp["result"]["serverInfo"]["name"], "llaundry-mcp");
        assert!(resp["result"]["capabilities"].get("tools").is_some());
    }

    #[test]
    fn notifications_get_no_response() {
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        let (_t, server) = temp_server();
        assert!(server.handle(&note).is_none());
    }

    #[test]
    fn tools_list_advertises_the_catalogue() {
        let req = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
        let (_t, server) = temp_server();
        let resp = server.handle(&req).unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for expected in ["init_store", "add_node", "list_nodes", "complete_node", "node_origin"] {
            assert!(names.contains(&expected), "missing tool {expected} in {names:?}");
        }
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let req = json!({"jsonrpc": "2.0", "id": 3, "method": "no/such"});
        let (_t, server) = temp_server();
        let resp = server.handle(&req).unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn init_then_list_over_tools_call() {
        let (_t, server) = temp_server();

        // init_store creates the store; a fresh store lists no nodes.
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "init_store", "arguments": {}}});
        let resp = server.handle(&init).unwrap();
        assert_eq!(resp["result"]["isError"], false);

        let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                          "params": {"name": "list_nodes", "arguments": {}}});
        let resp = server.handle(&list).unwrap();
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["content"][0]["text"], "(no nodes)");
    }

    #[test]
    fn tool_errors_are_reported_in_band() {
        let (_t, server) = temp_server();
        Store::init(server.store_path.clone()).unwrap();

        // show_node without its required `id` -> isError, not a protocol failure.
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "show_node", "arguments": {}}});
        let resp = server.handle(&call).unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("`id`"));
    }
}
