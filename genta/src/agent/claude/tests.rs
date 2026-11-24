use super::*;
use crate::agent::testing::{builtin, stub};
use serde_json::Value;

#[test]
fn claude_profile_speaks_stream_json_and_isolates_credentials() {
    let profile = builtin("claude", &SandboxLayout::default()).unwrap();

    assert_eq!(
        profile.name, "claude:claude-opus-5-5/medium",
        "a bare name still pins"
    );
    assert_eq!(profile.protocol, Protocol::ClaudeJsonl);
    assert_eq!(profile.message_format, MessageFormat::ClaudeStreamJson);
    assert!(profile.network);
    assert_eq!(profile.command[0], stub("claude"));
    assert!(profile.command.iter().any(|arg| arg == "stream-json"));
    assert!(profile
        .command
        .iter()
        .any(|arg| arg == "--dangerously-skip-permissions"));
    // Skipping the permission prompt makes Claude Code push the agent
    // toward Bash for file work; the profile pays that back in the same
    // command line rather than relying on host configuration.
    assert!(
        profile
            .command
            .windows(2)
            .any(|pair| pair[0] == "--append-system-prompt" && pair[1] == DEDICATED_FILE_TOOLS),
        "the bypass-mode file-tool guidance is countermanded in-band"
    );
    // A bare `claude` pins the provider's declared default rather than
    // leaving the model to Claude Code's own configuration.
    assert!(profile
        .command
        .windows(2)
        .any(|pair| pair[0] == "--model" && pair[1] == Provider::Claude.default_model()));
    assert!(
        !profile.single_turn,
        "an interactive claude session spans many turns"
    );
    assert!(profile.mounts.iter().any(|mount| mount.destination
        == std::path::Path::new("/tmp/agent-home/.claude")
        && mount.writable));
    assert_eq!(
        profile.environment.get("HOME"),
        Some(&"/tmp/agent-home".to_string())
    );
}

#[test]
fn claude_resume_is_a_native_launch_flag() {
    let mut profile = builtin("claude", &SandboxLayout::default()).unwrap();
    profile
        .resume(Provider::Claude, "claude-session-1")
        .unwrap();
    assert!(profile
        .command
        .windows(2)
        .any(|pair| pair[0] == "--resume" && pair[1] == "claude-session-1"));
}

/// Claude Code's own aliases (`opus`, `sonnet`) move to whatever the latest
/// release is, so the catalog offers full ids instead: a journal then records
/// the exact model a session ran on, and re-parsing that profile reproduces
/// it rather than silently landing on a newer model.
#[test]
fn the_claude_catalog_offers_full_model_ids() {
    for entry in Provider::Claude.models() {
        assert!(
            entry.id.starts_with("claude-"),
            "{} is not a full model id",
            entry.id
        );
    }
    assert!(Provider::Claude.model("claude-opus-5").is_some());
    // Anthropic lists this one as deprecated, and Mythos is reachable only
    // through Project Glasswing — neither belongs in a catalog offered to
    // every operator.
    assert!(Provider::Claude.model("claude-opus-4-1-20250805").is_none());
    assert!(Provider::Claude.model("claude-mythos-5").is_none());
}

#[test]
fn claude_model_is_selected_by_the_profile_suffix() {
    let profile = builtin("claude:opus", &SandboxLayout::default()).unwrap();
    assert_eq!(
        profile.name, "claude:opus/medium",
        "the missing effort defaults"
    );
    let model = profile
        .command
        .windows(2)
        .find(|pair| pair[0] == "--model")
        .map(|pair| pair[1].as_str());
    assert_eq!(model, Some("opus"));
}

#[test]
fn claude_effort_becomes_an_effort_argument() {
    let command = builtin("claude:opus/max", &SandboxLayout::default())
        .unwrap()
        .command;
    let argument = |flag: &str| {
        command
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
    };
    assert_eq!(argument("--model"), Some("opus"));
    assert_eq!(argument("--effort"), Some("max"));
}

#[test]
fn claude_submission_is_valid_json_carrying_the_text_and_one_line() {
    let profile = claude(
        &SandboxLayout::default(),
        Path::new("claude"),
        Provider::Claude.default_model(),
        Provider::Claude.default_effort(),
    );
    let encoded = profile.encode_message("fix the bug\nand test it");
    assert_eq!(*encoded.last().unwrap(), b'\n');
    assert_eq!(
        encoded.iter().filter(|&&b| b == b'\n').count(),
        1,
        "a stream-json message must be exactly one input line"
    );
    let line = std::str::from_utf8(&encoded).unwrap().trim_end();
    let value: Value = serde_json::from_str(line).expect("submission is valid JSON");
    assert_eq!(value["type"], "user");
    assert_eq!(value["message"]["role"], "user");
    assert_eq!(value["message"]["content"], "fix the bug\nand test it");
}
