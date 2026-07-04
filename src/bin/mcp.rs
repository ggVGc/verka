//! `llaundry-mcp` — a Model Context Protocol server over the llaundry library.
//!
//! It speaks JSON-RPC 2.0 over stdio, the way MCP's stdio transport expects:
//! one JSON message per line, requests answered on stdout, everything else (logs)
//! on stderr. It is deliberately synchronous and dependency-light, matching the
//! rest of the project — the loop reads a line, dispatches, writes a line.
//!
//! Each tool is its own type implementing [`Tool`]: it carries its name, its JSON
//! schema, and its behaviour together, and the [`registry`] lists them. Adding a
//! tool is adding a type and one registry entry — no central match to keep in sync.
//! The bodies are thin wrappers over `llaundry::ops`, so an agent gets the same
//! operations as the CLI.
//!
//! A [`Ctx`] opens the store fresh for each call, so `initialize`/`tools/list` work
//! even before a store exists and an agent can create one with `init_store`.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use llaundry::ops::{self, NewNode};
use llaundry::{Author, DepKind, GitVcs, NodeType, Store};

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
    Server::new(cli.store).serve()
}

// --- tools ------------------------------------------------------------------

/// One MCP tool: its identity, its input schema, and what it does. Each concrete
/// tool is a zero-sized type implementing this trait.
trait Tool {
    /// The tool name the client calls (e.g. `add_node`).
    fn name(&self) -> &'static str;
    /// One-line human description, shown in `tools/list`.
    fn description(&self) -> &'static str;
    /// JSON Schema for the `arguments` object.
    fn input_schema(&self) -> Value;
    /// Run the tool, returning display text. Errors become an in-band MCP error.
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String>;
}

/// What a tool needs from the environment: the store path, opened fresh per call so
/// nothing is cached across a long-running session.
struct Ctx {
    store_path: PathBuf,
}

impl Ctx {
    /// Open the store and wire up the real git seam for one operation.
    fn open(&self) -> Result<(Store, GitVcs)> {
        let store = Store::open(self.store_path.clone())?;
        let vcs = GitVcs::new(store.project_root());
        Ok((store, vcs))
    }
}

/// Every tool the server exposes, in `tools/list` order.
fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(InitStore),
        Box::new(AddNode),
        Box::new(LinkNodes),
        Box::new(EditNode),
        Box::new(CompleteNode),
        Box::new(FailNode),
        Box::new(ShowNode),
        Box::new(ListNodes),
        Box::new(StaleNodes),
        Box::new(ReadyNodes),
        Box::new(BlockedNodes),
        Box::new(NodeOrigin),
        Box::new(NodeOutputs),
        Box::new(NodeDependents),
        Box::new(CheckStore),
        Box::new(NodeSettled),
    ]
}

struct NodeSettled;
impl Tool for NodeSettled {
    fn name(&self) -> &'static str {
        "node_settled"
    }
    fn description(&self) -> &'static str {
        "Check whether a node is settled: done, not stale, and all work derived from it (transitively) also done and not stale. Lists what is still outstanding otherwise."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({ "id": {"type": "string"} }), &["id"])
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let id = req_str(args, "id")?;
        let reasons = ops::unsettled(&store, &vcs, &id)?;
        Ok(if reasons.is_empty() {
            format!("{id}: settled")
        } else {
            let mut lines = vec![format!("{id}: not settled")];
            lines.extend(reasons.iter().map(|r| format!("  {r}")));
            lines.join("\n")
        })
    }
}

struct CheckStore;
impl Tool for CheckStore {
    fn name(&self) -> &'static str {
        "check_store"
    }
    fn description(&self) -> &'static str {
        "Integrity-check the store (fsck): parse errors, missing or ill-typed edge targets, duplicates, self-references, and dependency cycles."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({}), &[])
    }
    fn call(&self, ctx: &Ctx, _args: &Value) -> Result<String> {
        let (store, _) = ctx.open()?;
        Ok(joined(ops::check(&store)?, "store is consistent"))
    }
}

struct InitStore;
impl Tool for InitStore {
    fn name(&self) -> &'static str {
        "init_store"
    }
    fn description(&self) -> &'static str {
        "Initialise a new llaundry store (creates the store directory skeleton)."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({}), &[])
    }
    fn call(&self, ctx: &Ctx, _args: &Value) -> Result<String> {
        Store::init(ctx.store_path.clone())?;
        Ok(format!("initialised store at {}", ctx.store_path.display()))
    }
}

struct AddNode;
impl Tool for AddNode {
    fn name(&self) -> &'static str {
        "add_node"
    }
    fn description(&self) -> &'static str {
        "Create a new node in the graph. Returns its id. Edge type rules: a task may link to tasks; an implementation to tasks and builds; a build to implementations; a verification to implementations and builds."
    }
    fn input_schema(&self) -> Value {
        obj_schema(
            json!({
                "type": enum_prop(&["task", "implementation", "build", "verification"], "Node type (default task)."),
                "title": {"type": "string", "description": "Short title."},
                "body": {"type": "string", "description": "Prose body (markdown)."},
                "author": author_prop(),
                "depends_on": ids_prop("Ids of nodes this node depends on."),
                "derived_from": ids_prop("Ids of nodes this node is derived from.")
            }),
            &["title"],
        )
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let new = NewNode {
            node_type: enum_or(args, "type", NodeType::Task)?,
            title: req_str(args, "title")?,
            body: opt_str(args, "body").unwrap_or_default(),
            author: enum_or(args, "author", Author::Machine)?,
            depends_on: str_list(args, "depends_on"),
            derived_from: str_list(args, "derived_from"),
        };
        let id = ops::add(&store, &vcs, new)?;
        Ok(format!("created {id}"))
    }
}

struct LinkNodes;
impl Tool for LinkNodes {
    fn name(&self) -> &'static str {
        "link_nodes"
    }
    fn description(&self) -> &'static str {
        "Add a dependency from one node to another (a definition change of the source node). Subject to the edge type rules: task -> task; implementation -> task/build; build -> implementation; verification -> implementation/build."
    }
    fn input_schema(&self) -> Value {
        obj_schema(
            json!({
                "from": {"type": "string", "description": "Source node id (gains the dependency)."},
                "to": {"type": "string", "description": "Target node id."},
                "rel": enum_prop(&["depends_on", "derived_from"], "Which list to add to (default depends_on).")
            }),
            &["from", "to"],
        )
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let from = req_str(args, "from")?;
        let to = req_str(args, "to")?;
        let rel: DepKind = enum_or(args, "rel", DepKind::DependsOn)?;
        ops::link(&store, &vcs, &from, &to, rel)?;
        Ok(format!("{from} +{} -> {to}", rel.as_str()))
    }
}

struct EditNode;
impl Tool for EditNode {
    fn name(&self) -> &'static str {
        "edit_node"
    }
    fn description(&self) -> &'static str {
        "Edit a node's title and/or body. A definition change: it reopens a done node and makes dependents' pins stale."
    }
    fn input_schema(&self) -> Value {
        obj_schema(
            json!({
                "id": {"type": "string"},
                "title": {"type": "string"},
                "body": {"type": "string"}
            }),
            &["id"],
        )
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let id = req_str(args, "id")?;
        let title = opt_str(args, "title");
        let body = opt_str(args, "body");
        ops::edit(&store, &vcs, &id, title, body)?;
        Ok(format!("edited {id} (now {})", ops::short(&store.node_version(&id)?)))
    }
}

struct CompleteNode;
impl Tool for CompleteNode {
    fn name(&self) -> &'static str {
        "complete_node"
    }
    fn description(&self) -> &'static str {
        "Record a node's work as done: git-commit any produced files as one output commit, pin the dependency versions and context the work was built against, and write result.md with your notes. `outputs` may be empty for graph-only work."
    }
    fn input_schema(&self) -> Value {
        obj_schema(
            json!({
                "id": {"type": "string"},
                "outputs": paths_prop("Produced files to commit, relative to the project root. Omit for graph-only work."),
                "context": paths_prop("Consumed files that are not any node's output (pinned by content)."),
                "message": {"type": "string", "description": "Output commit message (defaults to the node's type and title)."},
                "notes": {"type": "string", "description": "Narrative of what happened during the work — becomes the body of result.md."},
                "author": author_prop()
            }),
            &["id"],
        )
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let id = req_str(args, "id")?;
        let outputs = str_list(args, "outputs");
        let context = str_list(args, "context");
        let message = opt_str(args, "message");
        let notes = opt_str(args, "notes").unwrap_or_default();
        let author = enum_or(args, "author", Author::Machine)?;
        let commit = ops::complete(&store, &vcs, &id, &outputs, &context, message, &notes, author)?;
        Ok(match commit {
            Some(c) => format!("completed {id} (output commit {})", ops::short(&c)),
            None => format!("completed {id} (no output files)"),
        })
    }
}

struct FailNode;
impl Tool for FailNode {
    fn name(&self) -> &'static str {
        "fail_node"
    }
    fn description(&self) -> &'static str {
        "Record a node's work as failed, with notes on what went wrong. The node stays ready to retry; its dependents stay blocked."
    }
    fn input_schema(&self) -> Value {
        obj_schema(
            json!({
                "id": {"type": "string"},
                "notes": {"type": "string", "description": "What was attempted and why it failed."},
                "author": author_prop()
            }),
            &["id"],
        )
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let id = req_str(args, "id")?;
        let notes = opt_str(args, "notes").unwrap_or_default();
        let author = enum_or(args, "author", Author::Machine)?;
        ops::fail(&store, &vcs, &id, &notes, author)?;
        Ok(format!("{id} -> failed"))
    }
}

struct ShowNode;
impl Tool for ShowNode {
    fn name(&self) -> &'static str {
        "show_node"
    }
    fn description(&self) -> &'static str {
        "Show a node: definition, derived status, dependencies, result (outcome, output commit, pins, notes), and staleness reasons."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({ "id": {"type": "string"} }), &["id"])
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let id = req_str(args, "id")?;
        let (meta, body) = store.read_node(&id)?;

        let mut lines = vec![
            format!("id:      {id}"),
            format!("type:    {}", meta.node_type.as_str()),
            format!("title:   {}", meta.title),
            format!("status:  {}", ops::current_status(&store, &id).as_str()),
            format!("author:  {}", meta.author.as_str()),
            format!("version: {}", ops::short(&store.node_version(&id)?)),
        ];
        for dep in &meta.depends_on {
            lines.push(format!("depends_on:   {dep}"));
        }
        for src in &meta.derived_from {
            lines.push(format!("derived_from: {src}"));
        }
        if let Some((result, notes)) = store.read_result(&id)? {
            lines.push("result:".into());
            lines.push(format!("  outcome: {}", result.outcome.as_str()));
            lines.push(format!("  author:  {}", result.author.as_str()));
            if let Some(commit) = &result.output_commit {
                lines.push(format!("  output:  commit {}", ops::short(commit)));
            }
            for ba in &result.built_against {
                lines.push(match &ba.output {
                    Some(o) => format!(
                        "  built against {} @ {} (output {})",
                        ba.id,
                        ops::short(&ba.pin),
                        ops::short(o)
                    ),
                    None => format!("  built against {} @ {}", ba.id, ops::short(&ba.pin)),
                });
            }
            for pin in &result.context {
                lines.push(format!("  context {} @ {}", pin.path, ops::short(&pin.blob)));
            }
            let notes = notes.trim_end();
            if !notes.is_empty() {
                lines.push("  notes:".into());
                lines.extend(notes.lines().map(|l| format!("    {l}")));
            }
        }
        let reasons = ops::staleness(&store, &vcs, &id);
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
}

struct ListNodes;
impl Tool for ListNodes {
    fn name(&self) -> &'static str {
        "list_nodes"
    }
    fn description(&self) -> &'static str {
        "List every node with its derived status, type, and title."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({}), &[])
    }
    fn call(&self, ctx: &Ctx, _args: &Value) -> Result<String> {
        let (store, _) = ctx.open()?;
        let mut lines = Vec::new();
        for id in store.list_ids()? {
            let (meta, _) = store.read_node(&id)?;
            let status = ops::current_status(&store, &id);
            lines.push(format!(
                "{id}  [{}]  {}  {}",
                status.as_str(),
                meta.node_type.as_str(),
                meta.title
            ));
        }
        Ok(joined(lines, "(no nodes)"))
    }
}

struct StaleNodes;
impl Tool for StaleNodes {
    fn name(&self) -> &'static str {
        "stale_nodes"
    }
    fn description(&self) -> &'static str {
        "List nodes whose recorded work has been invalidated, with explicit reasons (moved dependency definitions or outputs, changed context, drifted outputs, edited definitions)."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({}), &[])
    }
    fn call(&self, ctx: &Ctx, _args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let mut lines = Vec::new();
        for id in store.list_ids()? {
            let reasons = ops::staleness(&store, &vcs, &id);
            if !reasons.is_empty() {
                lines.push(format!("{id}:"));
                lines.extend(reasons.iter().map(|r| format!("  {r}")));
            }
        }
        Ok(joined(lines, "all nodes up to date"))
    }
}

struct ReadyNodes;
impl Tool for ReadyNodes {
    fn name(&self) -> &'static str {
        "ready_nodes"
    }
    fn description(&self) -> &'static str {
        "List unfinished nodes whose dependencies are all satisfied (done and not stale)."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({}), &[])
    }
    fn call(&self, ctx: &Ctx, _args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let mut lines = Vec::new();
        for id in store.list_ids()? {
            if ops::is_ready(&store, &vcs, &id) {
                let (meta, _) = store.read_node(&id)?;
                lines.push(format!("{id}  {}", meta.title));
            }
        }
        Ok(joined(lines, "(nothing ready)"))
    }
}

struct BlockedNodes;
impl Tool for BlockedNodes {
    fn name(&self) -> &'static str {
        "blocked_nodes"
    }
    fn description(&self) -> &'static str {
        "List nodes blocked by an unsatisfied dependency, with reasons."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({}), &[])
    }
    fn call(&self, ctx: &Ctx, _args: &Value) -> Result<String> {
        let (store, vcs) = ctx.open()?;
        let mut lines = Vec::new();
        for id in store.list_ids()? {
            let blockers = ops::blockers(&store, &vcs, &id);
            if !blockers.is_empty() {
                lines.push(format!("{id}:"));
                lines.extend(blockers.iter().map(|b| format!("  blocked by {b}")));
            }
        }
        Ok(joined(lines, "nothing blocked"))
    }
}

struct NodeOrigin;
impl Tool for NodeOrigin {
    fn name(&self) -> &'static str {
        "node_origin"
    }
    fn description(&self) -> &'static str {
        "Find which node's work produced a given output commit."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({ "commit": {"type": "string"} }), &["commit"])
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, _) = ctx.open()?;
        let commit = req_str(args, "commit")?;
        Ok(match ops::origin(&store, &commit)? {
            Some(id) => id,
            None => format!("no node produced {}", ops::short(&commit)),
        })
    }
}

struct NodeOutputs;
impl Tool for NodeOutputs {
    fn name(&self) -> &'static str {
        "node_outputs"
    }
    fn description(&self) -> &'static str {
        "Show the output commit a node produced, if any."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({ "id": {"type": "string"} }), &["id"])
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, _) = ctx.open()?;
        let id = req_str(args, "id")?;
        if !store.exists(&id) {
            anyhow::bail!("unknown node `{id}`");
        }
        Ok(match ops::output_of(&store, &id) {
            Some(commit) => commit,
            None => format!("{id} has produced no output"),
        })
    }
}

struct NodeDependents;
impl Tool for NodeDependents {
    fn name(&self) -> &'static str {
        "node_dependents"
    }
    fn description(&self) -> &'static str {
        "List the nodes that depend on (or derive from) a node — the follow-up work affected when it changes."
    }
    fn input_schema(&self) -> Value {
        obj_schema(json!({ "id": {"type": "string"} }), &["id"])
    }
    fn call(&self, ctx: &Ctx, args: &Value) -> Result<String> {
        let (store, _) = ctx.open()?;
        let id = req_str(args, "id")?;
        if !store.exists(&id) {
            anyhow::bail!("unknown node `{id}`");
        }
        Ok(joined(ops::dependents(&store, &id)?, "(no dependents)"))
    }
}

// --- server -----------------------------------------------------------------

struct Server {
    store_path: PathBuf,
    tools: Vec<Box<dyn Tool>>,
}

impl Server {
    fn new(store_path: PathBuf) -> Self {
        Server {
            store_path,
            tools: registry(),
        }
    }

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
            "tools/list" => rpc_ok(id, json!({ "tools": self.tool_specs() })),
            "tools/call" => rpc_ok(id, self.call_tool(&params)),
            other => rpc_err(id, -32601, &format!("method not found: {other}")),
        })
    }

    /// The advertised tool catalogue, built from the registry.
    fn tool_specs(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name(),
                    "description": t.description(),
                    "inputSchema": t.input_schema(),
                })
            })
            .collect()
    }

    /// Run a `tools/call`. Tool *execution* failures are reported in-band as an MCP
    /// result with `isError: true` (per the spec), not as JSON-RPC protocol errors.
    fn call_tool(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let ctx = Ctx {
            store_path: self.store_path.clone(),
        };
        let result = match self.tools.iter().find(|t| t.name() == name) {
            Some(tool) => tool.call(&ctx, &args),
            None => Err(anyhow!("unknown tool `{name}`")),
        };
        match result {
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

// --- schema helpers ---------------------------------------------------------

/// An `object` schema over `props`, marking `required` keys and forbidding extras.
fn obj_schema(props: Value, required: &[&str]) -> Value {
    let mut schema = json!({ "type": "object", "properties": props, "additionalProperties": false });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn author_prop() -> Value {
    json!({ "type": "string", "enum": ["human", "machine"], "description": "Author of the change." })
}

fn paths_prop(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": desc })
}

fn ids_prop(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": desc })
}

fn enum_prop(values: &[&str], desc: &str) -> Value {
    json!({ "type": "string", "enum": values, "description": desc })
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
        let store_path = root.join(".llaundry");
        (TempDir(root), Server::new(store_path))
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
        for expected in [
            "init_store",
            "add_node",
            "list_nodes",
            "complete_node",
            "fail_node",
            "node_origin",
            "node_dependents",
        ] {
            assert!(names.contains(&expected), "missing tool {expected} in {names:?}");
        }
        assert!(!names.contains(&"set_status"), "set_status was removed");
    }

    #[test]
    fn every_registered_tool_has_an_object_schema() {
        // The registry and the advertised catalogue stay in lock-step, and each tool
        // presents a well-formed object schema.
        let (_t, server) = temp_server();
        assert_eq!(server.tool_specs().len(), registry().len());
        for spec in server.tool_specs() {
            assert!(spec["name"].as_str().is_some());
            assert!(!spec["description"].as_str().unwrap().is_empty());
            assert_eq!(spec["inputSchema"]["type"], "object");
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
    fn unknown_tool_is_an_in_band_error() {
        let (_t, server) = temp_server();
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "no_such_tool", "arguments": {}}});
        let resp = server.handle(&call).unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
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
