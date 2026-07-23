//! Agent event normalization, durable projection, and terminal rendering.
//!
//! Provider wire formats stop here. The rest of Orka consumes a small stable
//! event vocabulary and Driva remains an uninterpreted process transport.

use crate::agent::AgentProtocol;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

// The event vocabulary and its decoders live in Genta, the shared
// coding-agent library; Orka re-exports them so downstream modules keep their
// import paths.
pub use genta::event::{clean_terminal_text, AgentEvent, TokenUsage};

/// Provider-independent blocks used by every human-facing work-log view.
///
/// These are deliberately presentation-oriented without containing terminal
/// escape sequences or HTML. A terminal can add colours and a browser can add
/// richer styling without either view having to understand provider JSONL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkLogBlock {
    Session {
        id: String,
    },
    TurnStarted,
    CommandStarted {
        command: String,
    },
    CommandCompleted {
        command: String,
        status: String,
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        output: Vec<ContentBlock>,
    },
    FilesChanged {
        paths: Vec<String>,
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
    Plan {
        content: Vec<ContentBlock>,
    },
    AgentMessage {
        content: Vec<ContentBlock>,
    },
    Usage {
        usage: TokenUsage,
    },
    Error {
        message: String,
    },
    Transcript {
        content: Vec<ContentBlock>,
    },
}

/// A safe, structured piece of content within a work-log block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Code {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        text: String,
    },
}

/// Project a normalized event into presentation blocks. Unknown provider
/// events intentionally have no presentation, while malformed input remains
/// visible as an error.
pub fn event_blocks(event: &AgentEvent) -> Vec<WorkLogBlock> {
    let clean = clean_terminal_text;
    let block = match event {
        AgentEvent::ThreadStarted { thread_id } => WorkLogBlock::Session {
            id: clean(thread_id),
        },
        AgentEvent::TurnStarted => WorkLogBlock::TurnStarted,
        AgentEvent::CommandStarted { command, .. } => WorkLogBlock::CommandStarted {
            command: clean(command),
        },
        AgentEvent::CommandCompleted {
            command,
            status,
            exit_code,
            output,
            ..
        } => WorkLogBlock::CommandCompleted {
            command: clean(command),
            status: clean(status),
            exit_code: *exit_code,
            output: if output.is_empty() {
                Vec::new()
            } else {
                vec![ContentBlock::Code {
                    language: None,
                    text: clean(output),
                }]
            },
        },
        AgentEvent::FileChanged {
            paths,
            checkpoint,
            checkpoint_error,
            ..
        } => WorkLogBlock::FilesChanged {
            paths: paths.iter().map(|path| clean(path)).collect(),
            checkpoint: checkpoint.clone(),
            checkpoint_error: checkpoint_error.clone(),
        },
        AgentEvent::ToolStarted { name, detail, .. } => WorkLogBlock::ToolStarted {
            name: clean(name),
            detail: clean(detail),
        },
        AgentEvent::ToolCompleted { name, status, .. } => WorkLogBlock::ToolCompleted {
            name: clean(name),
            status: clean(status),
        },
        AgentEvent::PlanUpdated { text, .. } => WorkLogBlock::Plan {
            content: markdown_blocks(text),
        },
        AgentEvent::AgentMessage { text, .. } => WorkLogBlock::AgentMessage {
            content: markdown_blocks(text),
        },
        AgentEvent::TurnCompleted { usage } => WorkLogBlock::Usage {
            usage: usage.clone(),
        },
        AgentEvent::Error { message } => WorkLogBlock::Error {
            message: clean(message),
        },
        AgentEvent::Malformed { error } => WorkLogBlock::Error {
            message: format!("malformed agent event: {}", clean(error)),
        },
        // Unknown provider events have no presentation; a UserMessage is a
        // host echo of the operator's own input, which Orka's batch runs never
        // produce and its views do not render.
        AgentEvent::Unknown { .. } | AgentEvent::UserMessage { .. } => return Vec::new(),
    };
    vec![block]
}

/// Parse fenced Markdown code into explicit blocks while leaving prose as
/// Markdown text. The language info string is retained so browser views can
/// select a syntax highlighter.
pub fn markdown_blocks(markdown: &str) -> Vec<ContentBlock> {
    let markdown = clean_terminal_text(markdown);
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut code = String::new();
    let mut fence: Option<(char, usize, Option<String>)> = None;

    for line in markdown.split_inclusive('\n') {
        let candidate = line.trim_end_matches(['\r', '\n']);
        if let Some((marker, width, language)) = &fence {
            if closing_fence(candidate, *marker, *width) {
                blocks.push(ContentBlock::Code {
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
                blocks.push(ContentBlock::Text {
                    text: std::mem::take(&mut prose),
                });
            }
            fence = Some(opening);
        } else {
            prose.push_str(line);
        }
    }

    if let Some((_, _, language)) = fence {
        blocks.push(ContentBlock::Code {
            language,
            text: code,
        });
    }
    if !prose.is_empty() {
        blocks.push(ContentBlock::Text { text: prose });
    }
    if blocks.is_empty() && !markdown.is_empty() {
        blocks.push(ContentBlock::Text { text: markdown });
    }
    blocks
}

fn opening_fence(line: &str) -> Option<(char, usize, Option<String>)> {
    let line = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
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
    if line.len() < width || !line.chars().take(width).all(|ch| ch == marker) {
        return false;
    }
    line.chars().skip(width).all(char::is_whitespace)
}

/// Render a work log from a raw agent-output fact, selecting the decoder by the
/// [`AgentProtocol`] that produced it. This is the versioned dispatch point: a
/// new agent wire format is a new `AgentProtocol` variant plus a match arm here,
/// and the exhaustive match makes the compiler demand a decoder for it. An
/// attempt always decodes through its own recorded protocol, so adding decoders
/// never disturbs how older attempts read back.
///
/// `file_changes` supplies checkpoint annotations for decoders that carry
/// file-change events; decoders that do not (e.g. a plain transcript) ignore it.
pub fn work_log_from_raw(
    protocol: AgentProtocol,
    output: &[u8],
    file_changes: Option<&[u8]>,
) -> Result<Vec<WorkLogBlock>> {
    match protocol {
        AgentProtocol::Plain => Ok(transcript_blocks(&String::from_utf8_lossy(output))),
        AgentProtocol::CodexJsonl => work_log_from_codex_raw(output, file_changes),
    }
}

/// Render provider-independent work-log blocks on demand from a raw Codex event
/// stream — a fundamental fact — folding in the per-file-change checkpoint
/// commits from the file-change journal when it is supplied. Both inputs are
/// facts (the exact agent output and the harness's checkpoint mappings); the
/// blocks are an interpretation produced here at read time and never persisted.
///
/// Used both for the local attempt directory and for the durable copy read back
/// from Linka, so either path presents an identical work log.
pub fn work_log_from_codex_raw(
    raw: &[u8],
    file_changes: Option<&[u8]>,
) -> Result<Vec<WorkLogBlock>> {
    let events = decode_codex_events(raw, file_changes)?;
    Ok(events.iter().flat_map(event_blocks).collect())
}

/// Decode a raw Codex journal into the stable event vocabulary, attaching
/// checkpoint commits from the file-change journal in event order. This is the
/// single decode shared by every downstream view; its output is never written
/// to disk.
fn decode_codex_events(raw: &[u8], file_changes: Option<&[u8]>) -> Result<Vec<AgentEvent>> {
    let mut checkpoints = match file_changes {
        Some(bytes) => crate::file_changes::read_checkpoints_bytes(bytes)?,
        None => Vec::new(),
    }
    .into_iter();
    let mut events = Vec::new();
    for line in raw.split(|&byte| byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let line = String::from_utf8_lossy(line);
        let mut event = decode_codex_line(line.trim_end());
        if let AgentEvent::FileChanged {
            id,
            checkpoint,
            checkpoint_error,
            ..
        } = &mut event
        {
            if let Some(record) = checkpoints.next() {
                if record.event_id == *id {
                    *checkpoint = record.commit;
                    *checkpoint_error = record.error;
                } else {
                    *checkpoint_error = Some(format!(
                        "checkpoint journal expected event `{}`, found `{id}`",
                        record.event_id
                    ));
                }
            }
        }
        events.push(event);
    }
    Ok(events)
}

/// Wrap output from a plain-text agent — one that emits no event stream — in
/// the shared presentation format.
pub fn transcript_blocks(transcript: &str) -> Vec<WorkLogBlock> {
    if transcript.is_empty() {
        Vec::new()
    } else {
        vec![WorkLogBlock::Transcript {
            content: vec![ContentBlock::Code {
                language: None,
                text: clean_terminal_text(transcript),
            }],
        }]
    }
}

/// Decode one `codex exec --json` line through Genta's versioned decoder.
/// Unknown events remain visible and the exact input remains in the raw
/// journal.
pub fn decode_codex_line(line: &str) -> AgentEvent {
    genta::event::decode_line(genta::event::Protocol::CodexJsonl, line)
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
        for block in event_blocks(event) {
            self.render_block(&block)?;
        }
        Ok(())
    }

    pub fn render_block(&mut self, block: &WorkLogBlock) -> Result<()> {
        let (bold, cyan, green, yellow, red, dim, reset) = if self.color {
            (
                "\x1b[1m", "\x1b[36m", "\x1b[32m", "\x1b[33m", "\x1b[31m", "\x1b[2m", "\x1b[0m",
            )
        } else {
            ("", "", "", "", "", "", "")
        };
        match block {
            WorkLogBlock::Session { id } => writeln!(self.out, "{dim}session {id}{reset}")?,
            WorkLogBlock::TurnStarted => writeln!(self.out, "\n{bold}{cyan}━━ Agent turn{reset}")?,
            WorkLogBlock::CommandStarted { command } => {
                writeln!(self.out, "{cyan}▶{reset} {command}")?
            }
            WorkLogBlock::CommandCompleted {
                command,
                status,
                exit_code,
                output,
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
                    command,
                    status,
                    exit_code.map_or(String::new(), |code| format!(", exit {code}"))
                )?;
                if !succeeded {
                    self.render_content(output, dim, reset, Some(20))?;
                }
            }
            WorkLogBlock::FilesChanged { paths, .. } => {
                let detail = if paths.is_empty() {
                    "files".into()
                } else {
                    paths.join(", ")
                };
                writeln!(self.out, "{yellow}✎{reset} changed {detail}")?;
            }
            WorkLogBlock::ToolStarted { name, detail } => {
                writeln!(self.out, "{cyan}◆{reset} {name} {detail}")?
            }
            WorkLogBlock::ToolCompleted { name, status } => {
                writeln!(self.out, "{green}◇{reset} {name} {dim}[{status}]{reset}")?
            }
            WorkLogBlock::Plan { content } => {
                writeln!(self.out, "{bold}{yellow}Plan{reset}")?;
                self.render_content(content, "", "", None)?;
            }
            WorkLogBlock::AgentMessage { content } => {
                writeln!(self.out, "{bold}{cyan}╭─ Agent{reset}")?;
                self.render_content(content, "", "", None)?;
                writeln!(self.out, "{cyan}╰─{reset}")?;
            }
            WorkLogBlock::Usage { usage } => writeln!(
                self.out,
                "{dim}tokens: {} input ({} cached), {} output{reset}",
                usage.input_tokens, usage.cached_input_tokens, usage.output_tokens
            )?,
            WorkLogBlock::Error { message } => {
                writeln!(self.out, "{red}✗ agent error:{reset} {message}")?
            }
            WorkLogBlock::Transcript { content } => self.render_content(content, "", "", None)?,
        }
        self.out.flush()?;
        Ok(())
    }

    fn render_content(
        &mut self,
        content: &[ContentBlock],
        tone: &str,
        reset: &str,
        limit: Option<usize>,
    ) -> Result<()> {
        let mut shown = 0;
        for block in content {
            let text = match block {
                ContentBlock::Text { text } | ContentBlock::Code { text, .. } => text,
            };
            for line in text.lines() {
                if limit.is_some_and(|limit| shown >= limit) {
                    return Ok(());
                }
                let line = match block {
                    ContentBlock::Text { .. } => line.strip_prefix("# ").unwrap_or(line),
                    ContentBlock::Code { .. } => line,
                };
                writeln!(self.out, "  {tone}│{reset} {line}")?;
                shown += 1;
            }
        }
        Ok(())
    }
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
                text: "safe\x1b[31mred\x1b[0m\x1b]0;title\x07".into(),
            })
            .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("safered"));
        assert!(!output.contains('\x1b'));
        assert!(!output.contains("title"));
    }

    #[test]
    fn work_log_blocks_split_fenced_code_and_retain_its_language() {
        let blocks = event_blocks(&AgentEvent::AgentMessage {
            text: "Before\n\n```rust\nlet answer = 42;\n```\n\nAfter".into(),
        });
        assert_eq!(
            blocks,
            vec![WorkLogBlock::AgentMessage {
                content: vec![
                    ContentBlock::Text {
                        text: "Before\n\n".into(),
                    },
                    ContentBlock::Code {
                        language: Some("rust".into()),
                        text: "let answer = 42;\n".into(),
                    },
                    ContentBlock::Text {
                        text: "\nAfter".into(),
                    },
                ],
            }]
        );

        let json = serde_json::to_value(&blocks).unwrap();
        assert_eq!(json[0]["type"], "agent_message");
        assert_eq!(json[0]["content"][1]["type"], "code");
        assert_eq!(json[0]["content"][1]["language"], "rust");
    }

    #[test]
    fn terminal_renderer_consumes_the_shared_work_log_format() {
        let mut output = Vec::new();
        RichRenderer::new(&mut output, false)
            .render_block(&WorkLogBlock::AgentMessage {
                content: vec![ContentBlock::Code {
                    language: Some("rust".into()),
                    text: "fn main() {}".into(),
                }],
            })
            .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "╭─ Agent\n  │ fn main() {}\n╰─\n"
        );
    }

    #[test]
    fn work_log_is_rendered_on_demand_from_the_raw_journal() {
        let raw = concat!(
            "{\"type\":\"item.started\",\"item\":{\"id\":\"c1\",\"type\":\"command_execution\",\"command\":\"cargo test\"}}\n",
            "\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"m1\",\"type\":\"agent_message\",\"text\":\"All tests pass\"}}\n"
        );

        // No file is written; interpretation is produced from the raw fact.
        let blocks = work_log_from_codex_raw(raw.as_bytes(), None).unwrap();
        assert!(matches!(
            blocks.as_slice(),
            [
                WorkLogBlock::CommandStarted { command },
                WorkLogBlock::AgentMessage { .. },
            ] if command == "cargo test"
        ));
    }

    #[test]
    fn the_protocol_selects_the_decoder() {
        // Plain output is its own transcript; the dispatcher wraps it without
        // interpreting it as an event stream.
        let plain = work_log_from_raw(AgentProtocol::Plain, b"just some stdout\n", None).unwrap();
        assert!(matches!(
            plain.as_slice(),
            [WorkLogBlock::Transcript { .. }]
        ));

        // The same bytes routed through the Codex decoder would instead be
        // parsed as jsonl — here a real Codex line decodes to a message.
        let codex = work_log_from_raw(
            AgentProtocol::CodexJsonl,
            br#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"hi"}}"#,
            None,
        )
        .unwrap();
        assert!(matches!(
            codex.as_slice(),
            [WorkLogBlock::AgentMessage { .. }]
        ));
    }

    #[test]
    fn rendering_folds_in_file_change_checkpoint_commits() {
        let raw = "{\"type\":\"item.completed\",\"item\":{\"id\":\"f1\",\"type\":\"file_change\",\"changes\":[{\"path\":\"src/lib.rs\"}]}}\n";
        let checkpoints =
            "{\"schema\":1,\"sequence\":1,\"event_id\":\"f1\",\"paths\":[\"src/lib.rs\"],\"commit\":\"abc123\"}\n";

        let blocks =
            work_log_from_codex_raw(raw.as_bytes(), Some(checkpoints.as_bytes())).unwrap();
        assert!(matches!(
            blocks.as_slice(),
            [WorkLogBlock::FilesChanged { checkpoint: Some(commit), .. }] if commit == "abc123"
        ));

        // Without the checkpoint journal the same fact still renders, only
        // without the commit annotation.
        let bare = work_log_from_codex_raw(raw.as_bytes(), None).unwrap();
        assert!(matches!(
            bare.as_slice(),
            [WorkLogBlock::FilesChanged { checkpoint: None, .. }]
        ));
    }
}
