use super::*;

/// A turn that failed is an ending that says why, so the work log gets
/// both halves of it: the failure, then what the attempt spent getting
/// there.
#[test]
fn a_failed_ending_logs_the_failure_and_the_spend() {
    let blocks = event_blocks(&AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Failed {
            message: "out of credits".into(),
        },
        usage: genta::event::TurnUsage {
            turn: Some(TokenUsage {
                input_tokens: 12,
                ..Default::default()
            }),
            total: None,
        },
    });
    assert!(matches!(
        blocks.as_slice(),
        [
            WorkLogBlock::Error { message },
            WorkLogBlock::Usage { usage }
        ] if message == "out of credits" && usage.input_tokens == 12
    ));

    // An ending that reported neither figure has nothing to log.
    assert!(event_blocks(&AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: genta::event::TurnUsage::default(),
    })
    .is_empty());
}

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
        AgentEvent::TurnCompleted { usage, .. } if matches!(&usage.turn, Some(turn) if turn.input_tokens == 12 && turn.output_tokens == 3)
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
                    text: "Before".into(),
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
    let plain = work_log_from_raw(OutputFormat::Plain, b"just some stdout\n", None).unwrap();
    assert!(matches!(
        plain.as_slice(),
        [WorkLogBlock::Transcript { .. }]
    ));

    // The same bytes routed through the Codex decoder would instead be
    // parsed as jsonl — here a real Codex line decodes to a message.
    let codex = work_log_from_raw(
        OutputFormat::Agent(genta::event::Protocol::CodexJsonl),
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

    let blocks = work_log_from_codex_raw(raw.as_bytes(), Some(checkpoints.as_bytes())).unwrap();
    assert!(matches!(
        blocks.as_slice(),
        [WorkLogBlock::FilesChanged { checkpoint: Some(commit), .. }] if commit == "abc123"
    ));

    // Without the checkpoint journal the same fact still renders, only
    // without the commit annotation.
    let bare = work_log_from_codex_raw(raw.as_bytes(), None).unwrap();
    assert!(matches!(
        bare.as_slice(),
        [WorkLogBlock::FilesChanged {
            checkpoint: None,
            ..
        }]
    ));
}
