//! Decoder tests against saved *actual* codex output.
//!
//! The fixtures are captured verbatim from real codex-cli 0.145 runs during
//! development, so the decoders are exercised against the true wire format
//! rather than hand-written approximations. If codex changes its output, these
//! fail and the recorded protocol version needs a new decoder.

use styra::event::{decode_line, Protocol, AgentEvent};

/// A real `codex exec --json` run that executed a shell command and answered.
const EXEC_COMMAND: &str = include_str!("fixtures/codex_exec_command.jsonl");

#[test]
fn real_codex_exec_output_decodes_to_the_expected_event_sequence() {
    let events: Vec<AgentEvent> = EXEC_COMMAND
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| decode_line(Protocol::CodexJsonl, line))
        .collect();

    // Nothing in real output should fail to decode.
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Malformed { .. } | AgentEvent::Unknown { .. })),
        "real codex output produced an undecoded event: {events:#?}"
    );

    // The recorded run: thread start, turn start, a command (started then
    // completed), the final answer, and the closing usage.
    assert!(matches!(events[0], AgentEvent::ThreadStarted { .. }));
    assert_eq!(events[1], AgentEvent::TurnStarted);
    assert_eq!(
        events[2],
        AgentEvent::CommandStarted {
            command: "/usr/bin/bash -lc 'echo hello-from-codex'".into()
        }
    );
    assert_eq!(
        events[3],
        AgentEvent::CommandCompleted {
            command: "/usr/bin/bash -lc 'echo hello-from-codex'".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "hello-from-codex\n".into(),
        }
    );
    assert_eq!(events[4], AgentEvent::AgentMessage { text: "done".into() });
    match &events[5] {
        AgentEvent::TurnCompleted { usage } => {
            assert_eq!(usage.input_tokens, 26011);
            assert_eq!(usage.output_tokens, 42);
            assert_eq!(usage.cached_input_tokens, 22272);
        }
        other => panic!("expected final usage, got {other:?}"),
    }
    assert_eq!(events.len(), 6);
}
