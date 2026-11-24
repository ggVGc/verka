//! Codex: app-server conversations and one-shot exec batches.
//!
//! One provider, with a transport chosen by execution mode. Both transports
//! share a model catalog, defaults, and isolation policy.

use super::spec::{ModelSpec, ProviderOps, ProviderSpec};
use super::{
    profile_name, Effort, ExecutionMode, MessageFormat, MountSpec, Profile, Provider, SandboxLayout,
};
use crate::event::Protocol;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The executable used for both execution modes.
const EXECUTABLE: &str = "codex";

/// Models worth offering in a picker, most capable first.
///
/// The models the installed codex catalog marks `visibility: list` (read out
/// of codex-cli 0.156.1 on 2026-09-24), newest tier first. The hidden ones are
/// deliberately absent: `gpt-5.4` and `codex-auto-review` are not offered to
/// operators at all, and the `gpt-daybreak-*` pair are cyber-specialty
/// variants rather than general coding models.
const MODELS: &[ModelSpec] = &[
    current("gpt-6-astra"),
    current("gpt-6.1-sol"),
    current("gpt-6-sol"),
    current("gpt-5.6-sol"),
    current("gpt-5.6-terra"),
    current("gpt-5.6-luna"),
    ModelSpec {
        id: "gpt-5.5",
        // Hidden from the picker (see above), but on the same legacy ladder.
        aliases: &["gpt-5.4"],
        efforts: LEGACY_EFFORTS,
    },
    // Not an OpenAI model: codex reaches it through a model provider of its
    // own, which the operator configures. Its ladder is unverified, so it is
    // given the current one, as any unlisted id would be.
    current("deepseek/deepseek-v4.1-flash"),
];

/// A catalog entry on the current ladder.
const fn current(id: &'static str) -> ModelSpec {
    ModelSpec {
        id,
        aliases: &[],
        efforts: EFFORTS,
    }
}

/// The current codex ladder, as every `gpt-6*` and `gpt-5.6-*` model declares
/// it, and so the provider's widest one. Genta's `minimal` rung is on none of
/// them.
///
/// Incomplete in one known way: the catalog gives `gpt-6-astra`, `gpt-6-sol`,
/// `gpt-5.6-sol`, and `gpt-5.6-terra` a sixth rung above `max`, `ultra`, which
/// [`Effort`] cannot spell. Until it can, those models are offered one rung
/// short rather than offered a rung they would reject.
const EFFORTS: &[Effort] = &[
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::XHigh,
    Effort::Max,
];

/// The ladder the legacy codex models kept: no `max`.
const LEGACY_EFFORTS: &[Effort] = &[Effort::Low, Effort::Medium, Effort::High, Effort::XHigh];

/// The least expensive codex model, for the incidental errands a host runs
/// around a session rather than for the session's own work.
const CHEAPEST_MODEL: &str = "gpt-5.6-luna";

static OPS: Ops = Ops;

/// Codex identity and defaults shared by conversation and batch execution.
pub(super) static SPEC: ProviderSpec = ProviderSpec {
    name: "codex",
    executable: EXECUTABLE,
    protocol: Protocol::CodexAppServer,
    models: MODELS,
    efforts: EFFORTS,
    default_model: "gpt-6.1-sol",
    default_effort: Effort::Medium,
    cheapest_model: CHEAPEST_MODEL,
    ops: &OPS,
};

struct Ops;

impl ProviderOps for Ops {
    fn profile(
        &self,
        mode: ExecutionMode,
        layout: &SandboxLayout,
        executable: &Path,
        model: &str,
        effort: Effort,
    ) -> Profile {
        codex(mode, layout, executable, model, effort)
    }

    /// A conversation resumes through the host's app-server handshake, so its
    /// command line is left alone. A single-turn profile resumes with
    /// `codex exec resume ID -`: the exec subcommand's own resume, which takes
    /// the same options and reads the prompt from stdin the same way.
    fn resume(&self, profile: &mut Profile, provider_session_id: &str) -> Result<()> {
        if !profile.single_turn {
            return Ok(());
        }
        let exec = profile
            .command
            .iter()
            .position(|argument| argument == "exec")
            .context("a single-turn codex profile runs `codex exec`")?;
        let prompt = profile.command.len() - 1;
        profile
            .command
            .insert(prompt, provider_session_id.to_owned());
        profile.command.insert(exec + 1, "resume".into());
        Ok(())
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
        ..codex_exec(layout, executable, "-", model, effort)
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

/// Build the Codex profile for the host's execution mode. Both modes share
/// credentials, isolation policy, model, and effort; the command and wire
/// protocol follow the mode. Batch prompts are read from stdin.
pub fn codex(
    mode: ExecutionMode,
    layout: &SandboxLayout,
    executable: &Path,
    model: &str,
    effort: Effort,
) -> Profile {
    match mode {
        ExecutionMode::Conversation => codex_appserver(layout, executable, model, effort),
        ExecutionMode::Batch => codex_exec(layout, executable, "-", model, effort),
    }
}

/// Build a complete single-turn Codex profile with a host-selected executable
/// and prompt argument.
///
/// Hosts normally resolve a [`Selection`](super::Selection) in batch mode,
/// which reads the prompt from stdin. A batch orchestrator may instead stage
/// its full prompt elsewhere and pass a short
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
        name: profile_name(Provider::Codex, model, effort),
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
mod tests;
