//! Agent event vocabulary, wire decoding, and presentation.
//!
//! The provider wire format stops here. Hosts consume only [`AgentEvent`] and
//! its rendered [`summary`](AgentEvent::summary) and
//! [`detail`](AgentEvent::detail); their process transport stays uninterpreted.
//!
//! Decoding is versioned by [`Protocol`]: a new wire format is a new
//! `Protocol` variant plus a decode arm, and the match is exhaustive, so a
//! missing decoder is a compile error rather than a silent mis-decode.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The wire protocol an agent speaks, and thus the decoder that reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    /// The one-shot `codex exec --json` item/thread/turn event schema.
    #[default]
    CodexJsonl,
    /// The bidirectional `codex app-server` JSON-RPC protocol (v2). Notification
    /// lines carry the events; requests and responses are control traffic.
    CodexAppServer,
    /// The Claude Code `stream-json` schema: a `system`/`assistant`/`user`/
    /// `result` newline-delimited JSON stream, as emitted by
    /// `claude --output-format stream-json`.
    ClaudeJsonl,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
}

/// The stable, provider-independent event vocabulary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A message the operator sent to the agent, recorded so their own turns
    /// appear inline in the same list. Host-originated, never decoded.
    UserMessage { text: String },
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted,
    TurnCompleted {
        usage: TokenUsage,
    },
    CommandStarted {
        command: String,
    },
    CommandCompleted {
        command: String,
        status: String,
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        output: String,
    },
    FileChanged {
        /// The provider's item id, used by hosts that correlate file changes
        /// with their own journals (e.g. Orka's checkpoint commits).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
        paths: Vec<String>,
        /// A host-attached checkpoint commit for this change, never decoded
        /// from the wire.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_error: Option<String>,
    },
    ToolStarted {
        name: String,
        detail: String,
    },
    ToolCompleted {
        name: String,
        status: String,
    },
    PlanUpdated {
        text: String,
    },
    AgentMessage {
        text: String,
    },
    Error {
        message: String,
    },
    /// A recognised envelope with no rendered view; carried, not shown as prose.
    Unknown {
        wire_type: String,
    },
    /// An undecodable line, kept visible as an error rather than dropped.
    Malformed {
        error: String,
    },
}

/// A structured, escape-free piece of a detail body. The renderer adds styling;
/// this never carries terminal control sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailBlock {
    Text(String),
    Code { language: Option<String>, text: String },
}

impl AgentEvent {
    /// The short tag shown at the head of the collapsed list line.
    pub fn tag(&self) -> &'static str {
        match self {
            AgentEvent::UserMessage { .. } => "user",
            AgentEvent::ThreadStarted { .. } => "thread",
            AgentEvent::TurnStarted => "turn",
            AgentEvent::TurnCompleted { .. } => "usage",
            AgentEvent::CommandStarted { .. } | AgentEvent::CommandCompleted { .. } => "command",
            AgentEvent::FileChanged { .. } => "files",
            AgentEvent::ToolStarted { .. } | AgentEvent::ToolCompleted { .. } => "tool",
            AgentEvent::PlanUpdated { .. } => "plan",
            AgentEvent::AgentMessage { .. } => "agent",
            AgentEvent::Error { .. } => "error",
            AgentEvent::Unknown { .. } => "unknown",
            AgentEvent::Malformed { .. } => "malformed",
        }
    }

    /// True for high-frequency lifecycle/bookkeeping events — thread and turn
    /// markers, token usage — that carry little signal turn over turn. The UI
    /// hides these by default so the list reads as the agent's actual work.
    pub fn is_minor(&self) -> bool {
        matches!(
            self,
            AgentEvent::ThreadStarted { .. } | AgentEvent::TurnStarted | AgentEvent::TurnCompleted { .. }
        )
    }

    /// A single collapsed-line summary. Never contains newlines.
    pub fn summary(&self) -> String {
        let line = match self {
            AgentEvent::UserMessage { text } => first_line(text),
            AgentEvent::ThreadStarted { thread_id } => format!("session {thread_id}"),
            AgentEvent::TurnStarted => "turn started".into(),
            AgentEvent::TurnCompleted { usage } => format!(
                "in {} · out {} · cached {}",
                usage.input_tokens, usage.output_tokens, usage.cached_input_tokens
            ),
            AgentEvent::CommandStarted { command } => first_line(command),
            AgentEvent::CommandCompleted {
                command,
                status,
                exit_code,
                ..
            } => match exit_code {
                Some(code) => format!("{} ({status}, exit {code})", first_line(command)),
                None => format!("{} ({status})", first_line(command)),
            },
            AgentEvent::FileChanged { paths, .. } => paths.join(", "),
            AgentEvent::ToolStarted { name, detail } if !detail.is_empty() => {
                format!("{name}: {}", first_line(detail))
            }
            AgentEvent::ToolStarted { name, .. } => name.clone(),
            AgentEvent::ToolCompleted { name, status } => format!("{name} ({status})"),
            AgentEvent::PlanUpdated { text } | AgentEvent::AgentMessage { text } => first_line(text),
            AgentEvent::Error { message } => first_line(message),
            AgentEvent::Unknown { wire_type } => wire_type.clone(),
            AgentEvent::Malformed { error } => first_line(error),
        };
        truncate_line(&line, 200)
    }

    /// The expandable detail body as escape-free structured blocks.
    pub fn detail(&self) -> Vec<DetailBlock> {
        match self {
            AgentEvent::UserMessage { text } => markdown_blocks(text),
            AgentEvent::ThreadStarted { thread_id } => {
                vec![DetailBlock::Text(format!("thread id: {thread_id}"))]
            }
            AgentEvent::TurnStarted => Vec::new(),
            AgentEvent::TurnCompleted { usage } => vec![DetailBlock::Text(format!(
                "input {} · cached input {} · output {} · reasoning {}",
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.output_tokens,
                usage.reasoning_output_tokens
            ))],
            AgentEvent::CommandStarted { command } => {
                vec![DetailBlock::Code {
                    language: None,
                    text: command.clone(),
                }]
            }
            AgentEvent::CommandCompleted {
                command,
                status,
                exit_code,
                output,
            } => {
                let mut blocks = vec![DetailBlock::Text(match exit_code {
                    Some(code) => format!("$ {command}\nstatus: {status} (exit {code})"),
                    None => format!("$ {command}\nstatus: {status}"),
                })];
                if !output.is_empty() {
                    blocks.push(DetailBlock::Code {
                        language: None,
                        text: output.clone(),
                    });
                }
                blocks
            }
            AgentEvent::FileChanged { paths, .. } => {
                vec![DetailBlock::Text(paths.join("\n"))]
            }
            AgentEvent::ToolStarted { name, detail } => {
                let mut text = name.clone();
                if !detail.is_empty() {
                    text.push('\n');
                    text.push_str(detail);
                }
                vec![DetailBlock::Text(text)]
            }
            AgentEvent::ToolCompleted { name, status } => {
                vec![DetailBlock::Text(format!("{name}: {status}"))]
            }
            AgentEvent::PlanUpdated { text } | AgentEvent::AgentMessage { text } => {
                markdown_blocks(text)
            }
            AgentEvent::Error { message } => vec![DetailBlock::Text(message.clone())],
            AgentEvent::Unknown { wire_type } => {
                vec![DetailBlock::Text(format!("unrecognised event: {wire_type}"))]
            }
            AgentEvent::Malformed { error } => vec![DetailBlock::Text(error.clone())],
        }
    }
}

/// Decode one wire line under the given protocol. Never fails: undecodable
/// input becomes [`AgentEvent::Malformed`] so nothing is silently lost.
pub fn decode_line(protocol: Protocol, line: &str) -> AgentEvent {
    match protocol {
        Protocol::CodexJsonl => decode_codex_line(line),
        Protocol::CodexAppServer => decode_appserver_line(line),
        Protocol::ClaudeJsonl => decode_claude_line(line),
    }
}

/// Decode one `codex app-server` line. Notifications (which carry a `method`)
/// map to events; requests and responses are control traffic and decode to
/// [`AgentEvent::Unknown`] so they are carried without cluttering the list.
fn decode_appserver_line(line: &str) -> AgentEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return AgentEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    match string(&value, "method") {
        Some(method) => {
            decode_appserver_notification(method, value.get("params").unwrap_or(&Value::Null))
        }
        None => AgentEvent::Unknown {
            wire_type: "response".into(),
        },
    }
}

fn decode_appserver_notification(method: &str, params: &Value) -> AgentEvent {
    match method {
        "thread/started" => AgentEvent::ThreadStarted {
            thread_id: clean_terminal_text(
                params
                    .get("thread")
                    .and_then(|thread| string(thread, "id"))
                    .unwrap_or_default(),
            ),
        },
        "turn/started" => AgentEvent::TurnStarted,
        // The turn's usage arrives here, just before `turn/completed` (which
        // carries none), so this is what marks a turn done and shows usage.
        "thread/tokenUsage/updated" => AgentEvent::TurnCompleted {
            usage: appserver_usage(params),
        },
        "item/started" => {
            decode_appserver_item(params.get("item").unwrap_or(&Value::Null), false)
        }
        "item/completed" => {
            decode_appserver_item(params.get("item").unwrap_or(&Value::Null), true)
        }
        "error" | "warning" | "guardianWarning" | "configWarning" => AgentEvent::Error {
            message: clean_terminal_text(
                string(params, "message").unwrap_or("agent reported an error"),
            ),
        },
        other => AgentEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

fn decode_appserver_item(item: &Value, completed: bool) -> AgentEvent {
    let kind = string(item, "type").unwrap_or("unknown");
    let clean = |value: &str| clean_terminal_text(value);
    match (kind, completed) {
        ("agentMessage", true) => AgentEvent::AgentMessage {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("commandExecution", false) => AgentEvent::CommandStarted {
            command: clean(string(item, "command").unwrap_or_default()),
        },
        ("commandExecution", true) => AgentEvent::CommandCompleted {
            command: clean(string(item, "command").unwrap_or_default()),
            status: clean(string(item, "status").unwrap_or("completed")),
            exit_code: item.get("exitCode").and_then(Value::as_i64),
            output: clean(string(item, "aggregatedOutput").unwrap_or_default()),
        },
        ("fileChange", true) => AgentEvent::FileChanged {
            id: clean(string(item, "id").unwrap_or_default()),
            paths: changed_paths(item),
            checkpoint: None,
            checkpoint_error: None,
        },
        ("plan", true) => AgentEvent::PlanUpdated {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("mcpToolCall", false) | ("webSearch", false) => AgentEvent::ToolStarted {
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
        },
        ("mcpToolCall", true) | ("webSearch", true) => AgentEvent::ToolCompleted {
            name: clean(tool_name(item, kind)),
            status: clean(string(item, "status").unwrap_or("completed")),
        },
        // userMessage (echoed back — the host shows its own), reasoning, deltas,
        // and item lifecycles with no view carry without rendering.
        _ => AgentEvent::Unknown {
            wire_type: format!("item:{kind}"),
        },
    }
}

fn appserver_usage(params: &Value) -> TokenUsage {
    let total = params
        .get("tokenUsage")
        .and_then(|usage| usage.get("total"))
        .unwrap_or(&Value::Null);
    let field = |key: &str| total.get(key).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input_tokens: field("inputTokens"),
        cached_input_tokens: field("cachedInputTokens"),
        output_tokens: field("outputTokens"),
        reasoning_output_tokens: field("reasoningOutputTokens"),
    }
}

fn decode_codex_line(line: &str) -> AgentEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return AgentEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    decode_codex_value(&value)
}

fn decode_codex_value(value: &Value) -> AgentEvent {
    let wire_type = string(value, "type").unwrap_or("unknown");
    match wire_type {
        "thread.started" => AgentEvent::ThreadStarted {
            thread_id: clean_terminal_text(string(value, "thread_id").unwrap_or_default()),
        },
        "turn.started" => AgentEvent::TurnStarted,
        "turn.completed" => AgentEvent::TurnCompleted {
            usage: value
                .get("usage")
                .and_then(|usage| serde_json::from_value(usage.clone()).ok())
                .unwrap_or_default(),
        },
        "turn.failed" | "error" => AgentEvent::Error {
            message: clean_terminal_text(error_message(value)),
        },
        "item.started" | "item.updated" | "item.completed" => {
            decode_codex_item(wire_type, value.get("item").unwrap_or(&Value::Null))
        }
        other => AgentEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

fn decode_codex_item(event_type: &str, item: &Value) -> AgentEvent {
    let kind = string(item, "type").unwrap_or("unknown");
    let completed = event_type == "item.completed";
    let clean = |value: &str| clean_terminal_text(value);
    match (kind, completed) {
        ("command_execution", false) => AgentEvent::CommandStarted {
            command: clean(string(item, "command").unwrap_or_default()),
        },
        ("command_execution", true) => AgentEvent::CommandCompleted {
            command: clean(string(item, "command").unwrap_or_default()),
            status: clean(string(item, "status").unwrap_or("completed")),
            exit_code: item.get("exit_code").and_then(Value::as_i64),
            output: clean(
                string(item, "aggregated_output")
                    .or_else(|| string(item, "output"))
                    .unwrap_or_default(),
            ),
        },
        ("file_change", true) => AgentEvent::FileChanged {
            id: clean(string(item, "id").unwrap_or_default()),
            paths: changed_paths(item),
            checkpoint: None,
            checkpoint_error: None,
        },
        ("agent_message", true) => AgentEvent::AgentMessage {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("plan", true) | ("plan_update", true) => AgentEvent::PlanUpdated {
            text: clean(
                string(item, "text")
                    .or_else(|| string(item, "plan"))
                    .unwrap_or_default(),
            ),
        },
        ("mcp_tool_call", false) | ("web_search", false) => AgentEvent::ToolStarted {
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
        },
        ("mcp_tool_call", true) | ("web_search", true) => AgentEvent::ToolCompleted {
            name: clean(tool_name(item, kind)),
            status: clean(string(item, "status").unwrap_or("completed")),
        },
        (_, _) => AgentEvent::Unknown {
            wire_type: format!("{event_type}:{kind}"),
        },
    }
}

/// Decode one Claude Code `stream-json` line. Its schema differs from codex's:
/// a top-level `system` (session/init metadata), `assistant` and `user`
/// messages carrying Anthropic content blocks, and a `result` turn summary.
///
/// One wire line maps to one [`AgentEvent`], as in the codex decoder. An
/// `assistant` message may carry several content blocks; the salient one is
/// chosen (a tool call over prose, prose over reasoning) and the rest remain in
/// the verbatim raw view. NOTE: the exact `stream-json` shape must be confirmed
/// against the installed `claude` version; it is isolated here so adapting to a
/// revised contract is a localized change.
fn decode_claude_line(line: &str) -> AgentEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return AgentEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    decode_claude_value(&value)
}

fn decode_claude_value(value: &Value) -> AgentEvent {
    let wire_type = string(value, "type").unwrap_or("unknown");
    match wire_type {
        "system" => {
            let subtype = string(value, "subtype").unwrap_or_default();
            if subtype == "init" {
                AgentEvent::ThreadStarted {
                    thread_id: clean_terminal_text(string(value, "session_id").unwrap_or_default()),
                }
            } else {
                AgentEvent::Unknown {
                    wire_type: clean_terminal_text(&format!("system:{subtype}")),
                }
            }
        }
        "assistant" => decode_claude_assistant(value.get("message").unwrap_or(&Value::Null)),
        "user" => decode_claude_user(value.get("message").unwrap_or(&Value::Null)),
        "result" => decode_claude_result(value),
        other => AgentEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

/// Choose the salient block of an assistant message: a tool call is the action
/// worth surfacing, then visible prose, then reasoning. The full message stays
/// available verbatim in the raw view.
fn decode_claude_assistant(message: &Value) -> AgentEvent {
    match message.get("content") {
        Some(Value::String(text)) => {
            return AgentEvent::AgentMessage {
                text: clean_terminal_text(text),
            }
        }
        Some(Value::Array(blocks)) => {
            let mut text = None;
            let mut thinking = None;
            for block in blocks {
                match string(block, "type") {
                    Some("tool_use") => return claude_tool_started(block),
                    Some("text") if text.is_none() => text = string(block, "text"),
                    Some("thinking") if thinking.is_none() => {
                        thinking = string(block, "thinking")
                    }
                    _ => {}
                }
            }
            if let Some(text) = text.or(thinking) {
                return AgentEvent::AgentMessage {
                    text: clean_terminal_text(text),
                };
            }
        }
        _ => {}
    }
    AgentEvent::Unknown { wire_type: "assistant".into() }
}

fn claude_tool_started(block: &Value) -> AgentEvent {
    let detail = block
        .get("input")
        .filter(|input| !input.is_null())
        .map(|input| input.to_string())
        .unwrap_or_default();
    AgentEvent::ToolStarted {
        name: clean_terminal_text(string(block, "name").unwrap_or("tool")),
        detail: clean_terminal_text(&detail),
    }
}

/// A Claude `user` message is a synthetic turn carrying tool results back to the
/// model; it is not an echo of the operator's input (the host records that itself).
fn decode_claude_user(message: &Value) -> AgentEvent {
    if let Some(Value::Array(blocks)) = message.get("content") {
        for block in blocks {
            if string(block, "type") == Some("tool_result") {
                let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                return AgentEvent::ToolCompleted {
                    name: clean_terminal_text(string(block, "tool_use_id").unwrap_or("tool")),
                    status: if is_error { "error".into() } else { "completed".into() },
                };
            }
        }
    }
    AgentEvent::Unknown { wire_type: "user".into() }
}

fn decode_claude_result(value: &Value) -> AgentEvent {
    let subtype = string(value, "subtype").unwrap_or_default();
    let is_error = value.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    if is_error || subtype.starts_with("error") {
        return AgentEvent::Error {
            message: clean_terminal_text(
                string(value, "result")
                    .or_else(|| string(value, "error"))
                    .unwrap_or("agent reported an error"),
            ),
        };
    }
    AgentEvent::TurnCompleted {
        usage: claude_usage(value.get("usage").unwrap_or(&Value::Null)),
    }
}

/// Map Claude's usage object onto [`TokenUsage`]. Claude reports cached
/// input as `cache_read_input_tokens`; the rest align by name.
fn claude_usage(usage: &Value) -> TokenUsage {
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input_tokens: count("input_tokens"),
        cached_input_tokens: count("cache_read_input_tokens"),
        output_tokens: count("output_tokens"),
        reasoning_output_tokens: 0,
    }
}

/// Collect the changed paths of a file-change item, tolerating the schema
/// variants seen across codex versions (`path` or `file_path` per change, or a
/// single path on the item itself).
fn changed_paths(item: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(changes) = item.get("changes").and_then(Value::as_array) {
        for change in changes {
            if let Some(path) = string(change, "path").or_else(|| string(change, "file_path")) {
                paths.push(clean_terminal_text(path));
            }
        }
    }
    if paths.is_empty() {
        if let Some(path) = string(item, "path").or_else(|| string(item, "file_path")) {
            paths.push(clean_terminal_text(path));
        }
    }
    paths
}

fn tool_name<'a>(item: &'a Value, kind: &'a str) -> &'a str {
    string(item, "tool")
        .or_else(|| string(item, "name"))
        .or_else(|| string(item, "server"))
        .unwrap_or(kind)
}

fn tool_detail(item: &Value) -> &str {
    string(item, "query")
        .or_else(|| string(item, "arguments"))
        .or_else(|| string(item, "detail"))
        .unwrap_or_default()
}

fn error_message(value: &Value) -> &str {
    value
        .get("error")
        .and_then(|error| error.as_str().or_else(|| string(error, "message")))
        .or_else(|| string(value, "message"))
        .unwrap_or("agent reported an error")
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_owned()
}

fn truncate_line(line: &str, max: usize) -> String {
    let flat: String = line.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() <= max {
        flat
    } else {
        let kept: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// Strip ANSI escape sequences and stray control characters, keeping newlines
/// and tabs. Provider text is presentation data, not a terminal to replay.
pub fn clean_terminal_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.peek() {
                // CSI: ESC [ ... final byte in 0x40..=0x7e
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... terminated by BEL or ST (ESC \)
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{07}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Any other escape: drop ESC and the single following byte.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\n' | '\t' => out.push(ch),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Split fenced Markdown into prose and code blocks, keeping the fence's
/// language for the renderer. Ported in spirit from Orka's work-log renderer.
pub fn markdown_blocks(markdown: &str) -> Vec<DetailBlock> {
    let markdown = clean_terminal_text(markdown);
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut code = String::new();
    let mut fence: Option<(char, usize, Option<String>)> = None;

    for line in markdown.split_inclusive('\n') {
        let candidate = line.trim_end_matches(['\r', '\n']);
        if let Some((marker, width, language)) = &fence {
            if closing_fence(candidate, *marker, *width) {
                blocks.push(DetailBlock::Code {
                    language: language.clone(),
                    text: std::mem::take(&mut code),
                });
                fence = None;
            } else {
                code.push_str(line);
            }
            continue;
        }
        if let Some(opening) = opening_fence(candidate) {
            if !prose.is_empty() {
                blocks.push(DetailBlock::Text(
                    std::mem::take(&mut prose).trim_end().to_owned(),
                ));
            }
            fence = Some(opening);
        } else {
            prose.push_str(line);
        }
    }
    if let Some((_, _, language)) = fence {
        blocks.push(DetailBlock::Code { language, text: code });
    }
    if !prose.is_empty() {
        blocks.push(DetailBlock::Text(prose.trim_end().to_owned()));
    }
    if blocks.is_empty() && !markdown.is_empty() {
        blocks.push(DetailBlock::Text(markdown));
    }
    blocks
}

fn opening_fence(line: &str) -> Option<(char, usize, Option<String>)> {
    let line = line.trim_start_matches(' ');
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let width = line.chars().take_while(|ch| *ch == marker).count();
    if width < 3 {
        return None;
    }
    let info = line[width..].trim();
    if marker == '`' && info.contains('`') {
        return None;
    }
    let language = info
        .split_whitespace()
        .next()
        .filter(|language| !language.is_empty())
        .map(str::to_owned);
    Some((marker, width, language))
}

fn closing_fence(line: &str, marker: char, width: usize) -> bool {
    let line = line.trim_start_matches(' ');
    if line.chars().count() < width || !line.chars().take(width).all(|ch| ch == marker) {
        return false;
    }
    line.chars().skip(width).all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_message_decodes_and_summarises_to_one_line() {
        let event = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Added backoff.\nTests pass."}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::AgentMessage {
                text: "Added backoff.\nTests pass.".into()
            }
        );
        assert_eq!(event.tag(), "agent");
        assert_eq!(event.summary(), "Added backoff.");
    }

    #[test]
    fn command_lifecycle_decodes_with_status_and_output() {
        let started = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.started","item":{"id":"c1","type":"command_execution","command":"cargo test"}}"#,
        );
        assert_eq!(started, AgentEvent::CommandStarted { command: "cargo test".into() });

        let completed = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"cargo test","status":"completed","exit_code":0,"aggregated_output":"ok"}}"#,
        );
        assert_eq!(
            completed,
            AgentEvent::CommandCompleted {
                command: "cargo test".into(),
                status: "completed".into(),
                exit_code: Some(0),
                output: "ok".into(),
            }
        );
        assert_eq!(completed.summary(), "cargo test (completed, exit 0)");
    }

    #[test]
    fn thread_and_turn_events_decode() {
        assert_eq!(
            decode_line(Protocol::CodexJsonl, r#"{"type":"thread.started","thread_id":"t-7"}"#),
            AgentEvent::ThreadStarted { thread_id: "t-7".into() }
        );
        assert_eq!(
            decode_line(Protocol::CodexJsonl, r#"{"type":"turn.started"}"#),
            AgentEvent::TurnStarted
        );
        let usage = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":3,"cached_input_tokens":2}}"#,
        );
        assert_eq!(
            usage,
            AgentEvent::TurnCompleted {
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 3,
                    cached_input_tokens: 2,
                    reasoning_output_tokens: 0,
                }
            }
        );
        assert_eq!(usage.summary(), "in 10 · out 3 · cached 2");
    }

    #[test]
    fn file_change_collects_paths() {
        let event = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.completed","item":{"id":"f1","type":"file_change","changes":[{"path":"src/a.rs"},{"path":"src/b.rs"}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::FileChanged {
                id: "f1".into(),
                paths: vec!["src/a.rs".into(), "src/b.rs".into()],
                checkpoint: None,
                checkpoint_error: None,
            }
        );
        assert_eq!(event.summary(), "src/a.rs, src/b.rs");
    }

    #[test]
    fn unknown_and_malformed_are_preserved_not_dropped() {
        assert_eq!(
            decode_line(Protocol::CodexJsonl, r#"{"type":"future.event"}"#),
            AgentEvent::Unknown { wire_type: "future.event".into() }
        );
        assert!(matches!(
            decode_line(Protocol::CodexJsonl, "not json"),
            AgentEvent::Malformed { .. }
        ));
    }

    #[test]
    fn terminal_escapes_are_stripped_from_decoded_text() {
        // Valid JSON escapes the ESC byte as \u001b, as real agent output does.
        let event = decode_line(
            Protocol::CodexJsonl,
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"\\u001b[31mred\\u001b[0m done\"}}",
        );
        assert_eq!(event, AgentEvent::AgentMessage { text: "red done".into() });
    }

    #[test]
    fn markdown_detail_separates_prose_and_code() {
        let blocks = markdown_blocks("before\n```rust\nfn main() {}\n```\nafter");
        assert_eq!(
            blocks,
            vec![
                DetailBlock::Text("before".into()),
                DetailBlock::Code { language: Some("rust".into()), text: "fn main() {}\n".into() },
                DetailBlock::Text("after".into()),
            ]
        );
    }

    #[test]
    fn agent_message_detail_uses_markdown_blocks() {
        let event = AgentEvent::AgentMessage { text: "text\n```\ncode\n```".into() };
        assert_eq!(
            event.detail(),
            vec![
                DetailBlock::Text("text".into()),
                DetailBlock::Code { language: None, text: "code\n".into() },
            ]
        );
    }

    #[test]
    fn claude_init_starts_a_thread_and_other_system_lines_are_unknown() {
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"system","subtype":"init","session_id":"s-9","model":"claude-opus-4-8"}"#,
            ),
            AgentEvent::ThreadStarted { thread_id: "s-9".into() }
        );
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"system","subtype":"compact_boundary"}"#,
            ),
            AgentEvent::Unknown { wire_type: "system:compact_boundary".into() }
        );
    }

    #[test]
    fn claude_assistant_text_decodes_to_an_agent_message() {
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Added backoff.\nTests pass."}]},"session_id":"s"}"#,
        );
        assert_eq!(
            event,
            AgentEvent::AgentMessage { text: "Added backoff.\nTests pass.".into() }
        );
        assert_eq!(event.summary(), "Added backoff.");
    }

    #[test]
    fn claude_tool_use_is_preferred_over_prose_in_the_same_message() {
        // A tool call is the action worth surfacing; the preamble text stays in
        // the verbatim raw view.
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Running the tests."},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::ToolStarted { name: "Bash".into(), detail: "{\"command\":\"cargo test\"}".into() }
        );
        assert_eq!(event.summary(), "Bash: {\"command\":\"cargo test\"}");
    }

    #[test]
    fn claude_thinking_only_message_falls_back_to_an_agent_message() {
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"weigh the options"}]}}"#,
        );
        assert_eq!(event, AgentEvent::AgentMessage { text: "weigh the options".into() });
    }

    #[test]
    fn claude_tool_result_completes_a_tool_with_its_status() {
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#,
            ),
            AgentEvent::ToolCompleted { name: "toolu_1".into(), status: "completed".into() }
        );
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","is_error":true,"content":"boom"}]}}"#,
            ),
            AgentEvent::ToolCompleted { name: "toolu_2".into(), status: "error".into() }
        );
    }

    #[test]
    fn claude_result_carries_usage_or_surfaces_an_error() {
        let usage = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":12,"output_tokens":3,"cache_read_input_tokens":8}}"#,
        );
        assert_eq!(
            usage,
            AgentEvent::TurnCompleted {
                usage: TokenUsage {
                    input_tokens: 12,
                    cached_input_tokens: 8,
                    output_tokens: 3,
                    reasoning_output_tokens: 0,
                }
            }
        );
        assert_eq!(usage.summary(), "in 12 · out 3 · cached 8");

        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"hit the turn limit"}"#,
            ),
            AgentEvent::Error { message: "hit the turn limit".into() }
        );
    }

    #[test]
    fn claude_unknown_and_malformed_are_preserved() {
        assert_eq!(
            decode_line(Protocol::ClaudeJsonl, r#"{"type":"stream_event"}"#),
            AgentEvent::Unknown { wire_type: "stream_event".into() }
        );
        assert!(matches!(
            decode_line(Protocol::ClaudeJsonl, "not json"),
            AgentEvent::Malformed { .. }
        ));
    }

    #[test]
    fn summary_is_flattened_and_truncated() {
        let long = "x".repeat(500);
        let event = AgentEvent::AgentMessage { text: long };
        let summary = event.summary();
        assert!(summary.chars().count() <= 200);
        assert!(summary.ends_with('…'));
    }

    // The following lines are copied verbatim from a live `codex app-server`
    // session (codex-cli 0.145) captured during development.

    #[test]
    fn appserver_notifications_decode_from_real_output() {
        let d = |line| decode_line(Protocol::CodexAppServer, line);
        assert_eq!(
            d(r#"{"method":"thread/started","params":{"thread":{"id":"019f8f61-b7df-7291-81fc-04ff0bfb786f"}}}"#),
            AgentEvent::ThreadStarted { thread_id: "019f8f61-b7df-7291-81fc-04ff0bfb786f".into() }
        );
        assert_eq!(d(r#"{"method":"turn/started","params":{"threadId":"t"}}"#), AgentEvent::TurnStarted);
        assert_eq!(
            d(r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_1","text":"hello","phase":"final_answer"}}}"#),
            AgentEvent::AgentMessage { text: "hello".into() }
        );
    }

    #[test]
    fn appserver_command_execution_decodes_both_ends() {
        let started = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"i0","command":"/usr/bin/bash -lc 'echo hi'","status":"in_progress","exitCode":null}}}"#,
        );
        assert_eq!(
            started,
            AgentEvent::CommandStarted { command: "/usr/bin/bash -lc 'echo hi'".into() }
        );
        let completed = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"i0","command":"/usr/bin/bash -lc 'echo hi'","aggregatedOutput":"hi\n","exitCode":0,"status":"completed"}}}"#,
        );
        assert_eq!(
            completed,
            AgentEvent::CommandCompleted {
                command: "/usr/bin/bash -lc 'echo hi'".into(),
                status: "completed".into(),
                exit_code: Some(0),
                output: "hi\n".into(),
            }
        );
    }

    #[test]
    fn appserver_token_usage_maps_to_turn_completed() {
        let event = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"totalTokens":12603,"inputTokens":12598,"cachedInputTokens":9600,"cacheWriteInputTokens":0,"outputTokens":5,"reasoningOutputTokens":0}}}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::TurnCompleted {
                usage: TokenUsage {
                    input_tokens: 12598,
                    cached_input_tokens: 9600,
                    output_tokens: 5,
                    reasoning_output_tokens: 0,
                }
            }
        );
    }

    #[test]
    fn appserver_control_and_echoed_user_message_carry_without_rendering() {
        // A response (no "method") is control traffic.
        assert_eq!(
            decode_line(Protocol::CodexAppServer, r#"{"id":2,"result":{"thread":{"id":"t"}}}"#),
            AgentEvent::Unknown { wire_type: "response".into() }
        );
        // The server echoes the operator's own message; the host shows its own, so
        // this decodes to Unknown rather than duplicating it.
        assert_eq!(
            decode_line(
                Protocol::CodexAppServer,
                r#"{"method":"item/completed","params":{"item":{"type":"userMessage","id":"u","content":[{"type":"text","text":"hi"}]}}}"#
            ),
            AgentEvent::Unknown { wire_type: "item:userMessage".into() }
        );
    }
}
