//! Native Claude Code and Codex CLI session conversion.
//!
//! This deliberately works on their persisted JSONL transcripts, not the live
//! `stream-json`/`--json` protocols.  Codex rollouts retain their metadata and
//! model-visible response items; Claude sessions retain their resumable message
//! records. Provider-specific tool calls are represented as readable notes,
//! exactly as Codex's own Claude session importer does.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;
use uuid::Uuid;

/// The on-disk session format to read or write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionFormat {
    Codex,
    Claude,
}

/// Portable, model-visible session history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub timestamp: String,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub timestamp: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// Overrides for the newly-created destination session.  Omitted fields carry
/// source metadata across; `id` defaults to a fresh UUID so source and target
/// can coexist in their native session directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConversionOptions {
    pub id: Option<String>,
    pub cwd: Option<String>,
    pub timestamp: Option<String>,
    /// Keep only the first `keep_messages` messages of the source session,
    /// dropping the rest. `None` keeps the whole history. This is what turns
    /// a conversion into a *branch*: a destination session seeded with a
    /// prefix of the source's history rather than all of it, whether or not
    /// the format also changes. A full conversion is the special case where
    /// this is `None` — a branch at the very end of the history.
    pub keep_messages: Option<usize>,
}

/// Convert a native JSONL session into the other provider's native JSONL
/// format, optionally truncated to a prefix of its history (see
/// [`ConversionOptions::keep_messages`]). The result always ends in a newline
/// and is ready to write.
pub fn convert(
    input: &str,
    from: SessionFormat,
    to: SessionFormat,
    options: &ConversionOptions,
) -> Result<String> {
    let mut session = parse(input, from)?;
    if let Some(keep) = options.keep_messages {
        session.messages.truncate(keep);
    }
    if let Some(id) = &options.id {
        session.id = id.clone();
    } else if from != to {
        session.id = Uuid::new_v4().to_string();
    }
    if let Some(cwd) = &options.cwd {
        session.cwd = cwd.clone();
    }
    if let Some(timestamp) = &options.timestamp {
        session.timestamp = timestamp.clone();
    }
    Ok(serialize(&session, to))
}

pub fn parse(input: &str, format: SessionFormat) -> Result<Session> {
    match format {
        SessionFormat::Claude => parse_claude(input),
        SessionFormat::Codex => parse_codex(input),
    }
}

pub fn serialize(session: &Session, format: SessionFormat) -> String {
    match format {
        SessionFormat::Claude => serialize_claude(session),
        SessionFormat::Codex => serialize_codex(session),
    }
}

fn jsonl(input: &str) -> Result<Vec<Value>> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line)
                .with_context(|| format!("invalid JSONL record {}", index + 1))
        })
        .collect()
}

fn parse_claude(input: &str) -> Result<Session> {
    let records = jsonl(input)?;
    let mut id = None;
    let mut cwd = None;
    let mut timestamp = None;
    let mut messages = Vec::new();
    for record in records {
        id = id.or_else(|| string(&record, "sessionId").or_else(|| string(&record, "session_id")));
        cwd = cwd.or_else(|| string(&record, "cwd"));
        timestamp = timestamp.or_else(|| string(&record, "timestamp"));
        let Some(kind) = string(&record, "type") else {
            continue;
        };
        if record.get("isMeta").and_then(Value::as_bool) == Some(true)
            || record.get("isSidechain").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let role = match kind.as_str() {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => continue,
        };
        let Some(text) = claude_content(record.pointer("/message/content")) else {
            continue;
        };
        messages.push(Message {
            role,
            text: if role == Role::User {
                unwrap_user_query(text)
            } else {
                text
            },
            timestamp: string(&record, "timestamp"),
        });
    }
    Ok(Session {
        id: id.context("Claude session has no sessionId")?,
        cwd: cwd.context("Claude session has no cwd")?,
        timestamp: timestamp.context("Claude session has no timestamp")?,
        messages,
    })
}

fn claude_content(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = content.as_str().filter(|text| !text.is_empty()) {
        return Some(text.to_owned());
    }
    let mut parts = Vec::new();
    for block in content.as_array()? {
        match string(block, "type").as_deref() {
            Some("text") => {
                if let Some(text) = string(block, "text") {
                    parts.push(text);
                }
            }
            Some("tool_use") => parts.push(tool_call_note(block)),
            Some("tool_result") => parts.push(tool_result_note(block)),
            Some("thinking") | Some("redacted_thinking") => {}
            Some(other) => parts.push(format!("[external unsupported block: {other}]")),
            None => {}
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn unwrap_user_query(text: String) -> String {
    let trimmed = text.trim();
    trimmed
        .strip_prefix("<user_query>")
        .and_then(|inner| inner.strip_suffix("</user_query>"))
        .map(str::trim)
        .filter(|inner| !inner.is_empty())
        .map(str::to_owned)
        .unwrap_or(text)
}

fn parse_codex(input: &str) -> Result<Session> {
    let records = jsonl(input)?;
    let meta = records
        .iter()
        .find(|record| string(record, "type").as_deref() == Some("session_meta"))
        .context("Codex rollout has no session_meta record")?;
    let payload = meta
        .get("payload")
        .context("Codex session_meta has no payload")?;
    let id = string(payload, "id")
        .or_else(|| string(payload, "session_id"))
        .context("Codex session_meta has no id")?;
    let cwd = string(payload, "cwd").context("Codex session_meta has no cwd")?;
    let timestamp = string(payload, "timestamp")
        .or_else(|| string(meta, "timestamp"))
        .context("Codex session_meta has no timestamp")?;
    let mut messages = Vec::new();
    for record in records {
        if string(&record, "type").as_deref() != Some("response_item") {
            continue;
        }
        let Some(item) = record.get("payload") else {
            continue;
        };
        if string(item, "type").as_deref() != Some("message") {
            continue;
        }
        let role = match string(item, "role").as_deref() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => continue,
        };
        let Some(text) = codex_content(item.get("content")) else {
            continue;
        };
        messages.push(Message {
            role,
            text,
            timestamp: string(&record, "timestamp"),
        });
    }
    Ok(Session {
        id,
        cwd,
        timestamp,
        messages,
    })
}

fn codex_content(content: Option<&Value>) -> Option<String> {
    let mut parts = Vec::new();
    for block in content?.as_array()? {
        match string(block, "type").as_deref() {
            Some("input_text") | Some("output_text") => {
                if let Some(text) = string(block, "text") {
                    parts.push(text);
                }
            }
            Some("refusal") => {
                if let Some(text) = string(block, "refusal") {
                    parts.push(text);
                }
            }
            Some(kind) => parts.push(format!("[external unsupported Codex content: {kind}]")),
            None => {}
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn serialize_codex(session: &Session) -> String {
    let mut records = vec![json!({
        "timestamp": session.timestamp, "type": "session_meta", "payload": {
            "session_id": session.id, "id": session.id, "timestamp": session.timestamp,
            "cwd": session.cwd, "originator": "genta", "cli_version": env!("CARGO_PKG_VERSION"),
            "source": "cli", "model_provider": "openai", "history_mode": "legacy"
        }
    })];
    let mut turn = 0usize;
    for message in &session.messages {
        let timestamp = message.timestamp.as_deref().unwrap_or(&session.timestamp);
        if message.role == Role::User {
            turn += 1;
            records.push(json!({ "timestamp": timestamp, "type": "event_msg", "payload": { "type": "turn_started", "turn_id": format!("genta-import-turn-{turn}") } }));
            records.push(json!({ "timestamp": timestamp, "type": "event_msg", "payload": { "type": "user_message", "message": message.text, "kind": "plain" } }));
        } else {
            records.push(json!({ "timestamp": timestamp, "type": "event_msg", "payload": { "type": "agent_message", "message": message.text } }));
        }
        records.push(json!({ "timestamp": timestamp, "type": "response_item", "payload": {
            "type": "message", "role": match message.role { Role::User => "user", Role::Assistant => "assistant" },
            "content": [{ "type": match message.role { Role::User => "input_text", Role::Assistant => "output_text" }, "text": message.text }]
        }}));
    }
    if turn > 0 {
        records.push(json!({ "timestamp": session.timestamp, "type": "event_msg", "payload": { "type": "turn_complete", "turn_id": format!("genta-import-turn-{turn}"), "last_agent_message": Value::Null, "error": Value::Null } }));
    }
    to_jsonl(records)
}

fn serialize_claude(session: &Session) -> String {
    let mut records = Vec::new();
    let mut parent: Option<String> = None;
    for message in &session.messages {
        let uuid = Uuid::new_v4().to_string();
        let timestamp = message.timestamp.as_deref().unwrap_or(&session.timestamp);
        let mut record = json!({
            "type": match message.role { Role::User => "user", Role::Assistant => "assistant" },
            "uuid": uuid, "parentUuid": parent, "isSidechain": false, "cwd": session.cwd,
            "sessionId": session.id, "version": "genta", "timestamp": timestamp,
            "message": { "role": match message.role { Role::User => "user", Role::Assistant => "assistant" }, "content": [{ "type": "text", "text": message.text }] }
        });
        if message.role == Role::User {
            record["userType"] = json!("external");
        }
        parent = record
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned);
        records.push(record);
    }
    to_jsonl(records)
}

fn tool_call_note(block: &Value) -> String {
    format!(
        "[external_agent_tool_call: {}]\ninput: {}\n[/external_agent_tool_call]",
        string(block, "name").unwrap_or_else(|| "unknown".into()),
        block.get("input").unwrap_or(&Value::Null)
    )
}
fn tool_result_note(block: &Value) -> String {
    format!(
        "[external_agent_tool_result{}]\n{}\n[/external_agent_tool_result]",
        if block.get("is_error").and_then(Value::as_bool) == Some(true) {
            ": error"
        } else {
            ""
        },
        tool_result_text(block.get("content"))
    )
}

fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| string(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn to_jsonl(records: Vec<Value>) -> String {
    records
        .into_iter()
        .map(|record| record.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Read, convert, and write a session file. Kept separate from [`convert`] so
/// callers that manage their own storage can stay fully in-memory.
pub fn convert_file(
    input: &Path,
    output: &Path,
    from: SessionFormat,
    to: SessionFormat,
    options: &ConversionOptions,
) -> Result<()> {
    let source =
        std::fs::read_to_string(input).with_context(|| format!("reading {}", input.display()))?;
    std::fs::write(output, convert(&source, from, to, options)?)
        .with_context(|| format!("writing {}", output.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = r#"{"type":"user","uuid":"u1","parentUuid":null,"isSidechain":false,"cwd":"/repo","sessionId":"claude-id","timestamp":"2026-08-21T10:00:00.000Z","message":{"role":"user","content":"<user_query>Fix it</user_query>"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":false,"cwd":"/repo","sessionId":"claude-id","timestamp":"2026-08-21T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"I will fix it."},{"type":"tool_use","id":"tool1","name":"Bash","input":{"command":"cargo test"}}]}}
{"type":"user","uuid":"r1","parentUuid":"a1","isSidechain":false,"cwd":"/repo","sessionId":"claude-id","timestamp":"2026-08-21T10:00:02.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool1","content":"ok"}]}}
"#;

    #[test]
    fn claude_to_codex_writes_the_native_rollout_shape_and_all_messages() {
        let output = convert(
            CLAUDE,
            SessionFormat::Claude,
            SessionFormat::Codex,
            &ConversionOptions {
                id: Some("codex-id".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let records = jsonl(&output).unwrap();
        assert_eq!(records[0]["type"], "session_meta");
        assert_eq!(records[0]["payload"]["id"], "codex-id");
        assert_eq!(records[0]["payload"]["session_id"], "codex-id");
        assert_eq!(
            records
                .iter()
                .filter(|r| r["type"] == "response_item")
                .count(),
            3
        );
        let parsed = parse(&output, SessionFormat::Codex).unwrap();
        assert_eq!(parsed.cwd, "/repo");
        assert_eq!(parsed.messages.len(), 3);
        assert_eq!(parsed.messages[0].text, "Fix it");
        assert!(parsed.messages[1].text.contains("I will fix it."));
        assert!(parsed.messages[1].text.contains("external_agent_tool_call"));
        assert!(parsed.messages[2]
            .text
            .contains("external_agent_tool_result"));
    }

    #[test]
    fn codex_to_claude_writes_a_resumable_parent_chain() {
        let codex = convert(
            CLAUDE,
            SessionFormat::Claude,
            SessionFormat::Codex,
            &ConversionOptions {
                id: Some("codex-id".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let output = convert(
            &codex,
            SessionFormat::Codex,
            SessionFormat::Claude,
            &ConversionOptions {
                id: Some("claude-id".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let records = jsonl(&output).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["sessionId"], "claude-id");
        assert_eq!(records[0]["parentUuid"], Value::Null);
        assert_eq!(records[1]["parentUuid"], records[0]["uuid"]);
        assert_eq!(records[2]["parentUuid"], records[1]["uuid"]);
        assert_eq!(records[0]["message"]["content"][0]["type"], "text");
        assert_eq!(
            parse(&output, SessionFormat::Claude).unwrap().messages,
            parse(&codex, SessionFormat::Codex).unwrap().messages
        );
    }

    #[test]
    fn keep_messages_branches_a_prefix_of_the_history() {
        let output = convert(
            CLAUDE,
            SessionFormat::Claude,
            SessionFormat::Claude,
            &ConversionOptions {
                id: Some("branch-id".into()),
                keep_messages: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let parsed = parse(&output, SessionFormat::Claude).unwrap();
        assert_eq!(parsed.messages.len(), 1);
        assert_eq!(parsed.messages[0].text, "Fix it");

        // A branch can also change format in the same step.
        let converted = convert(
            CLAUDE,
            SessionFormat::Claude,
            SessionFormat::Codex,
            &ConversionOptions {
                id: Some("branch-id-2".into()),
                keep_messages: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            parse(&converted, SessionFormat::Codex).unwrap().messages.len(),
            2
        );
    }

    #[test]
    fn ignores_claude_sidechains_and_rejects_missing_codex_metadata() {
        let sidechain = r#"{"type":"assistant","isSidechain":true,"cwd":"/repo","sessionId":"id","timestamp":"t","message":{"content":"hidden"}}"#;
        assert!(parse(sidechain, SessionFormat::Claude)
            .unwrap()
            .messages
            .is_empty());
        assert!(parse(
            "{\"type\":\"response_item\",\"payload\":{}}\n",
            SessionFormat::Codex
        )
        .is_err());
    }

    #[test]
    fn codex_writer_covers_metadata_lifecycle_visible_events_and_response_items() {
        let session = Session {
            id: "00000000-0000-4000-8000-000000000001".into(),
            cwd: "/repo".into(),
            timestamp: "2026-08-21T10:00:00.000Z".into(),
            messages: vec![
                Message {
                    role: Role::User,
                    text: "first".into(),
                    timestamp: None,
                },
                Message {
                    role: Role::Assistant,
                    text: "answer".into(),
                    timestamp: Some("2026-08-21T10:00:01.000Z".into()),
                },
                Message {
                    role: Role::User,
                    text: "second".into(),
                    timestamp: None,
                },
            ],
        };
        let records = jsonl(&serialize(&session, SessionFormat::Codex)).unwrap();
        assert_eq!(records[0]["payload"]["cwd"], "/repo");
        assert_eq!(records[0]["payload"]["history_mode"], "legacy");
        assert_eq!(records[1]["payload"]["type"], "turn_started");
        assert_eq!(
            records[2]["payload"],
            json!({"type":"user_message","message":"first","kind":"plain"})
        );
        assert_eq!(records[3]["payload"]["role"], "user");
        assert_eq!(
            records[3]["payload"]["content"][0],
            json!({"type":"input_text","text":"first"})
        );
        assert_eq!(records[4]["payload"]["type"], "agent_message");
        assert_eq!(
            records[5]["payload"]["content"][0],
            json!({"type":"output_text","text":"answer"})
        );
        assert_eq!(records[6]["payload"]["turn_id"], "genta-import-turn-2");
        assert_eq!(records[9]["payload"]["type"], "turn_complete");
    }

    #[test]
    fn claude_writer_covers_required_native_fields_and_preserves_timestamps() {
        let session = Session {
            id: "claude-session".into(),
            cwd: "/repo".into(),
            timestamp: "2026-08-21T10:00:00.000Z".into(),
            messages: vec![
                Message {
                    role: Role::User,
                    text: "question".into(),
                    timestamp: None,
                },
                Message {
                    role: Role::Assistant,
                    text: "answer".into(),
                    timestamp: Some("2026-08-21T10:00:01.000Z".into()),
                },
            ],
        };
        let records = jsonl(&serialize(&session, SessionFormat::Claude)).unwrap();
        assert_eq!(records[0]["type"], "user");
        assert_eq!(records[0]["userType"], "external");
        assert_eq!(records[0]["isSidechain"], false);
        assert_eq!(records[0]["cwd"], "/repo");
        assert_eq!(records[0]["sessionId"], "claude-session");
        assert_eq!(
            records[0]["message"],
            json!({"role":"user","content":[{"type":"text","text":"question"}]})
        );
        assert_eq!(records[1]["type"], "assistant");
        assert_eq!(records[1]["timestamp"], "2026-08-21T10:00:01.000Z");
        assert_eq!(records[1]["parentUuid"], records[0]["uuid"]);
    }
}
