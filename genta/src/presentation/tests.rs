use super::*;

fn claude_tool(name: &str, input: &str) -> AgentEvent {
    AgentEvent::ToolCompleted {
        id: "toolu_1".into(),
        name: name.into(),
        detail: input.into(),
        status: "completed".into(),
        output: "line\n".into(),
    }
}

fn claude_summary(event: &AgentEvent) -> String {
    Protocol::ClaudeJsonl.presented_summary(event, PresentationMode::Pretty)
}

#[test]
fn claude_reads_show_the_path_and_its_line_range() {
    let event = claude_tool(
        "Read",
        r#"{"file_path":"/w/lua/tree.lua","limit":120,"offset":255}"#,
    );
    assert_eq!(claude_summary(&event), "/w/lua/tree.lua:255-375");
    assert_eq!(
        Protocol::ClaudeJsonl.presented_detail(&event, PresentationMode::Pretty),
        vec![
            DetailBlock::Code {
                language: None,
                text: "/w/lua/tree.lua:255-375".into(),
            },
            DetailBlock::Code {
                language: None,
                text: "line\n".into(),
            },
        ]
    );
}

#[test]
fn claude_reads_without_a_range_are_just_the_path() {
    let event = claude_tool("Read", r#"{"file_path":"/w/README.md"}"#);
    assert_eq!(claude_summary(&event), "/w/README.md");
}

#[test]
fn claude_searches_read_as_pattern_and_scope() {
    assert_eq!(
        claude_summary(&claude_tool(
            "Grep",
            r#"{"pattern":"fn main","path":"src","output_mode":"content"}"#
        )),
        "fn main in src"
    );
    assert_eq!(
        claude_summary(&claude_tool("Glob", r#"{"pattern":"**/*.rs"}"#)),
        "**/*.rs"
    );
}

#[test]
fn claude_bash_still_wins_over_the_generic_object() {
    let event = claude_tool(
        "Bash",
        r#"{"command":"cargo test","run_in_background":true}"#,
    );
    assert_eq!(claude_summary(&event), "cargo test");
    assert!(matches!(
        Protocol::ClaudeJsonl
            .presented_detail(&event, PresentationMode::Pretty)
            .first(),
        Some(DetailBlock::Code { language: Some(language), .. }) if language == "bash"
    ));
}

#[test]
fn claude_leaves_unknown_tools_to_the_stable_vocabulary() {
    let event = claude_tool("mcp__thing__do", r#"{"weird":1}"#);
    assert_eq!(
        claude_summary(&event),
        r#"mcp__thing__do: {"weird":1} (completed)"#
    );
}

#[test]
fn claude_raw_mode_keeps_the_request_object() {
    let event = claude_tool("Read", r#"{"file_path":"/w/README.md"}"#);
    assert!(matches!(
        Protocol::ClaudeJsonl
            .presented_detail(&event, PresentationMode::Raw)
            .first(),
        Some(DetailBlock::Text(text)) if text.contains(r#"{"file_path":"/w/README.md"}"#)
    ));
}

#[test]
fn codex_unwraps_a_bash_login_command_but_keeps_output() {
    let event = AgentEvent::CommandCompleted {
        command: "/usr/bin/bash -lc 'cargo test --all'".into(),
        status: "completed".into(),
        exit_code: Some(0),
        output: "ok\n".into(),
    };
    assert_eq!(
        Protocol::CodexAppServer.presented_summary(&event, PresentationMode::Pretty),
        "cargo test --all"
    );
    assert_eq!(
        Protocol::CodexAppServer.presented_detail(&event, PresentationMode::Pretty),
        vec![
            DetailBlock::Code {
                language: Some("bash".into()),
                text: "cargo test --all".into(),
            },
            DetailBlock::Code {
                language: None,
                text: "ok\n".into(),
            },
        ]
    );
    assert!(matches!(
        Protocol::CodexAppServer
            .presented_detail(&event, PresentationMode::Raw)
            .first(),
        Some(DetailBlock::Text(text)) if text.contains("/usr/bin/bash -lc")
    ));
}

#[test]
fn codex_leaves_non_shell_commands_alone() {
    let event = AgentEvent::CommandStarted {
        command: "cargo test".into(),
    };
    assert_eq!(
        Protocol::CodexJsonl.presented_summary(&event, PresentationMode::Pretty),
        "cargo test (running)"
    );
}

#[test]
fn codex_unwraps_shell_escaped_quotes() {
    let event = AgentEvent::CommandStarted {
        command: r#"/bin/bash -lc 'printf '"'"'%s\n'"'"' hello'"#.into(),
    };
    assert_eq!(
        Protocol::CodexJsonl.presented_summary(&event, PresentationMode::Pretty),
        "printf '%s\\n' hello"
    );
}
