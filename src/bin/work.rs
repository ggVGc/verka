//! `llaundry-work` — the driver for doing work on a llaundry node with an LLM.
//!
//! It launches one unit of work on one node: it loads the node, builds a prompt
//! from it, and hands that to a pluggable [`Backend`]. The backend is the LLM seam,
//! so different engines can be dropped in without touching the launcher.
//!
//! The first backend, [`ClaudeCode`], shells out to `claude -p` deliberately
//! sandboxed: it grants **only** the `llaundry` MCP server's tools (`--allowedTools
//! mcp__llaundry`) and no built-in tools at all, and pins MCP config to just that
//! server (`--strict-mcp-config`). So the model can act on the graph and nothing
//! else — no shell, no file, no network access.
//!
//! Every session's interaction stream is *streamed* to the node's `work.jsonl`
//! as it happens, one flushed line per event, so an abrupt exit (Ctrl-C, crash,
//! kill) loses at most an unflushed tail. The store's mutating operations
//! tolerate a dirty work log — the one exception to the clean-tree rule — and
//! sweep the story-so-far into their own commits; the driver commits the tail
//! when the session ends. Continuity across sessions is mechanical, not left
//! to agent discipline: when a node is re-worked while still mid-unit (open,
//! no result — e.g. it paused on a question node), the previous log is
//! replayed into the new session's prompt so it continues where it left off.

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use serde_json::{json, Value};
use std::process::Command;

use llaundry::{ops, Author, GitVcs, NodeMeta, Store};

#[derive(Parser)]
#[command(name = "llaundry-work", version, about = "Run an LLM against a llaundry node")]
struct Cli {
    /// Id of the node to work on.
    node: String,
    /// Path to the store directory.
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry")]
    store: std::path::PathBuf,
    /// Which LLM backend to run the work with.
    #[arg(long, value_enum, default_value = "claude-code")]
    backend: BackendKind,
    /// The Claude Code executable.
    #[arg(long, default_value = "claude")]
    claude_bin: String,
    /// Path to the `llaundry-mcp` executable the model is allowed to use.
    #[arg(long, default_value = "llaundry-mcp")]
    mcp_bin: String,
    /// Model to request from the backend (backend default if unset).
    #[arg(long)]
    model: Option<String>,
    /// Print the backend invocation instead of running it.
    #[arg(long)]
    dry_run: bool,
    /// Work the node even if its dependencies are unsatisfied.
    #[arg(long)]
    force: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum BackendKind {
    ClaudeCode,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let store = Store::open(cli.store.clone())?;
    let vcs = GitVcs::new(store.project_root());
    let (meta, body) = store.read_node(&cli.node)?;

    // A node assigned to a human is not an LLM's to work — it is waiting for a
    // human's answer (typically a question node minted by a paused worker).
    if meta.assignee == Some(Author::Human) && !cli.force {
        bail!(
            "node `{}` is assigned to a human; answer it (llaundry complete) or pass --force",
            cli.node
        );
    }

    // Don't launch work on a node whose dependencies aren't satisfied, unless forced.
    let blockers = ops::blockers(&store, &vcs, &cli.node);
    if !blockers.is_empty() && !cli.force {
        bail!(
            "node `{}` is blocked; resolve its dependencies or pass --force:\n  {}",
            cli.node,
            blockers.join("\n  ")
        );
    }

    // Open with no result and a recorded log means a paused unit of work (it
    // stopped mid-unit, e.g. on a question node): replay the log so the new
    // session continues where the last one left off. A node with a result is
    // being *re*worked — its old story doesn't continue, it starts over.
    let previous_log = if store.read_result(&cli.node)?.is_none() {
        store.read_work_log(&cli.node)?
    } else {
        None
    };

    let session = Session {
        node_id: cli.node.clone(),
        prompt: build_prompt(&cli.node, &meta, &body, previous_log.as_deref()),
        mcp: McpServer {
            name: "llaundry".into(),
            command: cli.mcp_bin.clone(),
            args: vec!["--store".into(), cli.store.to_string_lossy().into_owned()],
        },
    };

    let backend: Box<dyn Backend> = match cli.backend {
        BackendKind::ClaudeCode => Box::new(ClaudeCode {
            binary: cli.claude_bin.clone(),
            model: cli.model.clone(),
        }),
    };

    if cli.dry_run {
        println!("{}", backend.describe(&session));
        return Ok(());
    }

    eprintln!(
        "llaundry-work: {} working {} — {}{}",
        backend.name(),
        session.node_id,
        meta.title,
        if previous_log.is_some() { " (continuing)" } else { "" }
    );

    // The log is streamed during the session (the one dirty path the clean-tree
    // rule tolerates), so an abrupt exit loses at most an unflushed tail.
    // Append to a paused unit of work; start over on rework. The attempt header
    // is stamped at launch: it records which definition this session set out to
    // work, even if the agent edits the graph during it.
    use std::io::Write;
    let mut log = store.open_work_log(&cli.node, previous_log.is_some())?;
    writeln!(
        log,
        "{}",
        json!({
            "event": "attempt",
            "at": now_millis(),
            "backend": backend.name(),
            "node_version": store.node_version(&cli.node)?,
        })
    )?;
    log.flush()?;

    let success = backend.run(&session, &mut log)?;
    drop(log);

    // Commit whatever of the story the session's own store commits didn't
    // already sweep in — before judging the exit status, so even a failed
    // session's story is kept.
    ops::commit_work_log(&store, &vcs, &cli.node)?;

    if !success {
        bail!("backend session exited unsuccessfully");
    }
    Ok(())
}

/// One unit of work: an LLM session focused on a single node, together with the MCP
/// server the model is allowed to use. Deliberately free of any store handle — once
/// the prompt is built, a backend needs nothing else from the database.
struct Session {
    node_id: String,
    prompt: String,
    mcp: McpServer,
}

/// A stdio MCP server the backend should expose to the model.
struct McpServer {
    name: String,
    command: String,
    args: Vec<String>,
}

/// The LLM seam. An implementation runs a [`Session`] to completion, streaming
/// each JSONL transcript line to `log` as it arrives (flushed per line, so the
/// story survives an abrupt exit), and returns whether the backend exited
/// cleanly. `describe` renders the invocation for `--dry-run` without running
/// anything.
trait Backend {
    fn name(&self) -> &str;
    fn run(&self, session: &Session, log: &mut dyn std::io::Write) -> Result<bool>;
    fn describe(&self, session: &Session) -> String;
}

/// Backend that shells out to Claude Code (`claude -p`), sandboxed to the llaundry
/// MCP: no built-in tools, no other MCP servers.
struct ClaudeCode {
    binary: String,
    model: Option<String>,
}

impl ClaudeCode {
    /// Build the `claude` invocation for a session. Kept separate from [`run`] so it
    /// can be inspected in tests and printed by `--dry-run` without executing.
    fn command(&self, session: &Session) -> Command {
        // `--mcp-config` takes inline JSON; the server name is dynamic, so build the
        // object rather than using a string-literal key.
        let mut servers = serde_json::Map::new();
        servers.insert(
            session.mcp.name.clone(),
            json!({ "command": session.mcp.command, "args": session.mcp.args }),
        );
        let mcp_config = json!({ "mcpServers": Value::Object(servers) });

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-p")
            .arg(&session.prompt)
            // Only our MCP server, ignoring any user/project MCP config.
            .arg("--mcp-config")
            .arg(mcp_config.to_string())
            .arg("--strict-mcp-config")
            // Allow every tool from the llaundry server and nothing else. In `-p`
            // mode any tool not listed here is denied, so no built-ins are reachable.
            .arg("--allowedTools")
            .arg(format!("mcp__{}", session.mcp.name))
            // One JSON event per line — the session transcript recorded to the
            // node's work.jsonl (stream-json in print mode requires --verbose).
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        cmd
    }
}

impl Backend for ClaudeCode {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn run(&self, session: &Session, log: &mut dyn std::io::Write) -> Result<bool> {
        use std::io::{BufRead, BufReader};
        // Stream stdout (the JSONL event stream) to the log, teeing each line to
        // the terminal as it arrives; stderr stays inherited.
        let mut child = self
            .command(session)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to launch `{}` — is Claude Code installed and on PATH?",
                    self.binary
                )
            })?;
        let stdout = child.stdout.take().expect("stdout was piped");
        for line in BufReader::new(stdout).lines() {
            let line = line.context("reading backend output")?;
            println!("{line}");
            writeln!(log, "{line}").context("writing work log")?;
            log.flush().context("flushing work log")?;
        }
        let status = child.wait()?;
        Ok(status.success())
    }

    fn describe(&self, session: &Session) -> String {
        let cmd = self.command(session);
        let mut parts = vec![cmd.get_program().to_string_lossy().into_owned()];
        parts.extend(cmd.get_args().map(|a| shell_quote(&a.to_string_lossy())));
        parts.join(" ")
    }
}

/// The instruction handed to the model: what the node is, and the tools-only
/// discipline it must follow. A file-free session produces no output files, so it
/// finishes with `complete_node` (no outputs, notes as the record of what happened)
/// or `fail_node` — or pauses on a human-assigned question node.
///
/// On continuation, `previous_log` (the node's recorded `work.jsonl`) is replayed
/// verbatim so the session picks up exactly where the last one stopped — the
/// handoff is mechanical, not dependent on the previous agent having left notes.
fn build_prompt(id: &str, meta: &NodeMeta, body: &str, previous_log: Option<&str>) -> String {
    let mut p = vec![
        "You are an autonomous worker on a llaundry node graph.".to_string(),
        "You can act ONLY through the `llaundry` MCP tools — you have no shell, file,"
            .to_string(),
        "or network access. Every change you make must go through those tools."
            .to_string(),
        String::new(),
        format!("You are assigned to node `{id}`:"),
        format!("  title: {}", meta.title),
    ];
    for dep in &meta.depends_on {
        p.push(format!("  depends_on -> {dep}"));
    }
    for src in &meta.derived_from {
        p.push(format!("  derived_from -> {src}"));
    }
    let body = body.trim();
    if !body.is_empty() {
        p.push(String::new());
        p.push("Node description:".into());
        p.push(body.to_string());
    }
    if let Some(log) = previous_log {
        p.push(String::new());
        p.push("You already started this node in an earlier session. Its recorded".into());
        p.push("interaction log follows (JSONL, oldest first). Continue from where it".into());
        p.push("ends — do not redo work it already records. If it ends with a question".into());
        p.push("node being created, that question's answer is now in its result notes".into());
        p.push("(show_node it).".into());
        p.push(String::new());
        p.push(log.trim_end().to_string());
    }
    p.push(String::new());
    p.push("Do this:".into());
    p.push("  1. Gather what you need with show_node, list_nodes, node_dependents and".into());
    p.push("     the stale/ready/blocked queries.".into());
    p.push("  2. Carry out the node's work by updating the graph — add_node, link_nodes,".into());
    p.push("     edit_node — keeping each change small and clearly described.".into());
    p.push(format!(
        "  3. When the work is done, record it: complete_node {id}, with `notes`"
    ));
    p.push("     summarising what you did and why. If the work cannot be done,".into());
    p.push(format!("     record that instead: fail_node {id} with `notes` explaining why."));
    p.push("  4. If you need a decision or information only a human can give, do NOT".into());
    p.push("     complete or fail. Instead add_node a question (title `Question: ...`,".into());
    p.push("     assignee `human`, the context and options in its body), link_nodes".into());
    p.push(format!(
        "     {id} depends_on it, and stop. Work resumes here once it is answered."
    ));
    p.push("Finish with a brief summary of what you changed.".into());
    p.join("\n")
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Quote an argument for a copy-pasteable single-line command display.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llaundry::Author;

    fn sample_session() -> Session {
        Session {
            node_id: "node-1".into(),
            prompt: "do the work".into(),
            mcp: McpServer {
                name: "llaundry".into(),
                command: "llaundry-mcp".into(),
                args: vec!["--store".into(), ".llaundry".into()],
            },
        }
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn claude_command_sandboxes_to_only_the_llaundry_mcp() {
        let backend = ClaudeCode {
            binary: "claude".into(),
            model: None,
        };
        let cmd = backend.command(&sample_session());
        assert_eq!(cmd.get_program().to_string_lossy(), "claude");
        let args = args_of(&cmd);

        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--strict-mcp-config".to_string()));

        // The transcript is captured as one JSON event per line for work.jsonl.
        let k = args.iter().position(|a| a == "--output-format").unwrap();
        assert_eq!(args[k + 1], "stream-json");
        assert!(args.contains(&"--verbose".to_string()));

        // Allowed tools are exactly the whole llaundry MCP server — nothing else.
        let i = args.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(args[i + 1], "mcp__llaundry");

        // No built-in tools are granted, and permissions are not bypassed.
        assert!(!args.iter().any(|a| a.contains("Bash")));
        assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"));

        // The MCP config points at the llaundry-mcp binary and passes the store.
        let j = args.iter().position(|a| a == "--mcp-config").unwrap();
        let cfg: Value = serde_json::from_str(&args[j + 1]).unwrap();
        assert_eq!(cfg["mcpServers"]["llaundry"]["command"], "llaundry-mcp");
        assert_eq!(cfg["mcpServers"]["llaundry"]["args"][0], "--store");
        assert_eq!(cfg["mcpServers"]["llaundry"]["args"][1], ".llaundry");
    }

    #[test]
    fn model_is_forwarded_only_when_set() {
        let without = ClaudeCode {
            binary: "claude".into(),
            model: None,
        };
        assert!(!args_of(&without.command(&sample_session())).contains(&"--model".to_string()));

        let with = ClaudeCode {
            binary: "claude".into(),
            model: Some("opus".into()),
        };
        let args = args_of(&with.command(&sample_session()));
        let i = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(args[i + 1], "opus");
    }

    #[test]
    fn describe_is_a_copy_pasteable_command() {
        let backend = ClaudeCode {
            binary: "claude".into(),
            model: None,
        };
        let d = backend.describe(&sample_session());
        assert!(d.starts_with("claude "));
        assert!(d.contains("mcp__llaundry"));
        // The multi-word prompt gets quoted into a single token.
        assert!(d.contains("'do the work'"));
    }

    #[test]
    fn prompt_states_the_node_and_the_tools_only_rule() {
        let meta = NodeMeta {
            schema: 1,
            title: "Parse config".into(),
            author: Author::Human,
            assignee: None,
            depends_on: vec!["node-0".into()],
            derived_from: vec![],
        };
        let prompt = build_prompt("node-1", &meta, "  Write the config parser.  ", None);

        assert!(prompt.contains("node-1"));
        assert!(prompt.contains("Parse config"));
        assert!(prompt.contains("llaundry` MCP tools"));
        assert!(prompt.contains("complete_node node-1"));
        assert!(prompt.contains("fail_node node-1"));
        assert!(prompt.contains("depends_on -> node-0"));
        assert!(prompt.contains("Write the config parser."));

        // The pause protocol: a human question is a node, not a completion.
        assert!(prompt.contains("Question:"));
        assert!(prompt.contains("assignee `human`"));

        // No continuation section on a first attempt.
        assert!(!prompt.contains("earlier session"));
    }

    #[test]
    fn prompt_replays_the_previous_log_on_continuation() {
        let meta = NodeMeta {
            schema: 1,
            title: "Parse config".into(),
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
        };
        let log = "{\"event\":\"attempt\"}\n{\"tool\":\"add_node\"}\n";
        let prompt = build_prompt("node-1", &meta, "", Some(log));

        assert!(prompt.contains("earlier session"));
        assert!(prompt.contains("Continue from where it"));
        // The log is included verbatim (sans trailing newline).
        assert!(prompt.contains("{\"event\":\"attempt\"}\n{\"tool\":\"add_node\"}"));
    }
}
