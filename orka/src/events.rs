//! Agent event normalization, durable projection, and terminal rendering.
//!
//! Provider wire formats stop here. The rest of Orka consumes a small stable
//! event vocabulary and Driva remains an uninterpreted process transport.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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

/// Provider-independent events retained by Orka and consumed by its views.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted,
    CommandStarted {
        id: String,
        command: String,
    },
    CommandCompleted {
        id: String,
        command: String,
        status: String,
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        output: String,
    },
    FileChanged {
        id: String,
        paths: Vec<String>,
    },
    ToolStarted {
        id: String,
        name: String,
        detail: String,
    },
    ToolCompleted {
        id: String,
        name: String,
        status: String,
    },
    PlanUpdated {
        id: String,
        text: String,
    },
    AgentMessage {
        id: String,
        text: String,
    },
    TurnCompleted {
        usage: TokenUsage,
    },
    Error {
        message: String,
    },
    Unknown {
        wire_type: String,
    },
    Malformed {
        error: String,
    },
}

/// Decode one `codex exec --json` line without assuming every future event is
/// known. Unknown events remain visible and the exact input remains in the raw
/// journal.
pub fn decode_codex_line(line: &str) -> AgentEvent {
    match serde_json::from_str::<Value>(line) {
        Ok(value) => decode_codex_value(&value),
        Err(error) => AgentEvent::Malformed {
            error: error.to_string(),
        },
    }
}

fn decode_codex_value(value: &Value) -> AgentEvent {
    let wire_type = string(value, "type").unwrap_or("unknown");
    match wire_type {
        "thread.started" => AgentEvent::ThreadStarted {
            thread_id: string(value, "thread_id").unwrap_or_default().into(),
        },
        "turn.started" => AgentEvent::TurnStarted,
        "turn.completed" => AgentEvent::TurnCompleted {
            usage: serde_json::from_value(value.get("usage").cloned().unwrap_or_default())
                .unwrap_or_default(),
        },
        "turn.failed" | "error" => AgentEvent::Error {
            message: error_message(value),
        },
        "item.started" | "item.updated" | "item.completed" => {
            decode_item(wire_type, value.get("item").unwrap_or(&Value::Null))
        }
        other => AgentEvent::Unknown {
            wire_type: other.into(),
        },
    }
}

fn decode_item(event_type: &str, item: &Value) -> AgentEvent {
    let id = string(item, "id").unwrap_or_default().to_owned();
    let kind = string(item, "type").unwrap_or("unknown");
    let completed = event_type == "item.completed";
    match (kind, completed) {
        ("command_execution", false) => AgentEvent::CommandStarted {
            id,
            command: string(item, "command").unwrap_or_default().into(),
        },
        ("command_execution", true) => AgentEvent::CommandCompleted {
            id,
            command: string(item, "command").unwrap_or_default().into(),
            status: string(item, "status").unwrap_or("completed").into(),
            exit_code: item.get("exit_code").and_then(Value::as_i64),
            output: string(item, "aggregated_output")
                .or_else(|| string(item, "output"))
                .unwrap_or_default()
                .into(),
        },
        ("file_change", true) => AgentEvent::FileChanged {
            id,
            paths: changed_paths(item),
        },
        ("agent_message", true) => AgentEvent::AgentMessage {
            id,
            text: string(item, "text").unwrap_or_default().into(),
        },
        ("plan", true) | ("plan_update", true) => AgentEvent::PlanUpdated {
            id,
            text: string(item, "text")
                .or_else(|| string(item, "plan"))
                .unwrap_or_default()
                .into(),
        },
        ("mcp_tool_call", false) | ("web_search", false) => AgentEvent::ToolStarted {
            id,
            name: tool_name(item, kind),
            detail: tool_detail(item),
        },
        ("mcp_tool_call", true) | ("web_search", true) => AgentEvent::ToolCompleted {
            id,
            name: tool_name(item, kind),
            status: string(item, "status").unwrap_or("completed").into(),
        },
        _ => AgentEvent::Unknown {
            wire_type: format!("{event_type}:{kind}"),
        },
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn error_message(value: &Value) -> String {
    string(value, "message")
        .or_else(|| {
            value
                .get("error")
                .and_then(|error| string(error, "message"))
        })
        .unwrap_or("agent execution failed")
        .into()
}

fn tool_name(item: &Value, fallback: &str) -> String {
    string(item, "tool")
        .or_else(|| string(item, "name"))
        .or_else(|| string(item, "query"))
        .unwrap_or(fallback)
        .into()
}

fn tool_detail(item: &Value) -> String {
    string(item, "server")
        .or_else(|| string(item, "query"))
        .unwrap_or_default()
        .into()
}

fn changed_paths(item: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(changes) = item.get("changes").and_then(Value::as_array) {
        for change in changes {
            if let Some(path) = string(change, "path").or_else(|| string(change, "file_path")) {
                paths.push(path.into());
            }
        }
    }
    if paths.is_empty() {
        if let Some(path) = string(item, "path").or_else(|| string(item, "file_path")) {
            paths.push(path.into());
        }
    }
    paths
}

/// Convert the exact Codex journal into a versioned normalized journal and a
/// compact human-readable transcript after execution completes.
pub fn materialize_codex_events(raw: &Path, normalized: &Path, transcript: &Path) -> Result<()> {
    let input = File::open(raw).with_context(|| format!("opening {}", raw.display()))?;
    let mut events = BufWriter::new(
        File::create(normalized).with_context(|| format!("creating {}", normalized.display()))?,
    );
    let mut readable = BufWriter::new(
        File::create(transcript).with_context(|| format!("creating {}", transcript.display()))?,
    );

    for line in BufReader::new(input).lines() {
        let line = line.with_context(|| format!("reading {}", raw.display()))?;
        let event = decode_codex_line(&line);
        serde_json::to_writer(&mut events, &event)?;
        events.write_all(b"\n")?;
        write_transcript_event(&mut readable, &event)?;
    }
    events.flush()?;
    readable.flush()?;
    Ok(())
}

fn write_transcript_event(out: &mut dyn Write, event: &AgentEvent) -> Result<()> {
    match event {
        AgentEvent::CommandStarted { command, .. } => {
            writeln!(out, "$ {}", clean_terminal_text(command))?
        }
        AgentEvent::CommandCompleted {
            exit_code, output, ..
        } => {
            writeln!(
                out,
                "[exit {}]",
                exit_code.map_or("?".into(), |v| v.to_string())
            )?;
            if !output.is_empty() {
                writeln!(out, "{}", clean_terminal_text(output))?;
            }
        }
        AgentEvent::FileChanged { paths, .. } if !paths.is_empty() => {
            writeln!(out, "[changed] {}", clean_terminal_text(&paths.join(", ")))?
        }
        AgentEvent::ToolStarted { name, detail, .. } => writeln!(
            out,
            "[tool] {} {}",
            clean_terminal_text(name),
            clean_terminal_text(detail)
        )?,
        AgentEvent::PlanUpdated { text, .. } => {
            writeln!(out, "[plan]\n{}", clean_terminal_text(text))?
        }
        AgentEvent::AgentMessage { text, .. } => {
            writeln!(out, "\n{}\n", clean_terminal_text(text))?
        }
        AgentEvent::Error { message } => writeln!(out, "[error] {}", clean_terminal_text(message))?,
        AgentEvent::Malformed { error } => {
            writeln!(out, "[malformed event] {}", clean_terminal_text(error))?
        }
        _ => {}
    }
    Ok(())
}

/// Follow a growing raw journal until `done`, rendering complete JSONL records
/// as soon as they become visible.
pub fn follow_codex_events(path: &Path, done: &AtomicBool, color: bool) -> Result<()> {
    while !path.exists() && !done.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(25));
    }
    if !path.exists() {
        return Ok(());
    }

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("following {}", path.display()))?;
    let mut input = BufReader::new(file);
    let stderr = std::io::stderr();
    let mut renderer = RichRenderer::new(stderr.lock(), color);
    let mut line = String::new();
    let mut pending = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line)? {
            0 if done.load(Ordering::Acquire) => {
                if !pending.is_empty() {
                    renderer.render(&decode_codex_line(&pending))?;
                }
                break;
            }
            0 => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                pending.push_str(&line);
                if pending.ends_with('\n') {
                    renderer.render(&decode_codex_line(pending.trim_end()))?;
                    pending.clear();
                } else {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
    Ok(())
}

pub struct RichRenderer<W> {
    out: W,
    color: bool,
}

impl<W: Write> RichRenderer<W> {
    pub fn new(out: W, color: bool) -> Self {
        Self { out, color }
    }

    pub fn render(&mut self, event: &AgentEvent) -> Result<()> {
        let (bold, cyan, green, yellow, red, dim, reset) = if self.color {
            (
                "\x1b[1m", "\x1b[36m", "\x1b[32m", "\x1b[33m", "\x1b[31m", "\x1b[2m", "\x1b[0m",
            )
        } else {
            ("", "", "", "", "", "", "")
        };
        match event {
            AgentEvent::ThreadStarted { thread_id } => {
                writeln!(self.out, "{dim}session {thread_id}{reset}")?
            }
            AgentEvent::TurnStarted => writeln!(self.out, "\n{bold}{cyan}━━ Agent turn{reset}")?,
            AgentEvent::CommandStarted { command, .. } => {
                writeln!(self.out, "{cyan}▶{reset} {}", clean_terminal_text(command))?
            }
            AgentEvent::CommandCompleted {
                command,
                status,
                exit_code,
                output,
                ..
            } => {
                let succeeded =
                    exit_code == &Some(0) || (exit_code.is_none() && status == "completed");
                let (mark, tone) = if succeeded {
                    ("✓", green)
                } else {
                    ("✗", red)
                };
                writeln!(
                    self.out,
                    "{tone}{mark}{reset} {} {dim}[{}{}]{reset}",
                    clean_terminal_text(command),
                    status,
                    exit_code.map_or(String::new(), |code| format!(", exit {code}"))
                )?;
                if !succeeded && !output.is_empty() {
                    for line in clean_terminal_text(output).lines().take(20) {
                        writeln!(self.out, "  {dim}│{reset} {line}")?;
                    }
                }
            }
            AgentEvent::FileChanged { paths, .. } => {
                let detail = if paths.is_empty() {
                    "files".into()
                } else {
                    paths.join(", ")
                };
                writeln!(
                    self.out,
                    "{yellow}✎{reset} changed {}",
                    clean_terminal_text(&detail)
                )?;
            }
            AgentEvent::ToolStarted { name, detail, .. } => writeln!(
                self.out,
                "{cyan}◆{reset} {} {}",
                clean_terminal_text(name),
                clean_terminal_text(detail)
            )?,
            AgentEvent::ToolCompleted { name, status, .. } => writeln!(
                self.out,
                "{green}◇{reset} {} {dim}[{}]{reset}",
                clean_terminal_text(name),
                clean_terminal_text(status)
            )?,
            AgentEvent::PlanUpdated { text, .. } => {
                writeln!(self.out, "{bold}{yellow}Plan{reset}")?;
                self.render_markdown(text)?;
            }
            AgentEvent::AgentMessage { text, .. } => {
                writeln!(self.out, "{bold}{cyan}╭─ Agent{reset}")?;
                self.render_markdown(text)?;
                writeln!(self.out, "{cyan}╰─{reset}")?;
            }
            AgentEvent::TurnCompleted { usage } => writeln!(
                self.out,
                "{dim}tokens: {} input ({} cached), {} output{reset}",
                usage.input_tokens, usage.cached_input_tokens, usage.output_tokens
            )?,
            AgentEvent::Error { message } => writeln!(
                self.out,
                "{red}✗ agent error:{reset} {}",
                clean_terminal_text(message)
            )?,
            AgentEvent::Malformed { error } => writeln!(
                self.out,
                "{red}✗ malformed agent event:{reset} {}",
                clean_terminal_text(error)
            )?,
            AgentEvent::Unknown { .. } => {}
        }
        self.out.flush()?;
        Ok(())
    }

    fn render_markdown(&mut self, markdown: &str) -> Result<()> {
        for line in clean_terminal_text(markdown).lines() {
            let shown = line.strip_prefix("# ").unwrap_or(line);
            writeln!(self.out, "  │ {shown}")?;
        }
        Ok(())
    }
}

/// Remove terminal control sequences before placing agent-controlled content
/// in the operator's terminal. Newlines and tabs remain useful formatting.
pub fn clean_terminal_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut clean = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'[' => {
                    i += 1;
                    while i < bytes.len() {
                        let byte = bytes[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&byte) {
                            break;
                        }
                    }
                }
                b']' => {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => i += 1,
            }
            continue;
        }
        let byte = bytes[i];
        if byte >= 0x20 || matches!(byte, b'\n' | b'\t') {
            clean.push(byte);
        }
        i += 1;
    }
    String::from_utf8_lossy(&clean).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_command_message_file_and_usage_events() {
        assert!(matches!(
            decode_codex_line(r#"{"type":"item.started","item":{"id":"1","type":"command_execution","command":"cargo test","status":"in_progress"}}"#),
            AgentEvent::CommandStarted { command, .. } if command == "cargo test"
        ));
        assert!(matches!(
            decode_codex_line(r#"{"type":"item.completed","item":{"id":"2","type":"agent_message","text":"Done"}}"#),
            AgentEvent::AgentMessage { text, .. } if text == "Done"
        ));
        assert!(matches!(
            decode_codex_line(r#"{"type":"item.completed","item":{"id":"3","type":"file_change","changes":[{"path":"src/main.rs"}]}}"#),
            AgentEvent::FileChanged { paths, .. } if paths == ["src/main.rs"]
        ));
        assert!(matches!(
            decode_codex_line(r#"{"type":"turn.completed","usage":{"input_tokens":12,"cached_input_tokens":8,"output_tokens":3}}"#),
            AgentEvent::TurnCompleted { usage } if usage.input_tokens == 12 && usage.output_tokens == 3
        ));
    }

    #[test]
    fn malformed_and_future_events_are_nonfatal() {
        assert!(matches!(
            decode_codex_line("{"),
            AgentEvent::Malformed { .. }
        ));
        assert_eq!(
            decode_codex_line(r#"{"type":"future.event"}"#),
            AgentEvent::Unknown {
                wire_type: "future.event".into()
            }
        );
    }

    #[test]
    fn renderer_sanitizes_terminal_controls() {
        let mut output = Vec::new();
        RichRenderer::new(&mut output, false)
            .render(&AgentEvent::AgentMessage {
                id: "1".into(),
                text: "safe\x1b[31mred\x1b[0m\x1b]0;title\x07".into(),
            })
            .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("safered"));
        assert!(!output.contains('\x1b'));
        assert!(!output.contains("title"));
    }

    #[test]
    fn materializes_normalized_events_and_readable_transcript() {
        let directory = std::env::temp_dir().join(format!("orka-event-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&directory).unwrap();
        let raw = directory.join("events.raw.jsonl");
        let normalized = directory.join("events.v1.jsonl");
        let transcript = directory.join("transcript.log");
        std::fs::write(
            &raw,
            concat!(
                "{\"type\":\"item.started\",\"item\":{\"id\":\"c1\",\"type\":\"command_execution\",\"command\":\"cargo test\"}}\n",
                "{\"type\":\"item.completed\",\"item\":{\"id\":\"m1\",\"type\":\"agent_message\",\"text\":\"All tests pass\"}}\n"
            ),
        )
        .unwrap();

        materialize_codex_events(&raw, &normalized, &transcript).unwrap();

        let events = std::fs::read_to_string(normalized).unwrap();
        assert!(events.contains("command_started"));
        assert!(events.contains("agent_message"));
        let readable = std::fs::read_to_string(transcript).unwrap();
        assert!(readable.contains("$ cargo test"));
        assert!(readable.contains("All tests pass"));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
