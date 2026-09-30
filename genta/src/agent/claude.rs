//! Claude Code: the multi-turn `stream-json` provider.

use super::spec::{ProviderOps, ProviderSpec};
use super::{profile_name, Effort, MessageFormat, MountSpec, Profile, Provider, SandboxLayout};
use crate::event::Protocol;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Models worth offering in a picker, most capable first.
///
/// Every model listed `Active` in Anthropic's model-status table
/// (<https://platform.claude.com/docs/en/about-claude/model-deprecations>), read
/// on 2026-07-27, in that table's order — tier by tier, newest first. Two
/// knowingly-excluded classes: `claude-opus-4-1-20250805` is `Deprecated` there
/// (retires 2026-08-05), and `claude-mythos-5` is reachable only through
/// Project Glasswing, so offering it to every operator would suggest an agent
/// most cannot launch. Full ids rather than the `opus`/`sonnet` aliases, so a
/// journal records the exact model a session ran on even after an alias moves
/// to a newer release.
const MODELS: &[&str] = &[
    "claude-fable-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-opus-4-5-20251101",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-sonnet-4-5-20250929",
    "claude-haiku-4-5-20251001",
];

/// Claude Code's effort ladder, lowest first. It has a `max` rung codex does
/// not, and lacks codex's `minimal`.
const EFFORTS: &[Effort] = &[
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

static OPS: Ops = Ops;

/// The catalog leads with `claude-fable-5`, but that tier is priced above Opus,
/// so an operator who named no model gets `claude-opus-5` instead — see
/// [`Provider::default_model`](super::Provider::default_model) for why the
/// default is declared rather than read off the front of the catalog.
pub(super) static SPEC: ProviderSpec = ProviderSpec {
    name: "claude",
    executable: "claude",
    protocol: Protocol::ClaudeJsonl,
    models: MODELS,
    efforts: EFFORTS,
    default_model: "claude-opus-5-5",
    default_effort: Effort::Medium,
    cheapest_model: "claude-haiku-4-5-20251001",
    ops: &OPS,
};

struct Ops;

impl ProviderOps for Ops {
    fn profile(
        &self,
        layout: &SandboxLayout,
        executable: &Path,
        model: &str,
        effort: Effort,
    ) -> Profile {
        claude(layout, executable, model, effort)
    }

    /// Claude Code resumes at process launch time, so the id joins the command
    /// line the profile carries.
    fn resume(&self, profile: &mut Profile, provider_session_id: &str) -> Result<()> {
        profile.command.push("--resume".into());
        profile.command.push(provider_session_id.to_owned());
        Ok(())
    }
}

/// Countermands the file-tool guidance `--dangerously-skip-permissions` adds.
///
/// Kept next to the flag it answers so the two are read together: if that flag
/// ever leaves the profile, this goes with it.
const DEDICATED_FILE_TOOLS: &str = "\
Use the dedicated Read, Edit, and Write tools for reading and editing files. \
Do not route file work through Bash (cat, head, sed, heredocs, sed -i). This \
overrides any guidance to prefer Bash for file work under bypass permissions \
mode: this session is isolated in a sandbox, so tool calls are not gated by \
permission prompts and there is nothing to economize on. Bash remains correct \
for running commands, builds, tests, git, and process inspection, and for \
search with grep and find.";

/// The built-in Claude Code interactive profile.
///
/// The isolation mirrors the codex shape: Driva's outer Bubblewrap sandbox is
/// the boundary, so Claude Code's own permission prompt is skipped with
/// `--dangerously-skip-permissions`. `HOME` lives under `/tmp`, the writable
/// tmpfs Driva always provides, matching the codex profile's rationale: a
/// disposable, always-present home without depending on a particular
/// directory existing in the host rootfs. `~/.claude` is bound in under it,
/// writable, so refreshed credentials and native session transcripts persist
/// across interactions.
///
/// `--dangerously-skip-permissions` has a side effect beyond the prompt it
/// suppresses: Claude Code also injects guidance telling the agent to do its
/// file work through `cat`/`sed`/heredocs rather than the dedicated file
/// tools. That trade is aimed at sessions where each tool call costs a
/// permission prompt, which is not this one: the flag is set because Driva is
/// already the boundary, not because prompts are expensive. Bash-routed edits
/// are also worse here, since they leave transcripts of shell invocations
/// rather than legible file diffs. `--append-system-prompt` therefore restores
/// the dedicated tools, and is passed next to the flag that makes it needed.
///
/// The command drives Claude Code's bidirectional `stream-json` mode: it reads
/// `stream-json` user messages on stdin and emits `stream-json` events on
/// stdout, staying alive until stdin closes (so, like the app-server codex
/// profile, it spans many turns rather than running once to completion).
/// `--verbose` is required alongside `--output-format stream-json` under
/// `--print`. An optional `model` becomes a `--model` argument and an optional
/// `effort` an `--effort` one; when either is absent, Claude Code uses its
/// configured default. `executable` is the Claude Code binary to launch, as
/// located by [`resolve_executable`](super::resolve_executable): the common
/// install puts it in `~/.local/bin`, which the sandbox's `PATH` does not
/// contain.
///
/// NOTE: the exact flags and the `stream-json` envelope in [`claude_submission`]
/// must be confirmed against the installed `claude` version; both are isolated
/// here so adapting to a different contract is a localized change plus, if the
/// event schema differs, the [`Protocol::ClaudeJsonl`](crate::event::Protocol)
/// decoder.
pub fn claude(_layout: &SandboxLayout, executable: &Path, model: &str, effort: Effort) -> Profile {
    let mut command = vec![
        executable.to_string_lossy().into_owned(),
        "--print".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--dangerously-skip-permissions".into(),
        "--append-system-prompt".into(),
        DEDICATED_FILE_TOOLS.into(),
    ];
    command.push("--model".into());
    command.push(model.to_owned());
    command.push("--effort".into());
    command.push(effort.as_str().into());
    Profile {
        name: profile_name(Provider::Claude, model, effort),
        command,
        protocol: Protocol::ClaudeJsonl,
        mounts: vec![MountSpec {
            // Claude Code's native resume state lives alongside its
            // credentials under ~/.claude.
            source: "~/.claude".into(),
            destination: "/tmp/agent-home/.claude".into(),
            writable: true,
        }],
        environment: BTreeMap::from([
            ("HOME".into(), "/tmp/agent-home".into()),
            ("TERM".into(), "xterm-256color".into()),
        ]),
        network: true,
        message_format: MessageFormat::ClaudeStreamJson,
        single_turn: false,
    }
}

/// Build a Claude Code `stream-json` user message carrying the operator's text.
///
/// The text becomes the `content` of one user turn. Because it is a JSON string
/// value, embedded newlines are escaped rather than split, so the envelope
/// remains exactly one input line.
pub(crate) fn claude_submission(text: &str) -> String {
    let submission = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    });
    submission.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::{builtin, stub};
    use serde_json::Value;

    #[test]
    fn claude_profile_speaks_stream_json_and_isolates_credentials() {
        let profile = builtin("claude", &SandboxLayout::default()).unwrap();

        assert_eq!(
            profile.name, "claude:claude-opus-5/high",
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
        for model in Provider::Claude.models() {
            assert!(
                model.starts_with("claude-"),
                "{model} is not a full model id"
            );
        }
        assert!(Provider::Claude.models().contains(&"claude-opus-5"));
        // Anthropic lists this one as deprecated, and Mythos is reachable only
        // through Project Glasswing — neither belongs in a catalog offered to
        // every operator.
        assert!(!Provider::Claude
            .models()
            .contains(&"claude-opus-4-1-20250805"));
        assert!(!Provider::Claude.models().contains(&"claude-mythos-5"));
    }

    #[test]
    fn claude_model_is_selected_by_the_profile_suffix() {
        let profile = builtin("claude:opus", &SandboxLayout::default()).unwrap();
        assert_eq!(
            profile.name, "claude:opus/high",
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
}
