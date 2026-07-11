//! `llaundry-work` — the driver for doing work on a llaundry node with an LLM.
//!
//! It launches one unit of work on one node: it loads the node, builds a prompt
//! from it, and hands that to a pluggable [`Backend`]. The backend is the LLM seam,
//! so different engines can be dropped in without touching the launcher. The
//! node is named on the command line, or picked with `--next`: the first ready
//! node that is ready for a machine worker ([`ops::first_ready_for`]).
//!
//! Which backend, model, and executables it uses default to the store's
//! optional [`Config`] (`<store>/config.toml`); an explicit `--flag` always
//! wins over the file, which wins over the built-in default.
//!
//! The first backend, [`ClaudeCode`], shells out to `claude -p` with a deliberate
//! tool grant: the `llaundry` MCP server for graph operations, plus the built-in
//! file tools, every one scoped to the session's working directory (`./**`) —
//! no shell, and the web tools only behind `--network`. MCP config is pinned
//! to just the llaundry server (`--strict-mcp-config`).
//!
//! Isolation is the workbench geometry (see ISOLATION.md): the session runs
//! inside `project/`, an ordinary git repository, and everything it is granted
//! lives below its working directory. The store — and the store's git history,
//! in the workbench repo above — is not fenced off by any rule; it simply sits
//! outside the granted subtree. The MCP server, a separate process unbound by
//! the model's tool rules, is the only channel to graph state.
//!
//! Provenance does not depend on agent discipline. Output provenance is enforced
//! by git: `complete` refuses to record a result while undeclared writes are
//! dirty, so every produced file is declared. Input provenance is *derived*:
//! after the session, the driver mines the recorded transcript for the files the
//! agent was observed reading and pins the undeclared ones as context
//! ([`ops::amend_context`]), marked `observed`. What can't be pinned (web
//! fetches) still sits verbatim in the log.
//!
//! Every session's interaction stream is *streamed* to its durable attempt's
//! `work.jsonl` as it happens, one flushed line per event, so an abrupt exit (Ctrl-C, crash,
//! kill) loses at most an unflushed tail. The log dirties only the workbench
//! repository; store mutations sweep the story-so-far into their commits, and
//! attempt finalization commits the remaining tail.

#[path = "work/backend.rs"]
mod backend;
#[path = "work/claude.rs"]
mod claude;
#[path = "work/codex.rs"]
mod codex;

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use serde_json::{json, Value};

use backend::{Backend, McpServer, Session};
use claude::ClaudeCode;
use codex::OpenAiCodex;

use llaundry::{ops, title_of, Author, Config, GitVcs, NodeMeta, Store, WorkedBy};

#[derive(Parser)]
#[command(
    name = "llaundry-work",
    version,
    about = "Run an LLM against a llaundry node"
)]
struct Cli {
    /// Id of the node to work on. Omit with --next to pick one automatically.
    #[arg(required_unless_present = "next", conflicts_with = "next")]
    node: Option<String>,
    /// Work the first ready node instead of naming one: the first id (in
    /// sorted order) that is ready and not assigned to a human.
    #[arg(long)]
    next: bool,
    /// Path to the store directory.
    #[arg(long, env = "LLAUNDRY_DIR", default_value = ".llaundry")]
    store: std::path::PathBuf,
    /// Which LLM backend to run the work with.
    /// Config `work.backend`; built-in default `openai-codex`.
    #[arg(long, value_enum)]
    backend: Option<BackendKind>,
    /// The Claude Code executable.
    /// Config `work.claude-code.bin`; built-in default `claude`.
    #[arg(long)]
    claude_bin: Option<String>,
    /// The OpenAI Codex executable.
    /// Config `work.openai-codex.bin`; built-in default `codex`.
    #[arg(long)]
    codex_bin: Option<String>,
    /// Path to the `llaundry-mcp` executable the model is allowed to use.
    /// Config `work.mcp-bin`; built-in default `llaundry-mcp`.
    #[arg(long)]
    mcp_bin: Option<String>,
    /// Model to request from the backend.
    /// Config for the selected backend; backend default if unset.
    #[arg(long)]
    model: Option<String>,
    /// Allow backend network access. For Claude this grants WebFetch and
    /// WebSearch; for Codex it enables network access in the workspace sandbox.
    /// Web reads cannot be pinned as context; they are only visible in the log.
    #[arg(long)]
    network: bool,
    /// Print the backend invocation instead of running it.
    #[arg(long)]
    dry_run: bool,
    /// Work the node even if its dependencies are unsatisfied.
    #[arg(long)]
    force: bool,
    /// Project revision from which to create the isolated execution worktree.
    #[arg(long)]
    base: Option<String>,
    /// Retain the execution worktree even after successful clean completion.
    #[arg(long)]
    keep_worktree: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum BackendKind {
    ClaudeCode,
    OpenaiCodex,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let store = Store::open(cli.store.clone())?;
    let vcs = GitVcs::for_store(&store);
    let node = match &cli.node {
        Some(id) => id.clone(),
        None => match ops::first_ready_for(&store, &vcs, Author::Machine)? {
            Some(id) => id,
            None => bail!("no node is ready to work"),
        },
    };
    let (meta, description) = store.read_node(&node)?;

    // The store's optional defaults. Precedence for every setting it covers:
    // an explicit `--flag` wins, else `config.toml`, else the built-in default.
    let config = Config::load(&cli.store)?;
    let backend_kind = match cli.backend {
        Some(b) => b,
        None => match config.work.backend.as_deref() {
            Some(name) => match BackendKind::from_str(name, true) {
                Ok(b) => b,
                Err(e) => bail!("config work.backend: {e}"),
            },
            None => BackendKind::OpenaiCodex,
        },
    };
    let mcp_bin = cli
        .mcp_bin
        .clone()
        .or_else(|| config.work.mcp_bin.clone())
        .unwrap_or_else(|| "llaundry-mcp".into());

    ops::authorize_execution_start(&store, &vcs, &node, Author::Machine, cli.force)?;

    // Open with no result and a recorded log means a paused unit of work (it
    // stopped mid-unit, e.g. on a question node): replay the log so the new
    // session continues where the last one left off. A node with a result is
    // being *re*worked — its old story doesn't continue, it starts over.
    let previous_log = if store.read_result(&node)?.is_none() {
        store.read_work_log(&node)?
    } else {
        None
    };

    // The session is anchored to the workbench, not to wherever this driver was
    // launched: the backend runs in the project directory (so every `./**`
    // grant resolves there), and the MCP server gets the store — one level
    // above, outside the granted subtree — by absolute path.
    let workbench_abs = store
        .workbench_root()
        .canonicalize()
        .with_context(|| format!("resolving workbench of {}", cli.store.display()))?;
    let store_abs = workbench_abs.join(store.store_name());

    let workspace = ops::prepare_execution(
        &store, &vcs, &node, Author::Machine, cli.force, cli.base.as_deref(), !cli.dry_run,
    )?;
    let run_id = workspace.identity.attempt_id.clone();
    let candidate_branch = workspace.identity.candidate_branch.clone();
    let worktree_path = workspace.path.clone();
    let mcp_args = vec![
        "--store".into(),
        store_abs.to_string_lossy().into_owned(),
        "--project".into(),
        worktree_path.to_string_lossy().into_owned(),
        "--attempt".into(),
        run_id.clone(),
    ];

    let mut session = Session {
        node_id: node.clone(),
        prompt: {
            let mut prompt = build_prompt(&node, &meta, &description, previous_log.as_deref());
            if let Some(feedback) = &workspace.rejected_feedback {
                prompt.push_str("\n\nThe previous candidate was rejected by human review. Address this feedback in the new attempt:\n\n");
                prompt.push_str(feedback);
            }
            prompt
        },
        project_root: worktree_path.clone(),
        mcp: McpServer {
            name: "llaundry".into(),
            command: mcp_bin,
            args: mcp_args,
        },
    };

    let backend: Box<dyn Backend> = match backend_kind {
        BackendKind::ClaudeCode => {
            let cc = &config.work.claude_code;
            Box::new(ClaudeCode::new(
                cli
                    .claude_bin
                    .clone()
                    .or_else(|| cc.bin.clone())
                    .unwrap_or_else(|| "claude".into()),
                cli.model.clone().or_else(|| cc.model.clone()),
                cli.network,
            ))
        }
        BackendKind::OpenaiCodex => {
            let codex = &config.work.openai_codex;
            Box::new(OpenAiCodex::new(
                cli.codex_bin
                    .clone()
                    .or_else(|| codex.bin.clone())
                    .unwrap_or_else(|| "codex".into()),
                cli.model.clone().or_else(|| codex.model.clone()),
                cli.network,
            ))
        }
    };

    if cli.dry_run {
        println!("input commit: {}", workspace.input_commit);
        println!("input tree: {}", workspace.input_tree);
        println!("worktree: {}", worktree_path.display());
        println!("candidate branch: {candidate_branch}");
        println!("{}", backend.describe(&session));
        return Ok(());
    }

    session.project_root = workspace.path.clone();

    eprintln!(
        "llaundry-work: {} working {} — {}{}",
        backend.name(),
        session.node_id,
        title_of(&description),
        if previous_log.is_some() {
            " (continuing)"
        } else {
            ""
        }
    );

    // The log is streamed during the session (the one dirty path the clean-tree
    // rule tolerates), so an abrupt exit loses at most an unflushed tail.
    // The attempt header supplements the already-durable attempt metadata with
    // the backend launch facts and begins this attempt's transcript.
    use std::io::Write;
    let started = now_millis();
    let mut log = store.open_attempt_log(&run_id, false)?;
    writeln!(
        log,
        "{}",
        json!({
            "event": "attempt",
            "at": started,
            "backend": backend.name(),
            "model": backend.model(),
            "definition_version": store.node_version(&node)?,
            "run_id": run_id,
            "candidate_branch": workspace.identity.candidate_branch,
            "input_commit": workspace.input_commit,
            "input_tree": workspace.input_tree,
        })
    )?;
    log.flush()?;

    let success = backend.run(&session, &mut log)?;
    drop(log);

    // Stamp which engine did the work onto the result — if this session wrote
    // one (`since` keeps a dead rework session from relabelling the old
    // result, and a paused session has no result to stamp yet).
    let worked_by = WorkedBy {
        backend: backend.name().to_string(),
        model: backend.model().map(str::to_string),
    };
    let reads = store
        .read_attempt_log(&run_id)?
        .map(|log| observed_reads(&log, &store, Some(&workspace.path)))
        .unwrap_or_default();
    let review = ops::finalize_execution_attempt(
        &store,
        &vcs,
        &run_id,
        worked_by,
        started,
        &reads,
        success,
    )?;

    if !success {
        eprintln!("llaundry-work: retained worktree {}", workspace.path.display());
        bail!("backend session exited unsuccessfully");
    }

    if let Some(review) = review {
        eprintln!("llaundry-work: awaiting human review in node {review}");
    }

    if !ops::finish_attempt_workspace(&store, &vcs, &run_id, cli.keep_worktree)? {
        eprintln!(
            "llaundry-work: retained candidate worktree {} on {}",
            workspace.path.display(), workspace.identity.candidate_branch
        );
    }
    Ok(())
}

/// The node `--next` picks: the first id (in [`Store::list_ids`] order, i.e.
/// sorted) that is ready and not assigned to a human — the same pool
/// `llaundry ready --for llm` shows. A human-assigned node is waiting for a
/// human's answer, never an LLM's to pick up.
/// Project files the session was observed reading, mined from the recorded
/// transcript: the `file_path` of every built-in `Read` tool call, relativised
/// to the project root. Reads outside the project tree cannot happen (the
/// tools are scoped to it) but are dropped defensively should a transcript
/// claim one, as are `.git` reads — history is browsable context, not
/// pinnable file content. Everything stays visible in the raw log either way.
/// Deduplicated, transcript order. Unparseable lines are skipped: the log may
/// hold non-transcript events (the attempt header) and half-written tails.
fn observed_reads(log: &str, store: &Store, execution_root: Option<&std::path::Path>) -> Vec<String> {
    let root = execution_root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| store.project_root());
    let root = root.canonicalize().unwrap_or(root);
    let store_name = store.store_name();
    let mut seen = std::collections::HashSet::new();
    let mut reads = Vec::new();
    for line in log.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let blocks = event
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use")
                || block.get("name").and_then(Value::as_str) != Some("Read")
            {
                continue;
            }
            let Some(path) = block.pointer("/input/file_path").and_then(Value::as_str) else {
                continue;
            };
            let path = std::path::Path::new(path);
            let rel = if path.is_absolute() {
                match path.strip_prefix(&root) {
                    Ok(rel) => rel,
                    Err(_) => continue,
                }
            } else {
                path
            };
            let inside = rel.components().all(|c| {
                matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
            let first = rel.components().find_map(|c| match c {
                std::path::Component::Normal(seg) => Some(seg.to_string_lossy()),
                _ => None,
            });
            match first {
                Some(first) if inside && first != store_name && first != ".git" => {}
                _ => continue,
            }
            let rel = rel.to_string_lossy().into_owned();
            if seen.insert(rel.clone()) {
                reads.push(rel);
            }
        }
    }
    reads
}

/// The instruction handed to the model: what the node is, and the discipline it
/// must follow — graph changes through the `llaundry` MCP tools, files through
/// the built-in file tools, no shell. A session finishes with `complete_node`
/// (outputs and context declared, notes as the record of what happened) or
/// `fail_node` — or pauses on a human-assigned question node.
///
/// On legacy continuation, `previous_log` is replayed
/// verbatim so the session picks up exactly where the last one stopped — the
/// handoff is mechanical, not dependent on the previous agent having left notes.
fn build_prompt(
    id: &str,
    meta: &NodeMeta,
    description: &str,
    previous_log: Option<&str>,
) -> String {
    let mut p = vec![
        "You are an autonomous worker on a llaundry node graph.".to_string(),
        "Every graph change goes through the `llaundry` MCP tools. For real work you".to_string(),
        "also have the built-in file tools (Read, Glob, Grep, Edit, Write) — but no".to_string(),
        "shell. The file tools are scoped to your working directory, the project;".to_string(),
        "the graph lives outside it and is reachable only through the MCP tools.".to_string(),
        "Your session is recorded verbatim as the node's work log.".to_string(),
        String::new(),
        format!("You are assigned to node `{id}`:"),
    ];
    for dep in &meta.depends_on {
        p.push(format!("  depends_on -> {dep}"));
    }
    for src in &meta.derived_from {
        p.push(format!("  derived_from -> {src}"));
    }
    let description = description.trim();
    if !description.is_empty() {
        p.push(String::new());
        p.push("Node description:".into());
        p.push(description.to_string());
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
    p.push("  2. Carry out the node's work: produce files with the file tools, and".into());
    p.push("     update the graph with add_node, link_nodes, edit_node — keeping each".into());
    p.push("     change small and clearly described.".into());
    p.push(format!(
        "  3. When the work is done, record it: complete_node {id}, declaring every"
    ));
    p.push("     file you wrote in `outputs`, files you consumed in `context` (other".into());
    p.push("     nodes' outputs need no pin), and `notes` summarising what you did".into());
    p.push("     and why. Completion is refused while undeclared writes are dirty.".into());
    p.push("     If the work cannot be done,".into());
    p.push(format!(
        "     record that instead: fail_node {id} with `notes` explaining why."
    ));
    p.push("  4. If you need a decision or information only a human can give, do NOT".into());
    p.push("     complete or fail. Instead add_node a question (description starting".into());
    p.push("     `Question: ...`, assignee `human`, with the context and options),".into());
    p.push(format!(
        "     link_nodes {id} depends_on it, and stop. Work resumes here once it is answered."
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

#[cfg(test)]
mod tests {
    use super::*;
    use llaundry::Vcs;
    use llaundry::Author;

    /// Minimal in-memory [`Vcs`] — the library's `FakeVcs` is `cfg(test)` there,
    /// invisible to this crate's tests. Ready-node queries only read, so no-ops do.
    struct NullVcs;
    impl Vcs for NullVcs {
        fn capture(&self, _paths: &[String], _message: &str) -> Result<String> {
            Ok("id".into())
        }
        fn head_commit(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn current_branch(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn resolve_revision(&self, rev: &str) -> Result<(String, String)> {
            Ok((rev.into(), format!("tree-{rev}")))
        }
        fn tree_id(&self, commit: &str) -> Result<String> {
            Ok(format!("tree-{commit}"))
        }
        fn retain_output(&self, _node: &str, _commit: &str) -> Result<()> {
            Ok(())
        }
        fn file_blob(&self, _path: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn commit_store(&self, _path: &str, _message: &str) -> Result<()> {
            Ok(())
        }
        fn drift(&self, _id: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn files_in(&self, _id: &str) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn dirty_paths(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn root_commit(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn commit_exists(&self, _hash: &str) -> Result<bool> {
            Ok(true)
        }
        fn remote_url(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn ref_commit(&self, _reference: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn publish_fast_forward(&self, _target: &str, _old: &str, _new: &str) -> Result<bool> {
            Ok(false)
        }
        fn create_worktree(&self, _path: &std::path::Path, _branch: &str, _rev: &str) -> Result<()> {
            Ok(())
        }
        fn worktree_clean(&self, _path: &std::path::Path) -> Result<bool> {
            Ok(true)
        }
        fn remove_worktree(&self, _path: &std::path::Path) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn first_ready_skips_human_assigned_and_blocked_nodes() {
        let dir = std::env::temp_dir().join(format!("llaundry-next-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::init(dir.join(".llaundry")).unwrap();
        let vcs = NullVcs;

        assert_eq!(ops::first_ready_for(&store, &vcs, Author::Machine).unwrap(), None, "empty store");

        let node = |description: &str, assignee, depends_on| {
            ops::add(
                &store,
                &vcs,
                ops::NewNode {
                    description: description.into(),
                    author: Author::Human,
                    assignee,
                    depends_on,
                    derived_from: vec![],
                },
            )
            .unwrap()
        };
        let question = node("Question: which way?", Some(Author::Human), vec![]);
        assert_eq!(
            ops::first_ready_for(&store, &vcs, Author::Machine).unwrap(),
            None,
            "a human-assigned node is not an LLM's to pick up"
        );

        let a = node("do a thing", None, vec![]);
        let _blocked = node("after a", None, vec![a.clone()]);
        let _also_blocked = node("after the question", None, vec![question]);
        assert_eq!(ops::first_ready_for(&store, &vcs, Author::Machine).unwrap(), Some(a));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn observed_reads_mines_project_read_calls_from_the_transcript() {
        let dir = std::env::temp_dir().join(format!("llaundry-work-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::init(dir.join(".llaundry")).unwrap();
        let root = store.project_root().canonicalize().unwrap();
        let workbench = dir.canonicalize().unwrap();

        let read = |path: String| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"Read","input":{{"file_path":"{path}"}}}}]}}}}"#
            )
        };
        let log = [
            r#"{"event":"attempt","at":1}"#.to_string(),
            read(format!("{}/README.md", root.display())),
            read("/somewhere/else.txt".into()),
            // The store lives above the project root: an outside-the-tree path.
            read(format!("{}/.llaundry/nodes/n/node.toml", workbench.display())),
            read(format!("{}/.git/config", root.display())),
            read("src/lib.rs".into()), // relative paths count too
            read(format!("{}/README.md", root.display())), // duplicate
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__llaundry__show_node","input":{"id":"n"}}]}}"#.to_string(),
            "not json (a half-written tail)".to_string(),
        ]
        .join("\n");

        assert_eq!(observed_reads(&log, &store, None), ["README.md", "src/lib.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompt_states_the_node_and_the_tools_only_rule() {
        let meta = NodeMeta {
            schema: 1,
            author: Author::Human,
            assignee: None,
            depends_on: vec!["node-0".into()],
            derived_from: vec![],
            review: None,
        };
        let prompt = build_prompt("node-1", &meta, "  Write the config parser.  ", None);

        assert!(prompt.contains("node-1"));
        assert!(prompt.contains("llaundry` MCP tools"));
        // The scoping is stated up front: the graph is outside the file tools'
        // world, so a denied path reads as policy, not a malfunction.
        assert!(prompt.contains("reachable only through the MCP tools"));
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
            author: Author::Human,
            assignee: None,
            depends_on: vec![],
            derived_from: vec![],
            review: None,
        };
        let log = "{\"event\":\"attempt\"}\n{\"tool\":\"add_node\"}\n";
        let prompt = build_prompt("node-1", &meta, "", Some(log));

        assert!(prompt.contains("earlier session"));
        assert!(prompt.contains("Continue from where it"));
        // The log is included verbatim (sans trailing newline).
        assert!(prompt.contains("{\"event\":\"attempt\"}\n{\"tool\":\"add_node\"}"));
    }

}
