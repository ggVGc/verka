//! Codex: the multi-turn `app-server` provider and the one-shot `exec` one.
//!
//! Two providers over one binary. They share an executable, a catalog, an
//! effort ladder, and an isolation policy, and differ in wire protocol,
//! declared defaults, and whether a session spans more than one turn. The
//! sharing is by named const below rather than by inheriting one spec from the
//! other, so each still states every field it has — see
//! [`ProviderSpec`](super::spec::ProviderSpec).

use super::spec::{ProviderOps, ProviderSpec};
use super::{profile_name, Effort, MessageFormat, MountSpec, Profile, Provider, SandboxLayout};
use crate::event::Protocol;
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The codex binary both providers launch.
const EXECUTABLE: &str = "codex";

/// Models worth offering in a picker, most capable first. Shared by both codex
/// providers: the catalog is the agent's, and both launch the same agent.
const MODELS: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];

/// Codex's effort ladder, lowest first. It has a `minimal` rung Claude Code
/// does not, and lacks Claude Code's `max`.
const EFFORTS: &[Effort] = &[
    Effort::Minimal,
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
];

/// The least expensive codex model, for the incidental errands a host runs
/// around a session rather than for the session's own work.
const CHEAPEST_MODEL: &str = "gpt-5.6-luna";

static APP_SERVER_OPS: AppServerOps = AppServerOps;
static EXEC_OPS: ExecOps = ExecOps;

/// Interactive codex over the `app-server` JSON-RPC protocol. Styra's
/// interactive sessions default to the balanced Terra profile at `medium`.
pub(super) static APP_SERVER_SPEC: ProviderSpec = ProviderSpec {
    name: "codex",
    executable: EXECUTABLE,
    protocol: Protocol::CodexAppServer,
    models: MODELS,
    efforts: EFFORTS,
    default_model: "gpt-5.6-terra",
    default_effort: Effort::Medium,
    cheapest_model: CHEAPEST_MODEL,
    ops: &APP_SERVER_OPS,
};

/// Batch `codex exec --json`, which keeps its own explicit defaults: the
/// strongest model, at `high`.
pub(super) static EXEC_SPEC: ProviderSpec = ProviderSpec {
    name: "codex-exec",
    executable: EXECUTABLE,
    protocol: Protocol::CodexJsonl,
    models: MODELS,
    efforts: EFFORTS,
    default_model: "gpt-5.6-sol",
    default_effort: Effort::High,
    cheapest_model: CHEAPEST_MODEL,
    ops: &EXEC_OPS,
};

struct AppServerOps;
struct ExecOps;

impl ProviderOps for AppServerOps {
    fn profile(
        &self,
        layout: &SandboxLayout,
        executable: &Path,
        model: &str,
        effort: Effort,
    ) -> Profile {
        codex_appserver(layout, executable, model, effort)
    }

    /// Codex resumes through its app-server protocol rather than its command
    /// line, so the profile is unchanged and the host passes the id to
    /// [`AppServer`](crate::appserver::AppServer).
    fn resume(&self, _profile: &mut Profile, _provider_session_id: &str) -> Result<()> {
        Ok(())
    }
}

impl ProviderOps for ExecOps {
    fn profile(
        &self,
        layout: &SandboxLayout,
        executable: &Path,
        model: &str,
        effort: Effort,
    ) -> Profile {
        codex(layout, executable, model, effort)
    }

    /// A one-shot run has no conversation to reopen.
    fn resume(&self, _profile: &mut Profile, _provider_session_id: &str) -> Result<()> {
        bail!("provider codex-exec does not support resuming sessions")
    }
}

/// The built-in multi-turn codex profile, over the `app-server` JSON-RPC
/// protocol (verified against codex-cli 0.145).
///
/// The process is `codex app-server` on stdio; [`crate::appserver::AppServer`]
/// owns the initialize → thread/start → turn/start handshake and per-message
/// turn dispatch, so `message_format` is unused here and the session keeps
/// stdin open across turns. Isolation matches the exec profile below; the
/// thread itself is started with `approvalPolicy: never` and a
/// danger-full-access inner sandbox, delegating real isolation to Driva.
/// `model` and `effort` are always pinned — a profile does not leave either to
/// codex's own configuration, so that what ran is recorded rather than inferred
/// (see [`Selection`](super::Selection)). Both are `-c` config overrides, so
/// they are passed to the `codex` process itself rather than to `app-server`,
/// which inherits them for every thread it starts.
pub fn codex_appserver(
    layout: &SandboxLayout,
    executable: &Path,
    model: &str,
    effort: Effort,
) -> Profile {
    let mut command = vec![executable.to_string_lossy().into_owned()];
    command.extend(codex_model_overrides(model, effort));
    command.push("app-server".into());
    Profile {
        name: profile_name(Provider::Codex, model, effort),
        command,
        protocol: Protocol::CodexAppServer,
        single_turn: false,
        ..codex(layout, executable, model, effort)
    }
}

/// The `-c` overrides that pin codex's model and reasoning effort, in the order
/// they appear on the command line. Values are quoted because `-c` parses them
/// as TOML, where a bare model id is not a valid scalar.
fn codex_model_overrides(model: &str, effort: Effort) -> Vec<String> {
    vec![
        "-c".into(),
        format!("model={model:?}"),
        "-c".into(),
        format!("model_reasoning_effort={:?}", effort.as_str()),
    ]
}

/// The built-in single-turn codex profile.
///
/// Isolation follows Orka's proven codex shape: the workspace is trusted so
/// codex does not prompt, its inner sandbox is disabled in favour of Driva's
/// outer Bubblewrap isolation, `~/.codex` is mounted writable so credentials
/// and native session state persist, and stable `HOME`/`TERM` are set because
/// Bubblewrap clears the environment.
///
/// The command is `codex exec --json -`: a single-turn run that reads the
/// prompt from stdin and streams `thread`/`turn`/`item` events, verified
/// against codex-cli 0.145.
///
/// `executable` is the codex binary to launch, as located by
/// [`resolve_executable`](super::resolve_executable).
pub fn codex(layout: &SandboxLayout, executable: &Path, model: &str, effort: Effort) -> Profile {
    codex_exec(layout, executable, "-", model, effort)
}

/// Build a complete single-turn Codex profile with a host-selected executable
/// and prompt argument.
///
/// Hosts normally use [`codex`], which reads the prompt from stdin. A batch
/// orchestrator may instead stage its full prompt elsewhere and pass a short
/// instruction here, while retaining Genta's command flags, credentials,
/// environment, network policy, and wire-protocol identity as one profile.
pub fn codex_exec(
    layout: &SandboxLayout,
    executable: &Path,
    prompt: &str,
    model: &str,
    effort: Effort,
) -> Profile {
    Profile {
        name: profile_name(Provider::CodexExec, model, effort),
        command: codex_exec_command(
            &executable.to_string_lossy(),
            &layout.workspace.to_string_lossy(),
            prompt,
            model,
            effort,
        ),
        protocol: Protocol::CodexJsonl,
        // HOME lives under /tmp, the writable tmpfs Driva always provides, so
        // codex has a disposable, always-present home without depending on
        // /root existing in the host rootfs. Codex's state directory is bound
        // in below it.
        mounts: vec![MountSpec {
            // Native resume reads Codex's rollout files from this directory.
            // Keeping the whole provider directory mounted also preserves new
            // rollout files created by this interaction.
            source: "~/.codex".into(),
            destination: "/tmp/agent-home/.codex".into(),
            writable: true,
        }],
        environment: BTreeMap::from([
            ("HOME".into(), "/tmp/agent-home".into()),
            ("TERM".into(), "xterm-256color".into()),
        ]),
        network: true,
        message_format: MessageFormat::PlainLine,
        single_turn: true,
    }
}

/// The `codex exec --json` command line shared by hosts: the workspace is
/// trusted so codex does not prompt, its inner sandbox is disabled
/// (`danger-full-access`) in favour of the host's outer isolation, and the
/// prompt is the final argument (`-` reads it from stdin; hosts that stage a
/// prompt file pass their own instruction text instead).
pub fn codex_exec_command(
    executable: &str,
    workspace: &str,
    prompt: &str,
    model: &str,
    effort: Effort,
) -> Vec<String> {
    let trust = format!("projects.{workspace:?}.trust_level=\"trusted\"");
    let mut command = vec![
        executable.into(),
        "-c".into(),
        trust,
        "--sandbox".into(),
        "danger-full-access".into(),
    ];
    command.extend(codex_model_overrides(model, effort));
    command.extend([
        "exec".into(),
        "--skip-git-repo-check".into(),
        "--json".into(),
        prompt.into(),
    ]);
    command
}

/// Build a codex protocol submission line carrying the operator's text.
///
/// The envelope shape may need to track the installed codex; it is kept in one
/// place for that reason. The submission id is unique per process.
pub(super) fn codex_submission(text: &str) -> String {
    let submission = serde_json::json!({
        "id": submission_id(),
        "op": {
            "type": "user_input",
            "items": [{ "type": "text", "text": text }],
        }
    });
    submission.to_string()
}

fn submission_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("styra-{now}-{seq}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testing::{builtin, stub};
    use serde_json::Value;
    use std::path::PathBuf;

    #[test]
    fn codex_exec_profile_isolates_the_workspace_and_speaks_the_decoded_protocol() {
        let layout = SandboxLayout::default();
        let profile = builtin("codex-exec", &layout).unwrap();

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
        for (name, subcommand) in [
            ("codex:gpt-5.6-terra/xhigh", "app-server"),
            ("codex-exec:gpt-5.6-terra/xhigh", "exec"),
        ] {
            let command = builtin(name, &layout).unwrap().command;
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

    /// A one-shot run has no thread to reopen, and says so rather than
    /// launching something that silently starts over.
    #[test]
    fn codex_exec_refuses_to_resume() {
        let mut profile = builtin("codex-exec", &SandboxLayout::default()).unwrap();
        let error = profile
            .resume(Provider::CodexExec, "thread-1")
            .expect_err("a one-shot run cannot be resumed");
        assert!(format!("{error:#}").contains("does not support resuming"));
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
                &SandboxLayout::default(),
                Path::new("codex"),
                Provider::CodexExec.default_model(),
                Provider::CodexExec.default_effort(),
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
            &SandboxLayout::default(),
            Path::new("codex"),
            Provider::CodexExec.default_model(),
            Provider::CodexExec.default_effort(),
        );
        assert_eq!(profile.message_format, MessageFormat::PlainLine);
        let encoded = profile.encode_message("one\ntwo");
        assert_eq!(encoded, b"one two\n");
    }
}
