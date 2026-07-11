mod claude;
mod codex;

pub use claude::ClaudeCode;
pub use codex::OpenAiCodex;

use anyhow::Result;
use std::path::PathBuf;

/// One unit of work: an LLM session focused on a single node, together with the MCP
/// server the model is allowed to use. Deliberately free of any store handle — once
/// the prompt is built, a backend needs nothing else from the database.
pub struct Session {
    pub node_id: String,
    pub prompt: String,
    /// Absolute project root. The backend runs here, so project-scoped tool
    /// rules and relative paths resolve against the execution worktree.
    pub project_root: PathBuf,
    pub mcp: McpServer,
}

/// A stdio MCP server the backend should expose to the model.
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// What a run produced: whether the backend exited cleanly, and the model its
/// stream named as actually doing the work. `None` means the stream never
/// named one — the driver then falls back to the pinned request, so a result
/// is never stamped with less than what was asked for.
pub struct RunOutcome {
    pub success: bool,
    pub model: Option<String>,
}

/// The LLM seam. An implementation runs a [`Session`] to completion, streaming
/// each JSONL transcript line to `log` as it arrives (flushed per line, so the
/// story survives an abrupt exit), and returns a [`RunOutcome`]: whether the
/// backend exited cleanly, and the model observed in its stream. `describe`
/// renders the invocation for `--dry-run` without running anything.
pub trait Backend {
    fn name(&self) -> &str;
    /// The model this backend will request, if pinned; `None` means the
    /// backend's own default. Recorded in the attempt header as the request;
    /// what actually ran is reported back via [`RunOutcome::model`].
    fn model(&self) -> Option<&str>;
    fn run(&self, session: &Session, log: &mut dyn std::io::Write) -> Result<RunOutcome>;
    fn describe(&self, session: &Session) -> String;
}

/// The model a transcript event names, if any, looked up at the paths the
/// known backends use: Claude Code's init event (`model`) and assistant
/// messages (`message.model`), Codex's session events (`model`, `msg.model`).
/// Deliberately not a recursive search — a `model` key buried in tool
/// arguments is the agent's data, not the engine's self-report.
pub(crate) fn event_model(line: &str) -> Option<String> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    ["/model", "/message/model", "/msg/model"]
        .iter()
        .find_map(|path| Some(event.pointer(path)?.as_str()?.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_model_reads_the_known_self_report_paths() {
        // Claude Code: the init event and assistant messages.
        assert_eq!(
            event_model(r#"{"type":"system","subtype":"init","model":"claude-opus-4-1"}"#),
            Some("claude-opus-4-1".into())
        );
        assert_eq!(
            event_model(
                r#"{"type":"assistant","message":{"model":"claude-sonnet-4-5","content":[]}}"#
            ),
            Some("claude-sonnet-4-5".into())
        );
        // Codex: session configuration events.
        assert_eq!(
            event_model(r#"{"id":"0","msg":{"type":"session_configured","model":"gpt-5-codex"}}"#),
            Some("gpt-5-codex".into())
        );
    }

    #[test]
    fn event_model_ignores_agent_data_and_noise() {
        // A `model` key inside tool arguments is the agent's data, not a self-report.
        assert_eq!(
            event_model(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","input":{"model":"decoy"}}]}}"#
            ),
            None
        );
        assert_eq!(event_model(r#"{"model":42}"#), None, "non-string model");
        assert_eq!(event_model("not json"), None);
    }
}
