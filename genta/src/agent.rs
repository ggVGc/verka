//! Agent profiles: how each coding agent is launched and spoken to.
//!
//! A profile names the isolated command, the wire protocol it speaks, the
//! sandbox policy it needs, and how an operator message is encoded as one
//! protocol input line. The host's executor (Driva) stays an uninterpreted
//! transport; interpretation of the streams belongs here and in
//! [`crate::event`].

use crate::event::Protocol;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A host path exposed at an isolated destination, translated by the host into
/// its executor's bind-mount spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub writable: bool,
}

/// Stable paths inside one isolated agent session. The workspace is where the
/// operator's project (or a throwaway worktree) is mounted writable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxLayout {
    pub workspace: PathBuf,
}

impl Default for SandboxLayout {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("/tmp/styra/workspace"),
        }
    }
}

/// Which coding agent a session launches, and thus which command line and wire
/// protocol it gets. The model and reasoning effort are chosen separately (see
/// [`Selection`]); a provider is only the agent itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Multi-turn codex over the `app-server` JSON-RPC protocol.
    Codex,
    /// One-shot `codex exec --json`.
    CodexExec,
    /// Multi-turn Claude Code over bidirectional `stream-json`.
    Claude,
}

impl Provider {
    /// Every provider, in the order a picker should offer them.
    pub const ALL: [Provider; 3] = [Provider::Codex, Provider::CodexExec, Provider::Claude];

    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::CodexExec => "codex-exec",
            Provider::Claude => "claude",
        }
    }

    pub fn parse(name: &str) -> Result<Provider> {
        Provider::ALL
            .into_iter()
            .find(|provider| provider.as_str() == name)
            .with_context(|| {
                format!(
                    "unknown agent provider {name:?}; known providers: {}",
                    Provider::ALL.map(|provider| provider.as_str()).join(", ")
                )
            })
    }

    /// The agent's own executable name, as located on the host's `PATH`.
    fn executable(&self) -> &'static str {
        match self {
            Provider::Codex | Provider::CodexExec => "codex",
            Provider::Claude => "claude",
        }
    }

    /// Models worth offering in a picker, most capable first.
    ///
    /// Not a closed set: both agents accept any model id they know, so a
    /// [`Selection`] still carries a free-form string. What is *installed* — and
    /// which ids the operator's account may use — is the agent's business, not
    /// Genta's; an unknown model fails in the agent, where the authoritative
    /// catalog lives.
    ///
    /// The Claude ids are every model listed `Active` in Anthropic's model-status
    /// table (<https://platform.claude.com/docs/en/about-claude/model-deprecations>),
    /// read on 2026-07-27, in that table's order — tier by tier, newest first.
    /// Two knowingly-excluded classes: `claude-opus-4-1-20250805` is
    /// `Deprecated` there (retires 2026-08-05), and `claude-mythos-5` is
    /// reachable only through Project Glasswing, so offering it to every
    /// operator would suggest an agent most cannot launch. Full ids rather than
    /// the `opus`/`sonnet` aliases, so a journal records the exact model a
    /// session ran on even after an alias moves to a newer release.
    pub fn models(&self) -> &'static [&'static str] {
        match self {
            Provider::Codex | Provider::CodexExec => {
                &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]
            }
            Provider::Claude => &[
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
            ],
        }
    }

    /// The reasoning-effort levels this provider accepts, lowest first. The two
    /// agents' ladders differ at the ends: codex has a `minimal` rung, Claude
    /// Code a `max` one.
    pub fn efforts(&self) -> &'static [Effort] {
        match self {
            Provider::Codex | Provider::CodexExec => &[
                Effort::Minimal,
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::XHigh,
            ],
            Provider::Claude => &[
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::XHigh,
                Effort::Max,
            ],
        }
    }
}

/// How much reasoning the model is asked to spend per turn.
///
/// One vocabulary across providers, since the ladders coincide in the middle;
/// [`Provider::efforts`] narrows it to what a given agent accepts. Passed to
/// codex as its `model_reasoning_effort` config override and to Claude Code as
/// `--effort`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }

    pub fn parse(name: &str) -> Result<Effort> {
        const ALL: [Effort; 6] = [
            Effort::Minimal,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
        ];
        ALL.into_iter()
            .find(|effort| effort.as_str() == name)
            .with_context(|| {
                format!(
                    "unknown reasoning effort {name:?}; known levels: {}",
                    ALL.map(|effort| effort.as_str()).join(", ")
                )
            })
    }
}

/// What an operator picked to launch: an agent, optionally a model, optionally
/// a reasoning effort. An absent model or effort leaves the agent on whatever
/// it is configured for itself, which is not the same as any level Genta could
/// name — so it stays absent rather than being defaulted here.
///
/// A selection round-trips through one string, [`Selection::name`], of the form
/// `provider[:model][/effort]` (`codex`, `claude:opus`,
/// `codex:gpt-5.6-sol/xhigh`). That string is the profile name, so it is also
/// what a journal records and a status line shows: a stored session says which
/// model and effort actually ran, and re-parsing it reproduces the launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

impl Selection {
    /// A provider on its own defaults: no model or effort override.
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            model: None,
            effort: None,
        }
    }

    /// Parse a profile name of the form `provider[:model][/effort]`.
    pub fn parse(name: &str) -> Result<Selection> {
        let (head, effort) = match name.split_once('/') {
            Some((head, effort)) => (head, Some(Effort::parse(effort.trim())?)),
            None => (name, None),
        };
        let (provider, model) = match head.split_once(':') {
            Some((provider, model)) => {
                let model = model.trim();
                if model.is_empty() {
                    bail!("empty model in profile {name:?}; use e.g. claude:opus");
                }
                (provider, Some(model.to_owned()))
            }
            None => (head, None),
        };
        Ok(Selection {
            provider: Provider::parse(provider.trim())?,
            model,
            effort,
        })
    }

    /// The canonical profile name for this selection; see [`Selection`].
    pub fn name(&self) -> String {
        let mut name = self.provider.as_str().to_owned();
        if let Some(model) = &self.model {
            name.push(':');
            name.push_str(model);
        }
        if let Some(effort) = self.effort {
            name.push('/');
            name.push_str(effort.as_str());
        }
        name
    }

    /// Build the launchable profile, locating the agent on the host's `PATH`.
    pub fn resolve(&self, layout: &SandboxLayout) -> Result<Profile> {
        let search = std::env::var_os("PATH").unwrap_or_default();
        self.resolve_on_path(layout, &search)
    }

    /// [`Selection::resolve`] against an explicit executable search path.
    pub fn resolve_on_path(&self, layout: &SandboxLayout, search: &OsStr) -> Result<Profile> {
        let executable = resolve_executable_on_path(Path::new(self.provider.executable()), search)?;
        let model = self.model.as_deref();
        Ok(match self.provider {
            Provider::Codex => codex_appserver(layout, &executable, model, self.effort),
            Provider::CodexExec => codex(layout, &executable, model, self.effort),
            Provider::Claude => claude(layout, &executable, model, self.effort),
        })
    }
}

/// How an operator message becomes one line written to the agent's stdin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageFormat {
    /// A codex protocol submission envelope carrying the text as a user turn.
    CodexSubmission,
    /// A Claude Code `stream-json` user message envelope. Newlines survive as
    /// JSON string escapes, so the envelope is still exactly one input line.
    ClaudeStreamJson,
    /// The bare message text as a single line, for agents that read plain
    /// stdin turns.
    PlainLine,
}

/// Everything a host needs to launch and drive one agent. The workspace bind
/// mount is added by the session from the operator's `--workspace`; the profile
/// contributes only its own agent-specific mounts (credentials, state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub command: Vec<String>,
    pub protocol: Protocol,
    pub mounts: Vec<MountSpec>,
    pub environment: BTreeMap<String, String>,
    pub network: bool,
    pub message_format: MessageFormat,
    /// The agent reads one prompt to end-of-input, then runs to completion (a
    /// one-shot `exec` agent). The session closes stdin after the operator's
    /// message so the turn can start; a further message is not possible.
    pub single_turn: bool,
}

impl Profile {
    /// Resolve a built-in profile by name, locating its agent binary on the
    /// host's `PATH`.
    ///
    /// The name is a [`Selection`]: `provider[:model][/effort]`. A bare
    /// provider (`codex`, `codex-exec`, `claude`) leaves the agent on its own
    /// configured model and reasoning effort; `claude:opus` pins a model, and
    /// `codex:gpt-5.6-sol/xhigh` pins both.
    pub fn builtin(name: &str, layout: &SandboxLayout) -> Result<Profile> {
        Selection::parse(name)?.resolve(layout)
    }

    /// [`Profile::builtin`] against an explicit executable search path.
    ///
    /// Hosts use [`Profile::builtin`], which searches their own `PATH`; a
    /// caller that knows where its agents live (or a test that must not depend
    /// on what happens to be installed) supplies the search path here.
    pub fn builtin_on_path(name: &str, layout: &SandboxLayout, search: &OsStr) -> Result<Profile> {
        Selection::parse(name)?.resolve_on_path(layout, search)
    }

    /// Encode an operator message as one newline-terminated protocol input line.
    pub fn encode_message(&self, text: &str) -> Vec<u8> {
        let mut line = match self.message_format {
            MessageFormat::CodexSubmission => codex_submission(text),
            MessageFormat::ClaudeStreamJson => claude_submission(text),
            MessageFormat::PlainLine => text.replace('\n', " "),
        };
        line.push('\n');
        line.into_bytes()
    }
}

/// Which agent produced a session, recorded so a host can persist it
/// alongside a session's journal and later know what to decode the journal
/// with — and, for a human reading the store, which agent and model actually
/// ran — without depending on an operator re-supplying the same profile name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// The profile name that launched the session (e.g. `codex`, `claude:opus`).
    pub profile: String,
    /// The wire protocol the agent speaks, and thus the decoder its journal
    /// must be replayed with.
    pub protocol: Protocol,
}

impl SessionMeta {
    /// Capture the provenance of a session launched with `profile`.
    pub fn for_profile(profile: &Profile) -> Self {
        Self {
            profile: profile.name.clone(),
            protocol: profile.protocol,
        }
    }
}

/// Locate the agent binary `name` on the host's `PATH`, as [`Profile::builtin`]
/// does.
///
/// A profile's command must name a binary that resolves *inside* the sandbox,
/// where the isolation backend clears the environment and supplies a fixed
/// system `PATH`. An operator's own `PATH` entries — `~/.local/bin`, a version
/// manager's shims — are not part of it, so a bare `claude` or `codex` there
/// fails deep inside the sandbox with an opaque `execvp: No such file or
/// directory`. Resolving on the host instead pins the exact binary the operator
/// would have run (the sandbox binds the host root, so the path is valid on both
/// sides) and turns a missing agent into a clear error before any isolation is
/// built.
pub fn resolve_executable(name: &Path) -> Result<PathBuf> {
    let search = std::env::var_os("PATH").unwrap_or_default();
    resolve_executable_on_path(name, &search)
}

/// [`resolve_executable`] against an explicit `PATH`-shaped search path.
///
/// A `name` containing a separator is a path already and is only checked, not
/// searched for — matching `execvp`. Symlinks are followed to check the target,
/// but the returned path is the one given: a launcher symlink is what the
/// operator installed, and it resolves the same way inside the sandbox.
pub fn resolve_executable_on_path(name: &Path, search: &OsStr) -> Result<PathBuf> {
    if name.parent().is_some_and(|parent| !parent.as_os_str().is_empty()) {
        if is_executable_file(name) {
            return Ok(name.to_path_buf());
        }
        bail!("agent executable {} is not an executable file", name.display());
    }
    std::env::split_paths(search)
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable_file(candidate))
        .with_context(|| {
            format!(
                "agent executable {} was not found on PATH ({}); install it or configure an absolute path",
                name.display(),
                search.to_string_lossy(),
            )
        })
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
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
/// `model` and `effort` are the operator's overrides; either absent leaves codex
/// on its own configured default. Both are `-c` config overrides, so they are
/// passed to the `codex` process itself rather than to `app-server`, which
/// inherits them for every thread it starts.
pub fn codex_appserver(
    layout: &SandboxLayout,
    executable: &Path,
    model: Option<&str>,
    effort: Option<Effort>,
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

/// The canonical profile name for a launch, matching [`Selection::name`] so a
/// recorded profile re-parses into the selection that produced it.
fn profile_name(provider: Provider, model: Option<&str>, effort: Option<Effort>) -> String {
    Selection {
        provider,
        model: model.map(str::to_owned),
        effort,
    }
    .name()
}

/// The `-c` overrides that pin codex's model and reasoning effort, in the order
/// they appear on the command line. Values are quoted because `-c` parses them
/// as TOML, where a bare model id is not a valid scalar.
fn codex_model_overrides(model: Option<&str>, effort: Option<Effort>) -> Vec<String> {
    let mut overrides = Vec::new();
    if let Some(model) = model {
        overrides.push("-c".into());
        overrides.push(format!("model={model:?}"));
    }
    if let Some(effort) = effort {
        overrides.push("-c".into());
        overrides.push(format!("model_reasoning_effort={:?}", effort.as_str()));
    }
    overrides
}

/// The built-in single-turn codex profile.
///
/// Isolation follows Orka's proven codex shape: the workspace is trusted so
/// codex does not prompt, its inner sandbox is disabled in favour of Driva's
/// outer Bubblewrap isolation, `~/.codex/auth.json` is mounted writable so
/// credential refreshes persist, and stable `HOME`/`TERM` are set because
/// Bubblewrap clears the environment.
///
/// The command is `codex exec --json -`: a single-turn run that reads the
/// prompt from stdin and streams `thread`/`turn`/`item` events, verified
/// against codex-cli 0.145.
///
/// `executable` is the codex binary to launch, as located by
/// [`resolve_executable`].
pub fn codex(
    layout: &SandboxLayout,
    executable: &Path,
    model: Option<&str>,
    effort: Option<Effort>,
) -> Profile {
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
    model: Option<&str>,
    effort: Option<Effort>,
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
        // /root existing in the host rootfs. The auth file is bound in below it.
        mounts: vec![MountSpec {
            source: "~/.codex/auth.json".into(),
            destination: "/tmp/agent-home/.codex/auth.json".into(),
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
    model: Option<&str>,
    effort: Option<Effort>,
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
fn codex_submission(text: &str) -> String {
    let submission = serde_json::json!({
        "id": submission_id(),
        "op": {
            "type": "user_input",
            "items": [{ "type": "text", "text": text }],
        }
    });
    submission.to_string()
}

/// The built-in Claude Code interactive profile.
///
/// The isolation mirrors the codex shape: Driva's outer Bubblewrap sandbox is
/// the boundary, so Claude Code's own permission prompt is skipped with
/// `--dangerously-skip-permissions`. `HOME` lives under `/tmp`, the writable
/// tmpfs Driva always provides, matching the codex profile's rationale: a
/// disposable, always-present home without depending on a particular
/// directory existing in the host rootfs. `~/.claude/.credentials.json` is
/// bound in under it, writable, so a refreshed OAuth token persists across
/// sessions; the rest of the config directory is discarded with the tmpfs.
///
/// The command drives Claude Code's bidirectional `stream-json` mode: it reads
/// `stream-json` user messages on stdin and emits `stream-json` events on
/// stdout, staying alive until stdin closes (so, like the app-server codex
/// profile, it spans many turns rather than running once to completion).
/// `--verbose` is required alongside `--output-format stream-json` under
/// `--print`. An optional `model` becomes a `--model` argument and an optional
/// `effort` an `--effort` one; when either is absent, Claude Code uses its
/// configured default. `executable` is the Claude Code binary to launch, as
/// located by [`resolve_executable`]: the common install puts it in
/// `~/.local/bin`, which the sandbox's `PATH` does not contain.
///
/// NOTE: the exact flags and the `stream-json` envelope in [`claude_submission`]
/// must be confirmed against the installed `claude` version; both are isolated
/// here so adapting to a different contract is a localized change plus, if the
/// event schema differs, the [`Protocol::ClaudeJsonl`](crate::event::Protocol)
/// decoder.
pub fn claude(
    _layout: &SandboxLayout,
    executable: &Path,
    model: Option<&str>,
    effort: Option<Effort>,
) -> Profile {
    let mut command = vec![
        executable.to_string_lossy().into_owned(),
        "--print".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--dangerously-skip-permissions".into(),
    ];
    if let Some(model) = model {
        command.push("--model".into());
        command.push(model.to_string());
    }
    if let Some(effort) = effort {
        command.push("--effort".into());
        command.push(effort.as_str().into());
    }
    Profile {
        name: profile_name(Provider::Claude, model, effort),
        command,
        protocol: Protocol::ClaudeJsonl,
        mounts: vec![MountSpec {
            source: "~/.claude/.credentials.json".into(),
            destination: "/tmp/agent-home/.claude/.credentials.json".into(),
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
fn claude_submission(text: &str) -> String {
    let submission = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
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
    use serde_json::Value;

    /// A search path holding stub `codex` and `claude` binaries, so profile
    /// resolution is exercised against a known install rather than whatever the
    /// machine running the tests happens to have on its own `PATH`.
    fn agent_bin() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "genta-agent-bin-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["codex", "claude"] {
            let path = directory.join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        directory
    }

    /// The resolved command for `name` under [`agent_bin`].
    fn stub(name: &str) -> String {
        agent_bin().join(name).to_string_lossy().into_owned()
    }

    fn builtin(name: &str, layout: &SandboxLayout) -> Result<Profile> {
        Profile::builtin_on_path(name, layout, agent_bin().as_os_str())
    }

    /// The whole point of resolving on the host: the profile carries a path the
    /// sandbox can exec, not a name that only the operator's own `PATH` finds.
    #[test]
    fn a_profile_launches_the_resolved_executable_path_not_a_bare_name() {
        for name in ["codex", "codex-exec", "claude", "claude:opus"] {
            let profile = builtin(name, &SandboxLayout::default()).unwrap();
            let command = Path::new(&profile.command[0]);
            assert!(
                command.is_absolute(),
                "{name} must launch an absolute path, got {}",
                profile.command[0]
            );
            assert_eq!(command.parent(), Some(agent_bin().as_path()));
        }
    }

    #[test]
    fn resolution_takes_the_first_executable_file_on_the_search_path() {
        let root = std::env::temp_dir().join(format!("genta-resolve-{}", std::process::id()));
        let (empty, decoy, real) = (root.join("empty"), root.join("decoy"), root.join("real"));
        for directory in [&empty, &decoy, &real] {
            std::fs::create_dir_all(directory).unwrap();
        }
        // A same-named file that is not executable must not shadow the install.
        std::fs::write(decoy.join("claude"), "notes about claude").unwrap();
        let installed = real.join("claude");
        std::fs::write(&installed, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();

        let search = std::env::join_paths([&empty, &decoy, &real]).unwrap();
        assert_eq!(
            resolve_executable_on_path(Path::new("claude"), &search).unwrap(),
            installed
        );
    }

    /// A missing agent is a clear error at profile construction, not an opaque
    /// `execvp: No such file or directory` from inside the sandbox.
    #[test]
    fn a_missing_agent_is_reported_before_any_isolation_is_built() {
        let error = Profile::builtin_on_path("claude", &SandboxLayout::default(), OsStr::new(""))
            .expect_err("an agent that is not installed cannot be launched");
        let message = format!("{error:#}");
        assert!(message.contains("claude"), "{message}");
        assert!(message.contains("not found on PATH"), "{message}");
    }

    /// The common Claude Code install is a launcher symlink into a versioned
    /// directory. Both are visible inside the sandbox, so the link is kept: it is
    /// what the operator installed and what an update repoints.
    #[test]
    fn an_explicit_executable_is_used_as_given_including_a_launcher_symlink() {
        let root = std::env::temp_dir().join(format!("genta-symlink-{}", std::process::id()));
        let (bin, versions) = (root.join("bin"), root.join("versions"));
        for directory in [&bin, &versions] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let versioned = versions.join("2.1.219");
        std::fs::write(&versioned, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&versioned, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launcher = bin.join("claude");
        let _ = std::fs::remove_file(&launcher);
        std::os::unix::fs::symlink(&versioned, &launcher).unwrap();

        assert_eq!(
            resolve_executable_on_path(&launcher, OsStr::new("")).unwrap(),
            launcher,
            "the symlink is followed to check the target, but not resolved away"
        );
    }

    #[test]
    fn an_explicit_executable_that_is_not_a_program_is_rejected() {
        let path = std::env::temp_dir().join(format!("genta-not-a-program-{}", std::process::id()));
        std::fs::write(&path, "not executable").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let error = resolve_executable_on_path(&path, OsStr::new(""))
            .expect_err("a non-executable file is not an agent");
        assert!(format!("{error:#}").contains("not an executable file"));
    }

    #[test]
    fn codex_exec_profile_isolates_the_workspace_and_speaks_the_decoded_protocol() {
        let layout = SandboxLayout::default();
        let profile = builtin("codex-exec", &layout).unwrap();

        assert_eq!(profile.protocol, Protocol::CodexJsonl);
        assert!(profile.network);
        assert!(profile.single_turn);
        assert_eq!(profile.command[0], stub("codex"));
        assert!(profile.command.iter().any(|arg| arg == "danger-full-access"));
        assert!(profile.command.iter().any(|arg| arg == "exec"));
        assert!(profile.command.iter().any(|arg| arg == "--json"));
        assert_eq!(profile.command.last().unwrap(), "-", "prompt is read from stdin");
        assert!(profile
            .command
            .iter()
            .any(|arg| arg.contains("/tmp/styra/workspace") && arg.contains("trusted")));
        assert!(profile.mounts.iter().any(|mount| {
            mount.destination == std::path::Path::new("/tmp/agent-home/.codex/auth.json")
                && mount.writable
        }));
        assert_eq!(profile.environment.get("HOME"), Some(&"/tmp/agent-home".to_string()));
    }

    #[test]
    fn codex_exec_profile_accepts_host_selected_executable_and_prompt() {
        let layout = SandboxLayout {
            workspace: "/tmp/orka/workspace".into(),
        };
        let profile = codex_exec(&layout, Path::new("/opt/codex"), "read the staged prompt", None, None);

        assert_eq!(profile.command[0], "/opt/codex");
        assert_eq!(profile.command.last().unwrap(), "read the staged prompt");
        assert!(profile.command.iter().any(|argument| {
            argument == "projects.\"/tmp/orka/workspace\".trust_level=\"trusted\""
        }));
        assert_eq!(profile.protocol, Protocol::CodexJsonl);
        assert_eq!(profile.environment["HOME"], "/tmp/agent-home");
        assert_eq!(
            profile.mounts[0].destination,
            PathBuf::from("/tmp/agent-home/.codex/auth.json")
        );
        assert!(profile.network);
        assert!(profile.single_turn);
    }

    #[test]
    fn default_codex_profile_is_the_multi_turn_app_server() {
        let profile = builtin("codex", &SandboxLayout::default()).unwrap();
        assert_eq!(profile.protocol, Protocol::CodexAppServer);
        assert!(!profile.single_turn, "app-server sessions span many turns");
        assert_eq!(profile.command, vec![stub("codex"), "app-server".into()]);
        assert!(profile.network);
        // Isolation policy is shared with the exec profile.
        assert!(profile.mounts.iter().any(|mount| {
            mount.destination == std::path::Path::new("/tmp/agent-home/.codex/auth.json")
        }));
        assert_eq!(profile.environment.get("HOME"), Some(&"/tmp/agent-home".to_string()));
    }

    #[test]
    fn unknown_profile_is_rejected() {
        assert!(builtin("gpt5", &SandboxLayout::default()).is_err());
    }

    #[test]
    fn session_meta_captures_the_launching_profile_and_survives_json_round_trip() {
        let profile = builtin("claude:opus", &SandboxLayout::default()).unwrap();
        let meta = SessionMeta::for_profile(&profile);
        assert_eq!(meta.profile, "claude:opus");
        assert_eq!(meta.protocol, Protocol::ClaudeJsonl);

        let json = serde_json::to_string(&meta).unwrap();
        let restored: SessionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, meta);
    }

    fn codex_submission_profile() -> Profile {
        Profile {
            message_format: MessageFormat::CodexSubmission,
            ..codex(&SandboxLayout::default(), Path::new("codex"), None, None)
        }
    }

    #[test]
    fn claude_profile_speaks_stream_json_and_isolates_credentials() {
        let profile = builtin("claude", &SandboxLayout::default()).unwrap();

        assert_eq!(profile.name, "claude");
        assert_eq!(profile.protocol, Protocol::ClaudeJsonl);
        assert_eq!(profile.message_format, MessageFormat::ClaudeStreamJson);
        assert!(profile.network);
        assert_eq!(profile.command[0], stub("claude"));
        assert!(profile.command.iter().any(|arg| arg == "stream-json"));
        assert!(profile
            .command
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions"));
        // Without an explicit model, no --model argument is passed.
        assert!(!profile.command.iter().any(|arg| arg == "--model"));
        assert!(!profile.single_turn, "an interactive claude session spans many turns");
        assert!(profile.mounts.iter().any(|mount| mount.destination
            == std::path::Path::new("/tmp/agent-home/.claude/.credentials.json")
            && mount.writable));
        assert_eq!(profile.environment.get("HOME"), Some(&"/tmp/agent-home".to_string()));
    }

    /// Claude Code's own aliases (`opus`, `sonnet`) move to whatever the latest
    /// release is, so the catalog offers full ids instead: a journal then records
    /// the exact model a session ran on, and re-parsing that profile reproduces
    /// it rather than silently landing on a newer model.
    #[test]
    fn the_claude_catalog_offers_full_model_ids() {
        for model in Provider::Claude.models() {
            assert!(model.starts_with("claude-"), "{model} is not a full model id");
        }
        assert!(Provider::Claude.models().contains(&"claude-opus-5"));
        // Anthropic lists this one as deprecated, and Mythos is reachable only
        // through Project Glasswing — neither belongs in a catalog offered to
        // every operator.
        assert!(!Provider::Claude.models().contains(&"claude-opus-4-1-20250805"));
        assert!(!Provider::Claude.models().contains(&"claude-mythos-5"));
    }

    #[test]
    fn claude_model_is_selected_by_the_profile_suffix() {
        let profile = builtin("claude:opus", &SandboxLayout::default()).unwrap();
        assert_eq!(profile.name, "claude:opus");
        let model = profile
            .command
            .windows(2)
            .find(|pair| pair[0] == "--model")
            .map(|pair| pair[1].as_str());
        assert_eq!(model, Some("opus"));
    }

    #[test]
    fn empty_claude_model_suffix_is_rejected() {
        assert!(builtin("claude:", &SandboxLayout::default()).is_err());
    }

    /// A selection is the profile name: what a picker composes, what a journal
    /// records, and what re-parses into the same launch.
    #[test]
    fn a_selection_round_trips_through_its_profile_name() {
        for (name, expected) in [
            (
                "codex",
                Selection {
                    provider: Provider::Codex,
                    model: None,
                    effort: None,
                },
            ),
            (
                "claude:opus",
                Selection {
                    provider: Provider::Claude,
                    model: Some("opus".into()),
                    effort: None,
                },
            ),
            (
                "codex-exec/minimal",
                Selection {
                    provider: Provider::CodexExec,
                    model: None,
                    effort: Some(Effort::Minimal),
                },
            ),
            (
                "codex:gpt-5.6-sol/xhigh",
                Selection {
                    provider: Provider::Codex,
                    model: Some("gpt-5.6-sol".into()),
                    effort: Some(Effort::XHigh),
                },
            ),
        ] {
            let parsed = Selection::parse(name).unwrap();
            assert_eq!(parsed, expected, "parsing {name:?}");
            assert_eq!(parsed.name(), name, "round-tripping {name:?}");
        }
    }

    #[test]
    fn an_unknown_provider_or_effort_is_rejected_by_name() {
        for name in ["gpt5", "codex/turbo", "claude:opus/turbo", "codex:/high"] {
            let error = Selection::parse(name)
                .expect_err("an unlaunchable selection must not parse")
                .to_string();
            assert!(
                error.contains("unknown") || error.contains("empty model"),
                "{name:?}: {error}"
            );
        }
    }

    /// The profile name a launch records must be the selection that produced
    /// it, effort included, or a stored session cannot say what actually ran.
    #[test]
    fn a_resolved_profile_is_named_after_its_full_selection() {
        let layout = SandboxLayout::default();
        for name in ["codex", "codex:gpt-5.6-sol/high", "claude:opus/max"] {
            let profile = builtin(name, &layout).unwrap();
            assert_eq!(profile.name, name);
        }
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

    /// Effort ladders are per-provider: only codex has `minimal`, only Claude
    /// Code has `max`. A picker offers what the agent accepts.
    #[test]
    fn each_provider_offers_the_effort_levels_it_accepts() {
        assert!(Provider::Codex.efforts().contains(&Effort::Minimal));
        assert!(!Provider::Codex.efforts().contains(&Effort::Max));
        assert!(Provider::Claude.efforts().contains(&Effort::Max));
        assert!(!Provider::Claude.efforts().contains(&Effort::Minimal));
        for provider in Provider::ALL {
            assert!(!provider.models().is_empty());
            // Every suggestion must be launchable as written: a `Selection`
            // round-trips through one string, so a model id may not carry the
            // grammar's own separators.
            for model in provider.models() {
                assert!(
                    !model.contains(':') && !model.contains('/'),
                    "{model} would not survive Selection::name"
                );
            }
            for effort in provider.efforts() {
                assert_eq!(Effort::parse(effort.as_str()).unwrap(), *effort);
            }
        }
    }

    #[test]
    fn claude_submission_is_valid_json_carrying_the_text_and_one_line() {
        let profile = claude(&SandboxLayout::default(), Path::new("claude"), None, None);
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

    #[test]
    fn plain_line_format_flattens_to_a_single_line() {
        let profile = Profile {
            message_format: MessageFormat::PlainLine,
            ..codex(&SandboxLayout::default(), Path::new("codex"), None, None)
        };
        let encoded = profile.encode_message("one\ntwo");
        assert_eq!(encoded, b"one two\n");
    }
}
