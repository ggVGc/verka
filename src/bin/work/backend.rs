use anyhow::Result;
use std::path::PathBuf;

/// One unit of work: an LLM session focused on a single node, together with the MCP
/// server the model is allowed to use. Deliberately free of any store handle — once
/// the prompt is built, a backend needs nothing else from the database.
pub(crate) struct Session {
    pub(crate) node_id: String,
    pub(crate) prompt: String,
    /// Absolute project root. The backend runs here, so project-scoped tool
    /// rules and relative paths resolve against the execution worktree.
    pub(crate) project_root: PathBuf,
    pub(crate) mcp: McpServer,
}

/// A stdio MCP server the backend should expose to the model.
pub(crate) struct McpServer {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
}

/// The LLM seam. An implementation runs a [`Session`] to completion, streaming
/// each JSONL transcript line to `log` as it arrives (flushed per line, so the
/// story survives an abrupt exit), and returns whether the backend exited
/// cleanly. `describe` renders the invocation for `--dry-run` without running
/// anything.
pub(crate) trait Backend {
    fn name(&self) -> &str;
    /// The model this backend will request, if pinned; `None` means the
    /// backend's own default. Recorded in the attempt header and stamped onto
    /// the result, so every unit of work names the engine that did it.
    fn model(&self) -> Option<&str>;
    fn run(&self, session: &Session, log: &mut dyn std::io::Write) -> Result<bool>;
    fn describe(&self, session: &Session) -> String;
}
