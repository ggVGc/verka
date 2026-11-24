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
        parse(&converted, SessionFormat::Codex)
            .unwrap()
            .messages
            .len(),
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
