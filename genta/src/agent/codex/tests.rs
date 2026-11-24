use super::*;
use crate::agent::testing::{agent_bin, builtin, stub};
use serde_json::Value;
use std::path::PathBuf;

fn batch(name: &str, layout: &SandboxLayout) -> Profile {
    crate::agent::Selection::parse(name)
        .unwrap()
        .resolve_for_on_path(ExecutionMode::Batch, layout, agent_bin().as_os_str())
        .unwrap()
}

#[test]
fn codex_exec_profile_isolates_the_workspace_and_speaks_the_decoded_protocol() {
    let layout = SandboxLayout::default();
    let profile = batch("codex", &layout);

    assert_eq!(profile.protocol, Protocol::CodexJsonl);
    assert!(profile.network);
    assert!(profile.single_turn);
    assert_eq!(profile.command[0], stub("codex"));
    assert!(profile
        .command
        .iter()
        .any(|arg| arg == "danger-full-access"));
    assert!(profile.command.iter().any(|arg| arg == "exec"));
    assert!(profile.command.iter().any(|arg| arg == "--json"));
    assert_eq!(
        profile.command.last().unwrap(),
        "-",
        "prompt is read from stdin"
    );
    assert!(profile
        .command
        .iter()
        .any(|arg| arg.contains("/tmp/styra/workspace") && arg.contains("trusted")));
    assert!(profile.mounts.iter().any(|mount| {
        mount.destination == std::path::Path::new("/tmp/agent-home/.codex") && mount.writable
    }));
    assert_eq!(
        profile.environment.get("HOME"),
        Some(&"/tmp/agent-home".to_string())
    );
}

#[test]
fn codex_exec_profile_accepts_host_selected_executable_and_prompt() {
    let layout = SandboxLayout {
        workspace: "/tmp/orka/workspace".into(),
    };
    let profile = codex_exec(
        &layout,
        Path::new("/opt/codex"),
        "read the staged prompt",
        "gpt-5.6-terra",
        Effort::Low,
    );

    assert_eq!(profile.command[0], "/opt/codex");
    assert_eq!(profile.command.last().unwrap(), "read the staged prompt");
    assert!(profile.command.iter().any(|argument| {
        argument == "projects.\"/tmp/orka/workspace\".trust_level=\"trusted\""
    }));
    assert_eq!(profile.protocol, Protocol::CodexJsonl);
    assert_eq!(profile.environment["HOME"], "/tmp/agent-home");
    assert_eq!(
        profile.mounts[0].destination,
        PathBuf::from("/tmp/agent-home/.codex")
    );
    assert!(profile.network);
    assert!(profile.single_turn);
}

#[test]
fn default_codex_profile_is_the_multi_turn_app_server() {
    let profile = builtin("codex", &SandboxLayout::default()).unwrap();
    assert_eq!(profile.protocol, Protocol::CodexAppServer);
    assert!(!profile.single_turn, "app-server sessions span many turns");
    // The bare name pins the declared defaults, as `-c` overrides ahead of
    // the subcommand.
    assert_eq!(profile.command[0], stub("codex"));
    assert_eq!(profile.command.last().unwrap(), "app-server");
    assert!(profile
        .command
        .iter()
        .any(|arg| arg == &format!("model={:?}", Provider::Codex.default_model())));
    assert!(profile.command.iter().any(|arg| arg
        == &format!(
            "model_reasoning_effort={:?}",
            Provider::Codex.default_effort().as_str()
        )));
    assert!(profile.network);
    // Isolation policy is shared with the exec profile.
    assert!(profile
        .mounts
        .iter()
        .any(|mount| { mount.destination == std::path::Path::new("/tmp/agent-home/.codex") }));
    assert_eq!(
        profile.environment.get("HOME"),
        Some(&"/tmp/agent-home".to_string())
    );
}

/// Codex takes both as `-c` config overrides, and they must land on the
/// `codex` process itself — before the subcommand, which does not accept
/// them.
#[test]
fn codex_model_and_effort_become_config_overrides_before_the_subcommand() {
    let layout = SandboxLayout::default();
    let name = "codex:gpt-5.6-terra/xhigh";
    for (mode, subcommand) in [
        (ExecutionMode::Conversation, "app-server"),
        (ExecutionMode::Batch, "exec"),
    ] {
        let command = crate::agent::Selection::parse(name)
            .unwrap()
            .resolve_for_on_path(mode, &layout, agent_bin().as_os_str())
            .unwrap()
            .command;
        let position = |argument: &str| {
            command
                .iter()
                .position(|candidate| candidate == argument)
                .unwrap_or_else(|| panic!("{name} is missing {argument}: {command:?}"))
        };
        let model = position(r#"model="gpt-5.6-terra""#);
        let effort = position(r#"model_reasoning_effort="xhigh""#);
        assert_eq!(command[model - 1], "-c");
        assert_eq!(command[effort - 1], "-c");
        assert!(
            model < position(subcommand) && effort < position(subcommand),
            "{name}: overrides must precede {subcommand}: {command:?}"
        );
    }
}

/// A one-shot run can still be the next turn of a thread: `exec resume`,
/// with exec's own options, the thread and stdin's prompt last.
#[test]
fn codex_exec_resumes_through_its_own_subcommand() {
    let mut profile = batch("codex", &SandboxLayout::default());
    profile.resume(Provider::Codex, "thread-1").unwrap();
    let command = &profile.command;
    let exec = command
        .iter()
        .position(|argument| argument == "exec")
        .unwrap();
    assert_eq!(command[exec + 1], "resume");
    assert_eq!(command[command.len() - 2..], ["thread-1", "-"]);
    assert!(command.iter().any(|argument| argument == "--json"));
}

/// Codex resumes through the app-server handshake, so the command line the
/// profile carries is untouched.
#[test]
fn codex_resume_leaves_the_command_line_alone() {
    let mut profile = builtin("codex", &SandboxLayout::default()).unwrap();
    let before = profile.command.clone();
    profile.resume(Provider::Codex, "thread-1").unwrap();
    assert_eq!(profile.command, before);
}

fn codex_submission_profile() -> Profile {
    Profile {
        message_format: MessageFormat::CodexSubmission,
        ..codex(
            ExecutionMode::Batch,
            &SandboxLayout::default(),
            Path::new("codex"),
            Provider::Codex.default_model(),
            Provider::Codex.default_effort(),
        )
    }
}

#[test]
fn codex_submission_is_valid_json_carrying_the_text_and_one_line() {
    let profile = codex_submission_profile();
    let encoded = profile.encode_message("fix the bug\nand test it");
    assert_eq!(*encoded.last().unwrap(), b'\n');
    assert_eq!(
        encoded.iter().filter(|&&b| b == b'\n').count(),
        1,
        "a submission must be exactly one input line"
    );
    let line = std::str::from_utf8(&encoded).unwrap().trim_end();
    let value: Value = serde_json::from_str(line).expect("submission is valid JSON");
    assert_eq!(value["op"]["items"][0]["text"], "fix the bug\nand test it");
    assert!(value["id"].is_string());
}

#[test]
fn distinct_submissions_get_distinct_ids() {
    let profile = codex_submission_profile();
    let a = String::from_utf8(profile.encode_message("a")).unwrap();
    let b = String::from_utf8(profile.encode_message("b")).unwrap();
    let id = |s: &str| {
        serde_json::from_str::<Value>(s.trim_end()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_ne!(id(&a), id(&b));
}

/// The exec profile's own format: `codex exec -` reads a plain stdin turn.
#[test]
fn plain_line_format_flattens_to_a_single_line() {
    let profile = codex(
        ExecutionMode::Batch,
        &SandboxLayout::default(),
        Path::new("codex"),
        Provider::Codex.default_model(),
        Provider::Codex.default_effort(),
    );
    assert_eq!(profile.message_format, MessageFormat::PlainLine);
    let encoded = profile.encode_message("one\ntwo");
    assert_eq!(encoded, b"one two\n");
}
