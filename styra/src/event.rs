//! Agent event vocabulary, wire decoding, and presentation.
//!
//! The provider wire format stops here. The rest of Styra consumes only
//! [`StyraEvent`] and its rendered [`summary`](StyraEvent::summary) and
//! [`detail`](StyraEvent::detail); Driva remains an uninterpreted transport.
//!
//! Decoding is versioned by [`Protocol`], exactly as Orka versions its agent
//! output: a new wire format is a new `Protocol` variant plus a decode arm, and
//! the match is exhaustive, so a missing decoder is a compile error rather than
//! a silent mis-decode.

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

/// Styra's stable, provider-independent event vocabulary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StyraEvent {
    /// A message the operator sent to the agent, recorded so their own turns
    /// appear inline in the same list. Styra-originated, never decoded.
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
        paths: Vec<String>,
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
    /// A recognised envelope with no Styra view; carried, not rendered as prose.
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

impl StyraEvent {
    /// The short tag shown at the head of the collapsed list line.
    pub fn tag(&self) -> &'static str {
        match self {
            StyraEvent::UserMessage { .. } => "user",
            StyraEvent::ThreadStarted { .. } => "thread",
            StyraEvent::TurnStarted => "turn",
            StyraEvent::TurnCompleted { .. } => "usage",
            StyraEvent::CommandStarted { .. } | StyraEvent::CommandCompleted { .. } => "command",
            StyraEvent::FileChanged { .. } => "files",
            StyraEvent::ToolStarted { .. } | StyraEvent::ToolCompleted { .. } => "tool",
            StyraEvent::PlanUpdated { .. } => "plan",
            StyraEvent::AgentMessage { .. } => "agent",
            StyraEvent::Error { .. } => "error",
            StyraEvent::Unknown { .. } => "unknown",
            StyraEvent::Malformed { .. } => "malformed",
        }
    }

    /// A single collapsed-line summary. Never contains newlines.
    pub fn summary(&self) -> String {
        let line = match self {
            StyraEvent::UserMessage { text } => first_line(text),
            StyraEvent::ThreadStarted { thread_id } => format!("session {thread_id}"),
            StyraEvent::TurnStarted => "turn started".into(),
            StyraEvent::TurnCompleted { usage } => format!(
                "in {} · out {} · cached {}",
                usage.input_tokens, usage.output_tokens, usage.cached_input_tokens
            ),
            StyraEvent::CommandStarted { command } => first_line(command),
            StyraEvent::CommandCompleted {
                command,
                status,
                exit_code,
                ..
            } => match exit_code {
                Some(code) => format!("{} ({status}, exit {code})", first_line(command)),
                None => format!("{} ({status})", first_line(command)),
            },
            StyraEvent::FileChanged { paths } => paths.join(", "),
            StyraEvent::ToolStarted { name, detail } if !detail.is_empty() => {
                format!("{name}: {}", first_line(detail))
            }
            StyraEvent::ToolStarted { name, .. } => name.clone(),
            StyraEvent::ToolCompleted { name, status } => format!("{name} ({status})"),
            StyraEvent::PlanUpdated { text } | StyraEvent::AgentMessage { text } => first_line(text),
            StyraEvent::Error { message } => first_line(message),
            StyraEvent::Unknown { wire_type } => wire_type.clone(),
            StyraEvent::Malformed { error } => first_line(error),
        };
        truncate_line(&line, 200)
    }

    /// The expandable detail body as escape-free structured blocks.
    pub fn detail(&self) -> Vec<DetailBlock> {
        match self {
            StyraEvent::UserMessage { text } => markdown_blocks(text),
            StyraEvent::ThreadStarted { thread_id } => {
                vec![DetailBlock::Text(format!("thread id: {thread_id}"))]
            }
            StyraEvent::TurnStarted => Vec::new(),
            StyraEvent::TurnCompleted { usage } => vec![DetailBlock::Text(format!(
                "input {} · cached input {} · output {} · reasoning {}",
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.output_tokens,
                usage.reasoning_output_tokens
            ))],
            StyraEvent::CommandStarted { command } => {
                vec![DetailBlock::Code {
                    language: None,
                    text: command.clone(),
                }]
            }
            StyraEvent::CommandCompleted {
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
            StyraEvent::FileChanged { paths } => {
                vec![DetailBlock::Text(paths.join("\n"))]
            }
            StyraEvent::ToolStarted { name, detail } => {
                let mut text = name.clone();
                if !detail.is_empty() {
                    text.push('\n');
                    text.push_str(detail);
                }
                vec![DetailBlock::Text(text)]
            }
            StyraEvent::ToolCompleted { name, status } => {
                vec![DetailBlock::Text(format!("{name}: {status}"))]
            }
            StyraEvent::PlanUpdated { text } | StyraEvent::AgentMessage { text } => {
                markdown_blocks(text)
            }
            StyraEvent::Error { message } => vec![DetailBlock::Text(message.clone())],
            StyraEvent::Unknown { wire_type } => {
                vec![DetailBlock::Text(format!("unrecognised event: {wire_type}"))]
            }
            StyraEvent::Malformed { error } => vec![DetailBlock::Text(error.clone())],
        }
    }
}

/// Decode one wire line under the given protocol. Never fails: undecodable
/// input becomes [`StyraEvent::Malformed`] so nothing is silently lost.
pub fn decode_line(protocol: Protocol, line: &str) -> StyraEvent {
    match protocol {
        Protocol::CodexJsonl => decode_codex_line(line),
        Protocol::CodexAppServer => decode_appserver_line(line),
    }
}

/// Decode one `codex app-server` line. Notifications (which carry a `method`)
/// map to events; requests and responses are control traffic and decode to
/// [`StyraEvent::Unknown`] so they are carried without cluttering the list.
fn decode_appserver_line(line: &str) -> StyraEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return StyraEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    match string(&value, "method") {
        Some(method) => {
            decode_appserver_notification(method, value.get("params").unwrap_or(&Value::Null))
        }
        None => StyraEvent::Unknown {
            wire_type: "response".into(),
        },
    }
}

fn decode_appserver_notification(method: &str, params: &Value) -> StyraEvent {
    match method {
        "thread/started" => StyraEvent::ThreadStarted {
            thread_id: clean_terminal_text(
                params
                    .get("thread")
                    .and_then(|thread| string(thread, "id"))
                    .unwrap_or_default(),
            ),
        },
        "turn/started" => StyraEvent::TurnStarted,
        // The turn's usage arrives here, just before `turn/completed` (which
        // carries none), so this is what marks a turn done and shows usage.
        "thread/tokenUsage/updated" => StyraEvent::TurnCompleted {
            usage: appserver_usage(params),
        },
        "item/started" => {
            decode_appserver_item(params.get("item").unwrap_or(&Value::Null), false)
        }
        "item/completed" => {
            decode_appserver_item(params.get("item").unwrap_or(&Value::Null), true)
        }
        "error" | "warning" | "guardianWarning" | "configWarning" => StyraEvent::Error {
            message: clean_terminal_text(
                string(params, "message").unwrap_or("agent reported an error"),
            ),
        },
        other => StyraEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

fn decode_appserver_item(item: &Value, completed: bool) -> StyraEvent {
    let kind = string(item, "type").unwrap_or("unknown");
    let clean = |value: &str| clean_terminal_text(value);
    match (kind, completed) {
        ("agentMessage", true) => StyraEvent::AgentMessage {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("commandExecution", false) => StyraEvent::CommandStarted {
            command: clean(string(item, "command").unwrap_or_default()),
        },
        ("commandExecution", true) => StyraEvent::CommandCompleted {
            command: clean(string(item, "command").unwrap_or_default()),
            status: clean(string(item, "status").unwrap_or("completed")),
            exit_code: item.get("exitCode").and_then(Value::as_i64),
            output: clean(string(item, "aggregatedOutput").unwrap_or_default()),
        },
        ("fileChange", true) => StyraEvent::FileChanged {
            paths: item
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| string(change, "path"))
                        .map(clean)
                        .collect()
                })
                .unwrap_or_default(),
        },
        ("plan", true) => StyraEvent::PlanUpdated {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("mcpToolCall", false) | ("webSearch", false) => StyraEvent::ToolStarted {
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
        },
        ("mcpToolCall", true) | ("webSearch", true) => StyraEvent::ToolCompleted {
            name: clean(tool_name(item, kind)),
            status: clean(string(item, "status").unwrap_or("completed")),
        },
        // userMessage (echoed back — Styra shows its own), reasoning, deltas,
        // and item lifecycles with no view carry without rendering.
        _ => StyraEvent::Unknown {
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

fn decode_codex_line(line: &str) -> StyraEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return StyraEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    decode_codex_value(&value)
}

fn decode_codex_value(value: &Value) -> StyraEvent {
    let wire_type = string(value, "type").unwrap_or("unknown");
    match wire_type {
        "thread.started" => StyraEvent::ThreadStarted {
            thread_id: clean_terminal_text(string(value, "thread_id").unwrap_or_default()),
        },
        "turn.started" => StyraEvent::TurnStarted,
        "turn.completed" => StyraEvent::TurnCompleted {
            usage: value
                .get("usage")
                .and_then(|usage| serde_json::from_value(usage.clone()).ok())
                .unwrap_or_default(),
        },
        "turn.failed" | "error" => StyraEvent::Error {
            message: clean_terminal_text(error_message(value)),
        },
        "item.started" | "item.updated" | "item.completed" => {
            decode_codex_item(wire_type, value.get("item").unwrap_or(&Value::Null))
        }
        other => StyraEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

fn decode_codex_item(event_type: &str, item: &Value) -> StyraEvent {
    let kind = string(item, "type").unwrap_or("unknown");
    let completed = event_type == "item.completed";
    let clean = |value: &str| clean_terminal_text(value);
    match (kind, completed) {
        ("command_execution", false) => StyraEvent::CommandStarted {
            command: clean(string(item, "command").unwrap_or_default()),
        },
        ("command_execution", true) => StyraEvent::CommandCompleted {
            command: clean(string(item, "command").unwrap_or_default()),
            status: clean(string(item, "status").unwrap_or("completed")),
            exit_code: item.get("exit_code").and_then(Value::as_i64),
            output: clean(
                string(item, "aggregated_output")
                    .or_else(|| string(item, "output"))
                    .unwrap_or_default(),
            ),
        },
        ("file_change", true) => StyraEvent::FileChanged {
            paths: item
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| string(change, "path"))
                        .map(clean)
                        .collect()
                })
                .unwrap_or_default(),
        },
        ("agent_message", true) => StyraEvent::AgentMessage {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("plan", true) | ("plan_update", true) => StyraEvent::PlanUpdated {
            text: clean(
                string(item, "text")
                    .or_else(|| string(item, "plan"))
                    .unwrap_or_default(),
            ),
        },
        ("mcp_tool_call", false) | ("web_search", false) => StyraEvent::ToolStarted {
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
        },
        ("mcp_tool_call", true) | ("web_search", true) => StyraEvent::ToolCompleted {
            name: clean(tool_name(item, kind)),
            status: clean(string(item, "status").unwrap_or("completed")),
        },
        (_, _) => StyraEvent::Unknown {
            wire_type: format!("{event_type}:{kind}"),
        },
    }
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
            StyraEvent::AgentMessage {
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
        assert_eq!(started, StyraEvent::CommandStarted { command: "cargo test".into() });

        let completed = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"cargo test","status":"completed","exit_code":0,"aggregated_output":"ok"}}"#,
        );
        assert_eq!(
            completed,
            StyraEvent::CommandCompleted {
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
            StyraEvent::ThreadStarted { thread_id: "t-7".into() }
        );
        assert_eq!(
            decode_line(Protocol::CodexJsonl, r#"{"type":"turn.started"}"#),
            StyraEvent::TurnStarted
        );
        let usage = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":3,"cached_input_tokens":2}}"#,
        );
        assert_eq!(
            usage,
            StyraEvent::TurnCompleted {
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
            StyraEvent::FileChanged { paths: vec!["src/a.rs".into(), "src/b.rs".into()] }
        );
        assert_eq!(event.summary(), "src/a.rs, src/b.rs");
    }

    #[test]
    fn unknown_and_malformed_are_preserved_not_dropped() {
        assert_eq!(
            decode_line(Protocol::CodexJsonl, r#"{"type":"future.event"}"#),
            StyraEvent::Unknown { wire_type: "future.event".into() }
        );
        assert!(matches!(
            decode_line(Protocol::CodexJsonl, "not json"),
            StyraEvent::Malformed { .. }
        ));
    }

    #[test]
    fn terminal_escapes_are_stripped_from_decoded_text() {
        // Valid JSON escapes the ESC byte as \u001b, as real agent output does.
        let event = decode_line(
            Protocol::CodexJsonl,
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"\\u001b[31mred\\u001b[0m done\"}}",
        );
        assert_eq!(event, StyraEvent::AgentMessage { text: "red done".into() });
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
        let event = StyraEvent::AgentMessage { text: "text\n```\ncode\n```".into() };
        assert_eq!(
            event.detail(),
            vec![
                DetailBlock::Text("text".into()),
                DetailBlock::Code { language: None, text: "code\n".into() },
            ]
        );
    }

    #[test]
    fn summary_is_flattened_and_truncated() {
        let long = "x".repeat(500);
        let event = StyraEvent::AgentMessage { text: long };
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
            StyraEvent::ThreadStarted { thread_id: "019f8f61-b7df-7291-81fc-04ff0bfb786f".into() }
        );
        assert_eq!(d(r#"{"method":"turn/started","params":{"threadId":"t"}}"#), StyraEvent::TurnStarted);
        assert_eq!(
            d(r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_1","text":"hello","phase":"final_answer"}}}"#),
            StyraEvent::AgentMessage { text: "hello".into() }
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
            StyraEvent::CommandStarted { command: "/usr/bin/bash -lc 'echo hi'".into() }
        );
        let completed = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"i0","command":"/usr/bin/bash -lc 'echo hi'","aggregatedOutput":"hi\n","exitCode":0,"status":"completed"}}}"#,
        );
        assert_eq!(
            completed,
            StyraEvent::CommandCompleted {
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
            StyraEvent::TurnCompleted {
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
            StyraEvent::Unknown { wire_type: "response".into() }
        );
        // The server echoes the operator's own message; Styra shows its own, so
        // this decodes to Unknown rather than duplicating it.
        assert_eq!(
            decode_line(
                Protocol::CodexAppServer,
                r#"{"method":"item/completed","params":{"item":{"type":"userMessage","id":"u","content":[{"type":"text","text":"hi"}]}}}"#
            ),
            StyraEvent::Unknown { wire_type: "item:userMessage".into() }
        );
    }
}
