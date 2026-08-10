//! Provider-owned presentation of decoded events.
//!
//! Decoding preserves what the provider reported. Presenters may interpret
//! that provider-specific representation for the human-facing pretty view;
//! raw mode always falls back to the stable event vocabulary unchanged.

use crate::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};
use serde_json::Value;

trait EventPresenter: Sync {
    fn pretty_summary(&self, _event: &AgentEvent) -> Option<String> {
        None
    }

    fn pretty_detail(&self, _event: &AgentEvent) -> Option<Vec<DetailBlock>> {
        None
    }
}

struct CodexPresenter;
struct ClaudePresenter;

static CODEX: CodexPresenter = CodexPresenter;
static CLAUDE: ClaudePresenter = ClaudePresenter;

impl Protocol {
    fn presenter(self) -> &'static dyn EventPresenter {
        match self {
            Protocol::CodexJsonl | Protocol::CodexAppServer => &CODEX,
            Protocol::ClaudeJsonl => &CLAUDE,
        }
    }

    /// A collapsed-line summary under this provider's presentation rules.
    pub fn presented_summary(self, event: &AgentEvent, mode: PresentationMode) -> String {
        if mode == PresentationMode::Pretty {
            if let Some(summary) = self.presenter().pretty_summary(event) {
                return crate::event::truncate_summary(&summary);
            }
        }
        event.summary()
    }

    /// Structured detail under this provider's presentation rules.
    pub fn presented_detail(self, event: &AgentEvent, mode: PresentationMode) -> Vec<DetailBlock> {
        if mode == PresentationMode::Pretty {
            if let Some(detail) = self.presenter().pretty_detail(event) {
                return detail;
            }
        }
        event.presented_detail_default(mode)
    }
}

impl EventPresenter for ClaudePresenter {
    fn pretty_summary(&self, event: &AgentEvent) -> Option<String> {
        let (name, input) = claude_tool_input(event)?;
        claude_tool_request(&name, &input).map(|request| request.summary)
    }

    fn pretty_detail(&self, event: &AgentEvent) -> Option<Vec<DetailBlock>> {
        let (name, input) = claude_tool_input(event)?;
        let request = claude_tool_request(&name, &input)?;
        Some(request_detail(request, event))
    }
}

impl EventPresenter for CodexPresenter {
    fn pretty_summary(&self, event: &AgentEvent) -> Option<String> {
        codex_bash_command(event)
    }

    fn pretty_detail(&self, event: &AgentEvent) -> Option<Vec<DetailBlock>> {
        command_detail(codex_bash_command(event)?, event)
    }
}

/// One tool call rendered for humans: what it asked for, and the language its
/// output should be highlighted as (if any).
struct ToolRequest {
    summary: String,
    language: Option<String>,
}

/// The decoded `input` object Claude sent with a tool call, alongside the tool
/// name. Decoding stores that object verbatim as the event's `detail`.
fn claude_tool_input(event: &AgentEvent) -> Option<(String, Value)> {
    let (name, detail) = match event {
        AgentEvent::ToolStarted { name, detail, .. }
        | AgentEvent::ToolCompleted { name, detail, .. } => (name, detail),
        _ => return None,
    };
    Some((name.clone(), serde_json::from_str(detail).ok()?))
}

/// Interpret a Claude tool call into its request line. Unknown tools return
/// `None` so they fall back to the stable event vocabulary — better a raw
/// object than a confidently wrong paraphrase.
fn claude_tool_request(name: &str, input: &Value) -> Option<ToolRequest> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    let (summary, language) = match name {
        "Bash" | "BashOutput" | "KillShell" => (
            text("command")
                .or_else(|| text("bash_id"))
                .or_else(|| text("shell_id"))?,
            Some("bash".to_owned()),
        ),
        "Read" | "NotebookEdit" => (read_summary(input)?, None),
        "Glob" | "Grep" => {
            let pattern = text("pattern")?;
            match text("path") {
                Some(path) => (format!("{pattern} in {path}"), None),
                None => (pattern, None),
            }
        }
        "WebFetch" => (text("url")?, None),
        "WebSearch" => (text("query")?, None),
        "Task" | "Agent" => (text("description").or_else(|| text("prompt"))?, None),
        "Skill" => (
            match (text("skill")?, text("args")) {
                (skill, Some(args)) if !args.is_empty() => format!("{skill} {args}"),
                (skill, _) => skill,
            },
            None,
        ),
        "TodoWrite" => (todo_summary(input)?, None),
        _ => return None,
    };
    Some(ToolRequest { summary, language })
}

/// `Read`'s range arguments are optional and independent: either bound alone is
/// still worth showing, so the line reads `path:first-last`, `path:first-`, or
/// just `path`.
fn read_summary(input: &Value) -> Option<String> {
    let path = input
        .get("file_path")
        .or_else(|| input.get("notebook_path"))
        .or_else(|| input.get("path"))?
        .as_str()?;
    let number = |key: &str| input.get(key).and_then(Value::as_u64);
    Some(match (number("offset"), number("limit")) {
        (Some(offset), Some(limit)) => format!("{path}:{offset}-{}", offset + limit),
        (Some(offset), None) => format!("{path}:{offset}-"),
        (None, Some(limit)) => format!("{path} (first {limit} lines)"),
        (None, None) => path.to_owned(),
    })
}

/// Collapse a todo list to its first in-progress item, which is the only part
/// of a `TodoWrite` that says anything about what the agent is doing now.
fn todo_summary(input: &Value) -> Option<String> {
    let todos = input.get("todos")?.as_array()?;
    let active = todos
        .iter()
        .find(|todo| todo.get("status").and_then(Value::as_str) == Some("in_progress"))
        .or_else(|| todos.first())?;
    let text = active
        .get("activeForm")
        .or_else(|| active.get("content"))?
        .as_str()?;
    Some(format!("{text} ({} todos)", todos.len()))
}

fn request_detail(request: ToolRequest, event: &AgentEvent) -> Vec<DetailBlock> {
    let mut blocks = vec![DetailBlock::Code {
        language: request.language,
        text: request.summary,
    }];
    let output = match event {
        AgentEvent::ToolCompleted { output, .. } => output,
        _ => return blocks,
    };
    if !output.is_empty() {
        blocks.push(DetailBlock::Code {
            language: None,
            text: output.clone(),
        });
    }
    blocks
}

fn codex_bash_command(event: &AgentEvent) -> Option<String> {
    let command = match event {
        AgentEvent::CommandStarted { command } | AgentEvent::CommandCompleted { command, .. } => {
            command
        }
        _ => return None,
    };
    let (executable, rest) = command.split_once(char::is_whitespace)?;
    let shell = executable.rsplit('/').next()?;
    if !matches!(shell, "bash" | "sh" | "zsh") {
        return None;
    }
    let (flags, command) = rest.trim_start().split_once(char::is_whitespace)?;
    if !(flags == "-c" || (flags.starts_with('-') && flags.contains('c'))) {
        return None;
    }
    parse_shell_word(command.trim_start())
}

/// Parse the single shell word used as the argument to `bash -c`. Codex quotes
/// that argument using ordinary POSIX single/double quotes; interpreting just
/// one word keeps this presentation code small and never executes the text.
fn parse_shell_word(input: &str) -> Option<String> {
    #[derive(Clone, Copy)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut escaped = false;
    let mut output = String::new();
    let mut chars = input.char_indices();
    while let Some((index, ch)) = chars.next() {
        if escaped {
            output.push(ch);
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Quote::None, '\\') | (Quote::Double, '\\') => escaped = true,
            (Quote::None, '\'') => quote = Quote::Single,
            (Quote::None, '"') => quote = Quote::Double,
            (Quote::Single, '\'') | (Quote::Double, '"') => quote = Quote::None,
            (Quote::None, ch) if ch.is_whitespace() => {
                return input[index..].trim().is_empty().then_some(output);
            }
            _ => output.push(ch),
        }
    }
    if escaped || !matches!(quote, Quote::None) {
        None
    } else {
        Some(output)
    }
}

fn command_detail(command: String, event: &AgentEvent) -> Option<Vec<DetailBlock>> {
    let mut blocks = vec![DetailBlock::Code {
        language: Some("bash".into()),
        text: command,
    }];
    let output = match event {
        AgentEvent::ToolCompleted { output, .. } | AgentEvent::CommandCompleted { output, .. } => {
            output
        }
        _ => return Some(blocks),
    };
    if !output.is_empty() {
        blocks.push(DetailBlock::Code {
            language: None,
            text: output.clone(),
        });
    }
    Some(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_tool(name: &str, input: &str) -> AgentEvent {
        AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: name.into(),
            detail: input.into(),
            status: "completed".into(),
            output: "line\n".into(),
        }
    }

    fn claude_summary(event: &AgentEvent) -> String {
        Protocol::ClaudeJsonl.presented_summary(event, PresentationMode::Pretty)
    }

    #[test]
    fn claude_reads_show_the_path_and_its_line_range() {
        let event = claude_tool(
            "Read",
            r#"{"file_path":"/w/lua/tree.lua","limit":120,"offset":255}"#,
        );
        assert_eq!(claude_summary(&event), "/w/lua/tree.lua:255-375");
        assert_eq!(
            Protocol::ClaudeJsonl.presented_detail(&event, PresentationMode::Pretty),
            vec![
                DetailBlock::Code {
                    language: None,
                    text: "/w/lua/tree.lua:255-375".into(),
                },
                DetailBlock::Code {
                    language: None,
                    text: "line\n".into(),
                },
            ]
        );
    }

    #[test]
    fn claude_reads_without_a_range_are_just_the_path() {
        let event = claude_tool("Read", r#"{"file_path":"/w/README.md"}"#);
        assert_eq!(claude_summary(&event), "/w/README.md");
    }

    #[test]
    fn claude_searches_read_as_pattern_and_scope() {
        assert_eq!(
            claude_summary(&claude_tool(
                "Grep",
                r#"{"pattern":"fn main","path":"src","output_mode":"content"}"#
            )),
            "fn main in src"
        );
        assert_eq!(
            claude_summary(&claude_tool("Glob", r#"{"pattern":"**/*.rs"}"#)),
            "**/*.rs"
        );
    }

    #[test]
    fn claude_bash_still_wins_over_the_generic_object() {
        let event = claude_tool(
            "Bash",
            r#"{"command":"cargo test","run_in_background":true}"#,
        );
        assert_eq!(claude_summary(&event), "cargo test");
        assert!(matches!(
            Protocol::ClaudeJsonl
                .presented_detail(&event, PresentationMode::Pretty)
                .first(),
            Some(DetailBlock::Code { language: Some(language), .. }) if language == "bash"
        ));
    }

    #[test]
    fn claude_leaves_unknown_tools_to_the_stable_vocabulary() {
        let event = claude_tool("mcp__thing__do", r#"{"weird":1}"#);
        assert_eq!(
            claude_summary(&event),
            r#"mcp__thing__do: {"weird":1} (completed)"#
        );
    }

    #[test]
    fn claude_raw_mode_keeps_the_request_object() {
        let event = claude_tool("Read", r#"{"file_path":"/w/README.md"}"#);
        assert!(matches!(
            Protocol::ClaudeJsonl
                .presented_detail(&event, PresentationMode::Raw)
                .first(),
            Some(DetailBlock::Text(text)) if text.contains(r#"{"file_path":"/w/README.md"}"#)
        ));
    }

    #[test]
    fn codex_unwraps_a_bash_login_command_but_keeps_output() {
        let event = AgentEvent::CommandCompleted {
            command: "/usr/bin/bash -lc 'cargo test --all'".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "ok\n".into(),
        };
        assert_eq!(
            Protocol::CodexAppServer.presented_summary(&event, PresentationMode::Pretty),
            "cargo test --all"
        );
        assert_eq!(
            Protocol::CodexAppServer.presented_detail(&event, PresentationMode::Pretty),
            vec![
                DetailBlock::Code {
                    language: Some("bash".into()),
                    text: "cargo test --all".into(),
                },
                DetailBlock::Code {
                    language: None,
                    text: "ok\n".into(),
                },
            ]
        );
        assert!(matches!(
            Protocol::CodexAppServer
                .presented_detail(&event, PresentationMode::Raw)
                .first(),
            Some(DetailBlock::Text(text)) if text.contains("/usr/bin/bash -lc")
        ));
    }

    #[test]
    fn codex_leaves_non_shell_commands_alone() {
        let event = AgentEvent::CommandStarted {
            command: "cargo test".into(),
        };
        assert_eq!(
            Protocol::CodexJsonl.presented_summary(&event, PresentationMode::Pretty),
            "cargo test (running)"
        );
    }

    #[test]
    fn codex_unwraps_shell_escaped_quotes() {
        let event = AgentEvent::CommandStarted {
            command: r#"/bin/bash -lc 'printf '"'"'%s\n'"'"' hello'"#.into(),
        };
        assert_eq!(
            Protocol::CodexJsonl.presented_summary(&event, PresentationMode::Pretty),
            "printf '%s\\n' hello"
        );
    }
}
