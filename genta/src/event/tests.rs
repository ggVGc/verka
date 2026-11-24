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
    assert_eq!(
        started,
        AgentEvent::CommandStarted {
            command: "cargo test".into()
        }
    );

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
        decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"thread.started","thread_id":"t-7"}"#
        ),
        AgentEvent::ThreadStarted {
            thread_id: "t-7".into(),
            model: None,
            effort: None
        }
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
            outcome: TurnOutcome::Completed,
            usage: TurnUsage {
                turn: Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 3,
                    cached_input_tokens: 2,
                    reasoning_output_tokens: 0,
                }),
                total: None,
            }
        }
    );
    assert_eq!(
        usage.summary(),
        "turn complete · this turn in 10 · out 3 · cached 2"
    );
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
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        }
    );
    assert_eq!(event.summary(), "src/a.rs, src/b.rs");
}

#[test]
fn appserver_aggregated_diff_is_renderable_without_git() {
    let event = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"turn/diff/updated","params":{"threadId":"t","turnId":"u","diff":"diff --git a/src/a.rs b/src/a.rs\n+++ b/src/a.rs\n@@\n-old\n+new\n"}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::DiffUpdated {
            diff: "diff --git a/src/a.rs b/src/a.rs\n+++ b/src/a.rs\n@@\n-old\n+new\n".into()
        }
    );
    assert_eq!(event.summary(), "src/a.rs");
    assert!(matches!(event.detail()[0], DetailBlock::Code { .. }));
}

#[test]
fn appserver_turn_plan_update_preserves_explanation_steps_and_statuses() {
    let event = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"turn/plan/updated","params":{"turnId":"u","explanation":"Adjusted after inspection.","plan":[{"step":"Read the code","status":"completed"},{"step":"Implement the fix","status":"inProgress"},{"step":"Run tests","status":"pending"}]}}"#,
    );
    assert_eq!(
            event,
            AgentEvent::PlanUpdated {
                text: "Adjusted after inspection.\n\n- [completed] Read the code\n- [inProgress] Implement the fix\n- [pending] Run tests".into()
            }
        );
    assert_eq!(event.summary(), "Adjusted after inspection.");
}

#[test]
fn appserver_turn_plan_update_without_explanation_uses_first_step_as_summary() {
    let event = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"turn/plan/updated","params":{"turnId":"u","plan":[{"step":"Inspect\u001b[31m code","status":"inProgress"}]}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::PlanUpdated {
            text: "- [inProgress] Inspect code".into()
        }
    );
    assert_eq!(event.summary(), "- [inProgress] Inspect code");
}

#[test]
fn unknown_and_malformed_are_preserved_not_dropped() {
    assert_eq!(
        decode_line(Protocol::CodexJsonl, r#"{"type":"future.event"}"#),
        AgentEvent::Unknown {
            wire_type: "future.event".into()
        }
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
    assert_eq!(
        event,
        AgentEvent::AgentMessage {
            text: "red done".into()
        }
    );
}

#[test]
fn terminal_tabs_are_expanded_before_rendering() {
    let event = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"turn/diff/updated","params":{"diff":"@@\n-\told\n+\tnew"}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::DiffUpdated {
            diff: "@@\n-    old\n+    new".into()
        }
    );
    assert!(!event.detail().iter().any(|block| match block {
        DetailBlock::Text(text) | DetailBlock::Code { text, .. } => text.contains('\t'),
    }));
}

#[test]
fn markdown_detail_separates_prose_and_code() {
    let blocks = markdown_blocks("before\n```rust\nfn main() {}\n```\nafter");
    assert_eq!(
        blocks,
        vec![
            DetailBlock::Text("before".into()),
            DetailBlock::Code {
                language: Some("rust".into()),
                text: "fn main() {}\n".into()
            },
            DetailBlock::Text("after".into()),
        ]
    );
}

#[test]
fn agent_message_detail_uses_markdown_blocks() {
    let event = AgentEvent::AgentMessage {
        text: "text\n```\ncode\n```".into(),
    };
    assert_eq!(
        event.detail(),
        vec![
            DetailBlock::Text("text".into()),
            DetailBlock::Code {
                language: None,
                text: "code\n".into()
            },
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
        AgentEvent::ThreadStarted {
            thread_id: "s-9".into(),
            model: Some("claude-opus-4-8".into()),
            effort: None,
        }
    );
    let event = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"compact_boundary"}"#,
    );
    assert_eq!(
        event,
        AgentEvent::Unknown {
            wire_type: "system:compact_boundary".into()
        }
    );
    assert!(event.is_minor());
}

#[test]
fn claude_background_task_set_carries_the_running_count() {
    let started = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bk8","task_type":"local_bash","description":"Compile project with sbt"}]}"#,
    );
    assert_eq!(started, AgentEvent::BackgroundTasks { running: 1 });
    assert_eq!(started.background_tasks_running(), Some(1));
    assert!(started.is_minor());
    assert_eq!(started.summary(), "1 background task running");

    let drained = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#,
    );
    assert_eq!(drained, AgentEvent::BackgroundTasks { running: 0 });
    assert_eq!(drained.background_tasks_running(), Some(0));
    assert_eq!(drained.summary(), "no background tasks running");
}

#[test]
fn claude_bash_result_wording_is_not_trusted_to_end_a_background_task() {
    // Claude's own completion notice reads "[exited with code 0]", which
    // none of the heuristic phrases match. The reported task set is what
    // clients must rely on.
    let completed = AgentEvent::ToolCompleted {
        id: "t-1".into(),
        name: "t-1".into(),
        detail: String::new(),
        status: "completed".into(),
        output: "[exited with code 0]".into(),
    };
    assert!(!completed.finishes_background_task());
    assert_eq!(completed.background_tasks_running(), None);
}

#[test]
fn claude_task_lifecycle_decodes_to_one_task_id() {
    let started = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"task_started","task_id":"t-1","tool_use_id":"toolu_1","description":"Rebuild and run tests","task_type":"local_bash"}"#,
    );
    assert_eq!(
        started,
        AgentEvent::TaskStarted {
            id: "t-1".into(),
            description: "Rebuild and run tests".into(),
            kind: "local_bash".into(),
            agent: None,
        }
    );
    let progress = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"task_progress","task_id":"t-1","description":"Running cargo test","subagent_type":"Explore","last_tool_name":"Bash","usage":{"total_tokens":10469}}"#,
    );
    assert_eq!(
        progress,
        AgentEvent::TaskProgress {
            id: "t-1".into(),
            description: "Running cargo test".into(),
            agent: Some("Explore".into()),
            tool: Some("Bash".into()),
            tokens: Some(10469),
        }
    );
    let ended = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"task_notification","task_id":"t-1","status":"completed","summary":"Rebuild and run tests","output_file":""}"#,
    );
    assert_eq!(
        ended,
        AgentEvent::TaskCompleted {
            id: "t-1".into(),
            status: "completed".into(),
            summary: "Rebuild and run tests".into(),
            error: None,
        }
    );
    assert!([&started, &progress, &ended]
        .iter()
        .all(|event| event.task_id() == Some("t-1")));
}

#[test]
fn claude_task_update_ends_a_task_only_when_it_patches_the_status() {
    assert_eq!(
        decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"task_updated","task_id":"t-2","patch":{"status":"failed","end_time":1,"error":"terminated early"}}"#,
        ),
        AgentEvent::TaskCompleted {
            id: "t-2".into(),
            status: "failed".into(),
            summary: String::new(),
            error: Some("terminated early".into()),
        }
    );
    // Backgrounding a running command changes nothing the task's own row
    // does not already show.
    assert_eq!(
        decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"task_updated","task_id":"t-2","patch":{"is_backgrounded":true}}"#,
        ),
        AgentEvent::Unknown {
            wire_type: "system:task_updated".into()
        }
    );
}

#[test]
fn claude_system_thinking_tokens_carries_what_this_update_spent() {
    // The delta, not the count beside it: that count restarts at each
    // block of reasoning, so only the deltas add up across a turn.
    let event = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":150,"estimated_tokens_delta":100}"#,
    );
    assert_eq!(
        event,
        AgentEvent::Thinking {
            text: String::new(),
            tokens: Some(100),
        }
    );
    assert!(event.is_minor());
    assert!(event.updates_thinking());
    assert_eq!(event.summary(), "thinking · 100 tokens");

    // A release that reports only a count is read as that block's spend.
    assert_eq!(
        decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"thinking_tokens","thinking_tokens":1280}"#,
        ),
        AgentEvent::Thinking {
            text: String::new(),
            tokens: Some(1280),
        }
    );
}

#[test]
fn claude_system_thinking_tokens_is_minor() {
    let event = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"system","subtype":"thinking_tokens"}"#,
    );
    assert_eq!(
        event,
        AgentEvent::Unknown {
            wire_type: "system:thinking_tokens".into()
        }
    );
    assert!(event.is_minor());
}

#[test]
fn claude_assistant_text_decodes_to_an_agent_message() {
    let event = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Added backoff.\nTests pass."}]},"session_id":"s"}"#,
    );
    assert_eq!(
        event,
        AgentEvent::AgentMessage {
            text: "Added backoff.\nTests pass.".into()
        }
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
        AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into()
        }
    );
    assert_eq!(event.summary(), "Bash: {\"command\":\"cargo test\"}");
}

#[test]
fn claude_background_bash_is_detected_until_task_poll_finishes() {
    let started = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":"cargo test","run_in_background":true}}]}}"#,
    );
    assert!(started.starts_background_task());
    assert!(!started.finishes_background_task());

    let completed = AgentEvent::ToolCompleted {
        id: "poll-1".into(),
        name: "poll-1".into(),
        detail: String::new(),
        status: "completed".into(),
        output: "Task completed successfully".into(),
    };
    assert!(completed.finishes_background_task());
}

#[test]
fn claude_edit_captures_a_best_effort_change_without_git() {
    let event = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"old line","new_string":"new line"}}]}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::FileChanged {
            id: "tool-1".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ edit @@\n-old line\n+new line".into()),
            checkpoint: None,
            checkpoint_error: None,
        }
    );
}

#[test]
fn claude_thinking_only_message_falls_back_to_a_minor_thinking_event() {
    let event = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"weigh the options"}]}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::Thinking {
            text: "weigh the options".into(),
            tokens: None,
        }
    );
    assert!(event.is_minor());
}

#[test]
fn claude_rate_limit_event_is_minor() {
    let event = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"rate_limit_event","rate_limit":{"status":"ok"}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::Unknown {
            wire_type: "rate_limit_event".into()
        }
    );
    assert!(event.is_minor());
}

#[test]
fn claude_tool_result_completes_a_tool_with_its_status() {
    assert_eq!(
        decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#,
        ),
        AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "ok".into()
        }
    );
    assert_eq!(
        decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","is_error":true,"content":"boom"}]}}"#,
        ),
        AgentEvent::ToolCompleted {
            id: "toolu_2".into(),
            name: "toolu_2".into(),
            detail: String::new(),
            status: "error".into(),
            output: "boom".into()
        }
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
            outcome: TurnOutcome::Completed,
            usage: TurnUsage {
                turn: Some(TokenUsage {
                    input_tokens: 20,
                    cached_input_tokens: 8,
                    output_tokens: 3,
                    reasoning_output_tokens: 0,
                }),
                total: None,
            }
        }
    );
    assert_eq!(
        usage.summary(),
        "turn complete · this turn in 20 · out 3 · cached 8"
    );

    // A failed `result` is still the turn's ending, and states why.
    let failed = decode_line(
        Protocol::ClaudeJsonl,
        r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"hit the turn limit"}"#,
    );
    assert_eq!(
        failed,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed {
                message: "hit the turn limit".into()
            },
            usage: TurnUsage::default(),
        }
    );
    assert_eq!(failed.summary(), "turn failed: hit the turn limit");
    // Bookkeeping the operator can hide, a failure is not.
    assert!(!failed.is_minor());
}

#[test]
fn claude_input_includes_cache_creation_and_reads_across_turns() {
    let mut tracker = UsageTracker::default();
    for (raw, expected_turn, expected_total) in [
        (
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":46,"cache_creation_input_tokens":71924,"cache_read_input_tokens":1293031,"output_tokens":19912}}"#,
            TokenUsage {
                input_tokens: 1_365_001,
                cached_input_tokens: 1_293_031,
                output_tokens: 19_912,
                ..Default::default()
            },
            TokenUsage {
                input_tokens: 1_365_001,
                cached_input_tokens: 1_293_031,
                output_tokens: 19_912,
                ..Default::default()
            },
        ),
        (
            r#"{"type":"result","subtype":"success","usage":{"input_tokens":4,"cache_creation_input_tokens":100,"cache_read_input_tokens":2000,"output_tokens":50}}"#,
            TokenUsage {
                input_tokens: 2_104,
                cached_input_tokens: 2_000,
                output_tokens: 50,
                ..Default::default()
            },
            TokenUsage {
                input_tokens: 1_367_105,
                cached_input_tokens: 1_295_031,
                output_tokens: 19_962,
                ..Default::default()
            },
        ),
    ] {
        let mut event = decode_line(Protocol::ClaudeJsonl, raw);
        tracker.observe(&mut event);
        let AgentEvent::TurnCompleted { usage, .. } = event else {
            panic!("expected a turn ending");
        };
        assert_eq!(usage.turn, Some(expected_turn));
        assert_eq!(usage.total, Some(expected_total));
    }
}

#[test]
fn nested_error_prose_survives_decoding() {
    // The app-server states some failures only inside a nested `error`
    // object; the prose there is what the operator needs to see.
    assert_eq!(
        decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"error","params":{"error":{"code":"out_of_credits","message":"This workspace is out of credits."}}}"#,
        ),
        AgentEvent::Error {
            message: "This workspace is out of credits.".into()
        }
    );

    // `turn.failed` ends the turn; the prose comes through as the
    // ending's own account of why.
    assert_eq!(
        decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"turn.failed","error":{"error":{"message":"You have run out of credits."}}}"#,
        ),
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed {
                message: "You have run out of credits.".into()
            },
            usage: TurnUsage::default(),
        }
    );
}

#[test]
fn an_error_without_prose_names_itself() {
    assert_eq!(
        decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#,
        ),
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed {
                message: "agent reported an error (error_during_execution)".into()
            },
            usage: TurnUsage::default(),
        }
    );
}

#[test]
fn a_model_change_says_which_settings_moved() {
    let changed = |model: Option<&str>, effort: Option<&str>| AgentEvent::ModelChanged {
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
    };

    assert_eq!(
        changed(Some("gpt-5"), Some("high")).summary(),
        "model → gpt-5 · effort → high"
    );
    // The operator switched model only; the line has to say the effort
    // stayed as it was rather than leaving it unmentioned.
    assert_eq!(
        changed(Some("gpt-5"), None).summary(),
        "model → gpt-5 (same effort)"
    );
    assert_eq!(
        changed(None, Some("low")).summary(),
        "effort → low (same model)"
    );
    assert_eq!(
        changed(Some("gpt-5"), None).detail(),
        vec![DetailBlock::Text(
            "model: gpt-5\nreasoning effort: unchanged".into()
        )]
    );
}

#[test]
fn claude_unknown_and_malformed_are_preserved() {
    assert_eq!(
        decode_line(Protocol::ClaudeJsonl, r#"{"type":"stream_event"}"#),
        AgentEvent::Unknown {
            wire_type: "stream_event".into()
        }
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

#[test]
fn pretty_claude_bash_presents_only_the_command() {
    let event = AgentEvent::ToolStarted {
        id: "toolu_1".into(),
        name: "Bash".into(),
        detail: r#"{"command":"cargo test --all","description":"run tests"}"#.into(),
    };
    assert_eq!(
        Protocol::ClaudeJsonl.presented_summary(&event, PresentationMode::Pretty),
        "cargo test --all"
    );
    assert_eq!(
        Protocol::ClaudeJsonl.presented_detail(&event, PresentationMode::Pretty),
        vec![DetailBlock::Code {
            language: Some("bash".into()),
            text: "cargo test --all".into(),
        }]
    );
    assert!(Protocol::ClaudeJsonl
        .presented_detail(&event, PresentationMode::Raw)
        .iter()
        .any(|block| matches!(block, DetailBlock::Text(text) if text.contains("description"))));
}

#[test]
fn pretty_diffs_are_provider_independent_and_changed_lines_only() {
    let event = AgentEvent::DiffUpdated {
        diff: "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new".into(),
    };
    assert_eq!(
        Protocol::CodexAppServer.presented_detail(&event, PresentationMode::Pretty),
        vec![DetailBlock::Code {
            language: Some("diff".into()),
            text: "-old\n+new".into(),
        }]
    );
}

// The following lines are copied verbatim from a live `codex app-server`
// session (codex-cli 0.145) captured during development.

#[test]
fn appserver_notifications_decode_from_real_output() {
    let d = |line| decode_line(Protocol::CodexAppServer, line);
    assert_eq!(
        d(
            r#"{"method":"thread/started","params":{"thread":{"id":"019f8f61-b7df-7291-81fc-04ff0bfb786f"}}}"#
        ),
        AgentEvent::ThreadStarted {
            thread_id: "019f8f61-b7df-7291-81fc-04ff0bfb786f".into(),
            model: None,
            effort: None,
        }
    );
    assert_eq!(
        d(r#"{"method":"turn/started","params":{"threadId":"t"}}"#),
        AgentEvent::TurnStarted
    );
    assert_eq!(
        d(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_1","text":"hello","phase":"final_answer"}}}"#
        ),
        AgentEvent::AgentMessage {
            text: "hello".into()
        }
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
        AgentEvent::CommandStarted {
            command: "/usr/bin/bash -lc 'echo hi'".into()
        }
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
fn appserver_token_usage_updates_usage_without_ending_the_turn() {
    // A real session reports this after every step within a turn (each
    // tool call, each model round), not just the last, so it must not be
    // read as the turn ending — only `turn/completed` is that signal.
    let event = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"totalTokens":12603,"inputTokens":12598,"cachedInputTokens":9600,"cacheWriteInputTokens":0,"outputTokens":5,"reasoningOutputTokens":0}}}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::UsageUpdated {
            usage: TokenUsage {
                input_tokens: 12598,
                cached_input_tokens: 9600,
                output_tokens: 5,
                reasoning_output_tokens: 0,
            }
        }
    );
}

/// What is actually running is the agent's to state, not the launch's to
/// assume: the `thread/start` response is where the app-server names the
/// model and reasoning effort it resolved, including when the operator
/// pinned neither.
#[test]
fn appserver_thread_start_response_reports_the_model_and_effort_in_use() {
    assert_eq!(
        decode_line(
            Protocol::CodexAppServer,
            r#"{"id":2,"result":{"thread":{"id":"t-9"},"model":"gpt-5.6-sol","modelProvider":"openai","reasoningEffort":"high","cwd":"/tmp/styra/workspace"}}"#,
        ),
        AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
        }
    );
    // A thread reported without them (an older app-server, or the
    // `thread/started` notification) still decodes; the fields stay absent
    // rather than being guessed at.
    assert_eq!(
        decode_line(
            Protocol::CodexAppServer,
            r#"{"id":2,"result":{"thread":{"id":"t"}}}"#
        ),
        AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: None,
            effort: None,
        }
    );
}

#[test]
fn reported_cwd_reads_each_provider_wire_shape() {
    assert_eq!(
        reported_cwd(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","cwd":"/workspace/crates/ui","message":{}}"#,
        ),
        Some("/workspace/crates/ui".into())
    );
    assert_eq!(
        reported_cwd(
            Protocol::CodexAppServer,
            r#"{"id":2,"result":{"thread":{"id":"t-9"},"cwd":"/workspace/crates/ui"}}"#,
        ),
        Some("/workspace/crates/ui".into())
    );
    assert_eq!(
        reported_cwd(
            Protocol::CodexJsonl,
            r#"{"type":"session_meta","payload":{"cwd":"/workspace"}}"#,
        ),
        Some("/workspace".into())
    );
    assert_eq!(
        reported_cwd(Protocol::ClaudeJsonl, r#"{"type":"assistant"}"#),
        None
    );
}

/// Claude Code states a top-level `cwd` only on its `system`/`init` line,
/// which it emits once per turn. An agent that moves mid-turn says so only
/// through the per-tool ingest context on the assistant lines that follow,
/// so reading the top level alone leaves the reported directory a whole
/// turn stale — the Session keeps naming the directory it was launched in
/// until the operator happens to send another message.
#[test]
fn a_claude_move_mid_turn_is_reported_from_the_tool_ingest_context() {
    assert_eq!(
        reported_cwd(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{},"wire_ingest_context":{"toolu_01R4iQQhVV5J6g3XqUCukR1M":{"cwd":"/workspace/.worktrees/audio-transcript"}}}"#,
        ),
        Some("/workspace/.worktrees/audio-transcript".into())
    );
}

/// The turn's own `init` line outranks the ingest context: a tool may run
/// somewhere other than the thread's directory (a subagent, a `cd` inside
/// one command), and only the top level speaks for the thread itself.
#[test]
fn a_stated_claude_cwd_outranks_the_tool_ingest_context() {
    assert_eq!(
        reported_cwd(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","cwd":"/workspace","wire_ingest_context":{"toolu_1":{"cwd":"/workspace/crates/ui"}}}"#,
        ),
        Some("/workspace".into())
    );
}

/// Tool calls that ran in different directories name no directory for the
/// thread: one of them is a subagent or a command that moved on its way,
/// and nothing on the line says which. Staying put costs the rest of a
/// turn; following the wrong one costs until something moves it back.
#[test]
fn disagreeing_tool_ingest_directories_report_nothing() {
    assert_eq!(
        reported_cwd(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","wire_ingest_context":{"toolu_1":{"cwd":"/workspace/crates/ui"},"toolu_2":{"cwd":"/workspace/.worktrees/audio"}}}"#,
        ),
        None
    );
    // Agreeing entries still speak, however many of them there are.
    assert_eq!(
        reported_cwd(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","wire_ingest_context":{"toolu_1":{"cwd":"/workspace/crates/ui"},"toolu_2":{"cwd":"/workspace/crates/ui"}}}"#,
        ),
        Some("/workspace/crates/ui".into())
    );
    // An entry that names no directory at all leaves the line silent
    // rather than letting its neighbours speak for it.
    assert_eq!(
        reported_cwd(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","wire_ingest_context":{"toolu_1":{"cwd":"/workspace"},"toolu_2":{}}}"#,
        ),
        None
    );
    assert_eq!(
        reported_cwd(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","wire_ingest_context":{}}"#,
        ),
        None
    );
}

/// The reported model and effort must survive a journal round trip, since a
/// stored session is read back through the same decoded events.
#[test]
fn a_reported_model_and_effort_survive_the_journal_round_trip() {
    let event = AgentEvent::ThreadStarted {
        thread_id: "t-9".into(),
        model: Some("gpt-5.6-sol".into()),
        effort: Some("xhigh".into()),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert_eq!(serde_json::from_str::<AgentEvent>(&json).unwrap(), event);
    assert!(event.summary().contains("gpt-5.6-sol"));
    assert!(event.summary().contains("xhigh"));

    // An older journal entry with neither field still reads back.
    let bare: AgentEvent =
        serde_json::from_str(r#"{"type":"thread_started","thread_id":"t"}"#).unwrap();
    assert_eq!(
        bare,
        AgentEvent::ThreadStarted {
            thread_id: "t".into(),
            model: None,
            effort: None,
        }
    );
}

#[test]
fn appserver_turn_completed_is_the_end_of_turn_signal() {
    let event = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"t1","status":"completed"}}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage::default()
        }
    );
    // Nothing is invented for the figures the notification does not carry.
    assert_eq!(event.summary(), "turn complete");
}

/// The app-server states a failure on the ending itself, so the ending is
/// what reports it — an `error` notification may or may not have arrived
/// first, and the turn ended either way.
#[test]
fn an_appserver_turn_that_failed_ends_saying_why() {
    let event = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"t1","status":"failed",
               "error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded"}}}}"#,
    );
    assert_eq!(
        event,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed {
                message: "You've hit your usage limit.".into()
            },
            usage: TurnUsage::default(),
        }
    );
    assert_eq!(event.summary(), "turn failed: You've hit your usage limit.");

    // An interrupted turn is not a failed one: the client that asked for
    // the interrupt is the only thing that knows it happened.
    let interrupted = decode_line(
        Protocol::CodexAppServer,
        r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"t1","status":"interrupted"}}}"#,
    );
    assert_eq!(
        interrupted,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage::default(),
        }
    );
}

/// `usage` changed shape, from what one turn spent to both figures. A peer
/// on a build from before that sends the old shape, and the fields do not
/// collide, so without saying otherwise serde would read it as an ending
/// that reported nothing — dropping a figure that was measured and stated.
/// It has to fail instead.
#[test]
fn an_ending_in_the_old_usage_shape_is_a_decode_error() {
    let old = r#"{"type":"turn_completed","usage":{"input_tokens":7,"output_tokens":3,
            "cached_input_tokens":0,"reasoning_output_tokens":0}}"#;
    assert!(serde_json::from_str::<AgentEvent>(old).is_err());

    // An ending with nothing to report still carries a `usage`, so the
    // empty object is the honest shape and has to keep decoding.
    let empty = r#"{"type":"turn_completed","usage":{}}"#;
    assert_eq!(
        serde_json::from_str::<AgentEvent>(empty).unwrap(),
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage::default(),
        }
    );

    // And that is what this build writes, so its own lines round-trip.
    let ours = serde_json::to_string(&AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Failed {
            message: "out of credits".into(),
        },
        usage: TurnUsage {
            turn: Some(TokenUsage {
                input_tokens: 7,
                ..Default::default()
            }),
            total: None,
        },
    })
    .unwrap();
    assert_eq!(
        serde_json::from_str::<AgentEvent>(&ours).unwrap(),
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed {
                message: "out of credits".into()
            },
            usage: TurnUsage {
                turn: Some(TokenUsage {
                    input_tokens: 7,
                    ..Default::default()
                }),
                total: None,
            },
        }
    );
}

/// The summary flattens a long failure to its first line; the detail pane
/// is where the whole of it can be read.
#[test]
fn a_failed_ending_keeps_the_whole_message_in_its_detail() {
    let event = AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Failed {
            message: "the model refused\nand said so at length".into(),
        },
        usage: TurnUsage::default(),
    };
    assert_eq!(event.summary(), "turn failed: the model refused");
    assert_eq!(
        event.detail(),
        vec![DetailBlock::Text(
            "the model refused\nand said so at length".into()
        )]
    );
}

/// The app-server states a running thread total and nothing at the turn's
/// end, so what the turn itself cost is the distance between the totals
/// either side of it.
#[test]
fn a_turn_between_two_running_totals_reports_both_figures() {
    let mut tracker = UsageTracker::default();
    let total = |input: u64, output: u64| TokenUsage {
        input_tokens: input,
        output_tokens: output,
        ..Default::default()
    };
    for mut event in [
        AgentEvent::TurnStarted,
        AgentEvent::UsageUpdated {
            usage: total(100, 10),
        },
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage::default(),
        },
        AgentEvent::TurnStarted,
        AgentEvent::UsageUpdated {
            usage: total(250, 40),
        },
    ] {
        tracker.observe(&mut event);
    }
    let mut second = AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    };
    tracker.observe(&mut second);
    assert_eq!(
        second,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage {
                turn: Some(total(150, 30)),
                total: Some(total(250, 40)),
            },
        }
    );
    assert_eq!(
        second.summary(),
        "turn complete · this turn in 150 · out 30 · cached 0 \
             (thread total in 250 · out 40 · cached 0)"
    );
}

/// Claude states what each turn cost and never a thread total, so the
/// total is the turns added up.
#[test]
fn per_turn_figures_add_up_into_the_thread_total() {
    let mut tracker = UsageTracker::default();
    let turn = |input: u64| TurnUsage {
        turn: Some(TokenUsage {
            input_tokens: input,
            ..Default::default()
        }),
        total: None,
    };
    let mut first = AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: turn(12),
    };
    tracker.observe(&mut first);
    let mut second = AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: turn(30),
    };
    tracker.observe(&mut second);

    let totals = |event: &AgentEvent| match event {
        AgentEvent::TurnCompleted { usage, .. } => (
            usage.turn.as_ref().unwrap().input_tokens,
            usage.total.as_ref().unwrap().input_tokens,
        ),
        other => panic!("not a turn ending: {other:?}"),
    };
    assert_eq!(totals(&first), (12, 12));
    assert_eq!(totals(&second), (30, 42));
}

/// An ending the host synthesized for an interrupt reports nothing, and a
/// figure that was never measured must not be shown as zero.
#[test]
fn an_ending_with_nothing_reported_states_nothing() {
    let mut event = AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    };
    UsageTracker::default().observe(&mut event);
    assert_eq!(
        event,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage::default()
        }
    );
    assert!(event.detail().is_empty());
}

/// A turn's end is a turn event; only the running totals are tagged as
/// usage, and each says which of the two figures it is.
#[test]
fn the_two_usage_lines_are_told_apart() {
    let ending = AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    };
    assert_eq!(ending.tag(), "turn");
    assert_eq!(AgentEvent::TurnStarted.tag(), ending.tag());

    let running = AgentEvent::UsageUpdated {
        usage: TokenUsage {
            input_tokens: 9,
            ..Default::default()
        },
    };
    assert_eq!(running.tag(), "usage");
    assert_eq!(running.summary(), "thread total · in 9 · out 0 · cached 0");
}

#[test]
fn appserver_control_and_echoed_user_message_carry_without_rendering() {
    // A response (no "method") is control traffic, unless it reports a
    // started thread — see the model/effort test below.
    for line in [
        r#"{"id":1,"result":{}}"#,
        r#"{"id":9,"result":{"turn":{"id":"t1"}}}"#,
    ] {
        assert_eq!(
            decode_line(Protocol::CodexAppServer, line),
            AgentEvent::Unknown {
                wire_type: "response".into()
            }
        );
    }
    // The server echoes the operator's own message; the host shows its own, so
    // this decodes to Unknown rather than duplicating it.
    assert_eq!(
        decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"item/completed","params":{"item":{"type":"userMessage","id":"u","content":[{"type":"text","text":"hi"}]}}}"#
        ),
        AgentEvent::Unknown {
            wire_type: "item:userMessage".into()
        }
    );
}
