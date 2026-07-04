//! `llaundry-work` — the driver for doing work on a llaundry node with an LLM.
//!
//! It launches a *session* focused on one node: it loads the node, builds a prompt
//! from it, and hands that to a pluggable [`Backend`]. The backend is the LLM seam,
//! so different engines can be dropped in without touching the launcher.
//!
//! The first backend, [`ClaudeCode`], shells out to `claude -p` deliberately
//! sandboxed: it grants **only** the `llaundry` MCP server's tools (`--allowedTools
//! mcp__llaundry`) and no built-in tools at all, and pins MCP config to just that
//! server (`--strict-mcp-config`). So the model can act on the graph and nothing
//! else — no shell, no file, no network access.

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use serde_json::{json, Value};
use std::process::Command;

use llaundry::{ops, GitVcs, Meta, Store};

#[derive(Parser)]
#[command(name = "llaundry-work", version, about = "Run an LLM session against a llaundry node")]
struct Cli {
    /// Logical id of the node to work on.
    node: String,
    /// Path to the store directory.
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry")]
    store: std::path::PathBuf,
    /// Which LLM backend to run the session with.
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
    let hash = store.get_ref(&cli.node)?;
    let (meta, body) = store.get_object(&hash)?;

    // Don't launch work on a node whose dependencies aren't satisfied, unless forced.
    let blockers = ops::blockers(&store, &vcs, &meta);
    if !blockers.is_empty() && !cli.force {
        bail!(
            "node `{}` is blocked; resolve its dependencies or pass --force:\n  {}",
            cli.node,
            blockers.join("\n  ")
        );
    }

    let session = Session {
        node_id: cli.node.clone(),
        prompt: build_prompt(&cli.node, &meta, &body),
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
        "llaundry-work: {} session on {} — {}",
        backend.name(),
        session.node_id,
        meta.title
    );
    backend.run(&session)
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

/// The LLM seam. An implementation runs a [`Session`] to completion; `describe`
/// renders the invocation for `--dry-run` without running anything.
trait Backend {
    fn name(&self) -> &str;
    fn run(&self, session: &Session) -> Result<()>;
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
            .arg("--output-format")
            .arg("text");
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

    fn run(&self, session: &Session) -> Result<()> {
        // Inherit stdio so the session streams to the terminal.
        let status = self.command(session).status().with_context(|| {
            format!(
                "failed to launch `{}` — is Claude Code installed and on PATH?",
                self.binary
            )
        })?;
        if !status.success() {
            bail!("{} exited unsuccessfully ({status})", self.binary);
        }
        Ok(())
    }

    fn describe(&self, session: &Session) -> String {
        let cmd = self.command(session);
        let mut parts = vec![cmd.get_program().to_string_lossy().into_owned()];
        parts.extend(cmd.get_args().map(|a| shell_quote(&a.to_string_lossy())));
        parts.join(" ")
    }
}

/// The instruction handed to the model: what the node is, and the tools-only
/// discipline it must follow. It steers completion toward `set_status … done`
/// rather than `complete_node`, since a file-free session produces no output files.
fn build_prompt(id: &str, meta: &Meta, body: &str) -> String {
    let mut p = vec![
        "You are an autonomous worker on a llaundry node graph.".to_string(),
        "You can act ONLY through the `llaundry` MCP tools — you have no shell, file,"
            .to_string(),
        "or network access. Every change you make must go through those tools."
            .to_string(),
        String::new(),
        format!("You are assigned to node `{id}`:"),
        format!("  type:  {}", meta.node_type.as_str()),
        format!("  title: {}", meta.title),
    ];
    if !meta.edges.is_empty() {
        p.push("  edges:".into());
        for e in &meta.edges {
            p.push(format!("    {} -> {}", e.rel, e.to));
        }
    }
    let body = body.trim();
    if !body.is_empty() {
        p.push(String::new());
        p.push("Node description:".into());
        p.push(body.to_string());
    }
    p.push(String::new());
    p.push("Do this:".into());
    p.push(format!("  1. Mark it in progress: set_status {id} in_progress."));
    p.push("  2. Gather what you need with show_node, list_nodes, node_history and the".into());
    p.push("     stale/ready/blocked queries.".into());
    p.push("  3. Carry out the node's work by updating the graph — add_node, link_nodes,".into());
    p.push("     edit_node — keeping each change small and clearly described.".into());
    p.push(format!("  4. When the work is done, record it: set_status {id} done."));
    p.push("Finish with a brief summary of what you changed.".into());
    p.join("\n")
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
    use llaundry::{Author, Edge, NodeType};

    fn sample_session() -> Session {
        Session {
            node_id: "task-1".into(),
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
        let meta = Meta {
            schema: 1,
            logical_id: "task-1".into(),
            node_type: NodeType::Task,
            title: "Parse config".into(),
            author: Author::Human,
            parent: None,
            output_commit: None,
            edges: vec![Edge {
                to: "task-0".into(),
                rel: "depends_on".into(),
                pin: "abc".into(),
            }],
            context: vec![],
        };
        let prompt = build_prompt("task-1", &meta, "  Write the config parser.  ");

        assert!(prompt.contains("task-1"));
        assert!(prompt.contains("Parse config"));
        assert!(prompt.contains("llaundry` MCP tools"));
        assert!(prompt.contains("set_status task-1 in_progress"));
        assert!(prompt.contains("set_status task-1 done"));
        assert!(prompt.contains("depends_on -> task-0"));
        assert!(prompt.contains("Write the config parser."));
    }
}
