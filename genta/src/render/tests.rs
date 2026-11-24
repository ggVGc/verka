use super::*;
use crate::event::TurnOutcome;

#[test]
fn a_codex_log_renders_as_a_tagged_transcript() {
    let log = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}\n",
            "{\"type\":\"item.started\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test\",\"status\":\"completed\",\"exit_code\":0}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"All good.\\nTests pass.\"}}\n",
        );
    let text = render(log, Protocol::CodexJsonl, false, true);
    assert_eq!(
        text,
        "  thread session t-1\n\
             \x20  shell cargo test (running)\n\
             \x20  shell cargo test (completed, exit 0)\n\
             \x20  agent All good.\n\
             \x20        Tests pass.\n"
    );
}

#[test]
fn a_claude_log_renders_through_the_claude_decoder() {
    let log = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done.\"}]}}\n",
        );
    let text = render(log, Protocol::ClaudeJsonl, false, true);
    assert!(text.contains("session s-1"));
    assert!(text.contains("agent Done."));
}

#[test]
fn render_events_matches_rendering_the_equivalent_raw_log() {
    // A caller that already decoded events (e.g. a host replaying its own
    // journal format) must get exactly what render() would have produced
    // from the equivalent raw wire lines.
    let log = concat!(
        "{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"done\"}}\n",
    );
    let events = vec![
        AgentEvent::ThreadStarted {
            thread_id: "t-1".into(),
            model: None,
            effort: None,
        },
        AgentEvent::AgentMessage {
            text: "done".into(),
        },
    ];
    assert_eq!(
        render_events(&events, false, true),
        render(log, Protocol::CodexJsonl, false, true)
    );
}

#[test]
fn minor_events_are_skipped_unless_show_minor_is_requested() {
    let events = vec![
        AgentEvent::ThreadStarted {
            thread_id: "t-1".into(),
            model: None,
            effort: None,
        },
        AgentEvent::TurnStarted,
        AgentEvent::AgentMessage {
            text: "done".into(),
        },
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: Default::default(),
        },
    ];
    let hidden = render_events(&events, false, false);
    assert!(!hidden.contains("t-1"));
    assert!(!hidden.contains("turn started"));
    assert!(!hidden.contains("usage"));
    assert!(hidden.contains("agent done"));

    let shown = render_events(&events, false, true);
    assert!(shown.contains("t-1"));
    assert!(shown.contains("turn started"));
}

/// The end of a turn reads as a turn boundary that states what the turn
/// cost, and the mid-turn snapshots read as the running totals they are —
/// even for a provider (this one) that reports only the totals.
#[test]
fn a_turn_ends_with_a_line_of_its_own_stating_both_figures() {
    let log = concat!(
        r#"{"method":"turn/started","params":{}}"#,
        "\n",
        r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":100,"outputTokens":10}}}}"#,
        "\n",
        r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":180,"outputTokens":25}}}}"#,
        "\n",
        r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"t1","status":"completed"}}}"#,
        "\n",
    );
    assert_eq!(
        render(log, Protocol::CodexAppServer, false, true),
        "    turn turn started\n\
             \x20  usage thread total · in 100 · out 10 · cached 0\n\
             \x20  usage thread total · in 180 · out 25 · cached 0\n\
             \x20   turn turn complete · this turn in 180 · out 25 · cached 0 \
             (thread total in 180 · out 25 · cached 0)\n"
    );
}

#[test]
fn unknown_events_are_skipped_unless_all_is_requested() {
    let log = "{\"type\":\"future.event\"}\n";
    assert_eq!(render(log, Protocol::CodexJsonl, false, true), "");
    assert!(render(log, Protocol::CodexJsonl, true, true).contains("future.event"));
}
