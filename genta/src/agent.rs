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

impl SandboxLayout {
    /// Use `workspace` as its own destination inside the isolation.
    ///
    /// This is appropriate when the host directory is a durable, canonical
    /// project path. Hosts that construct ephemeral worktrees should keep using
    /// an explicit fixed layout instead.
    pub fn same_path(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }
}

/// Which coding agent a session launches, and thus which command line and wire
/// protocol it gets. The model and reasoning effort are chosen separately (see
/// [`Selection`]); a provider is only the agent itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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

    /// The wire protocol (and therefore provider-specific presentation rules)
    /// used by this provider.
    pub fn protocol(&self) -> Protocol {
        match self {
            Provider::Codex => Protocol::CodexAppServer,
            Provider::CodexExec => Protocol::CodexJsonl,
            Provider::Claude => Protocol::ClaudeJsonl,
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

    /// The model a [`Selection`] takes when a profile name omits one.
    ///
    /// Declared here rather than read off the front of [`Provider::models`], so
    /// that reordering the catalog cannot silently move every unpinned launch to
    /// a different model — and so the choice can differ from the catalog's lead.
    /// It does for Claude Code: the catalog leads with `claude-fable-5`, but
    /// that tier is priced above Opus, so an operator who named no model gets
    /// `claude-opus-5` instead.
    pub fn default_model(&self) -> &'static str {
        match self {
            // Styra's interactive Codex sessions default to the balanced
            // Terra profile. Batch Codex execution keeps its own explicit
            // defaults below.
            Provider::Codex => "gpt-5.6-terra",
            Provider::CodexExec => "gpt-5.6-sol",
            Provider::Claude => "claude-opus-5",
        }
    }

    /// The reasoning effort a [`Selection`] takes when a profile name omits one.
    /// Interactive Codex uses `medium`; the other providers retain their own
    /// declared `high` default.
    pub fn default_effort(&self) -> Effort {
        match self {
            Provider::Codex => Effort::Medium,
            Provider::CodexExec | Provider::Claude => Effort::High,
        }
    }
}

/// How much reasoning the model is asked to spend per turn.
///
/// One vocabulary across providers, since the ladders coincide in the middle;
/// [`Provider::efforts`] narrows it to what a given agent accepts. Passed to
/// codex as its `model_reasoning_effort` config override and to Claude Code as
/// `--effort`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

/// What an operator picked to launch: an agent, a model, and a reasoning effort.
///
/// All three are always present. A selection never leaves the model or effort to
/// whatever the agent happens to be configured for, because that configuration is
/// invisible to Genta and to anything reading a journal afterwards — a session
/// recorded as plain `codex` says nothing about what actually ran. A profile name
/// that omits either therefore takes this provider's declared default
/// ([`Provider::default_model`], [`Provider::default_effort`]) rather than
/// standing for "unset".
///
/// A selection round-trips through one string, [`Selection::name`], of the form
/// `provider:model/effort` — `codex:gpt-5.6-terra/medium`,
/// `claude:claude-opus-5/xhigh`. That string is the profile name, so it is also
/// what a journal records and a status line shows: a stored session states which
/// model and effort ran, and re-parsing it reproduces the launch. Parsing accepts
/// the shorter `provider[:model][/effort]` forms and fills in the defaults, so
/// `--profile claude` still works and names itself fully afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub provider: Provider,
    pub model: String,
    pub effort: Effort,
}

impl Selection {
    /// A provider on its declared defaults.
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            model: provider.default_model().to_owned(),
            effort: provider.default_effort(),
        }
    }

    /// Parse a profile name of the form `provider[:model][/effort]`, filling an
    /// omitted model or effort from the provider's declared defaults.
    pub fn parse(name: &str) -> Result<Selection> {
        let (head, effort) = match name.split_once('/') {
            Some((head, effort)) => (head, Some(Effort::parse(effort.trim())?)),
            None => (name, None),
        };
        let (provider, model) = match head.split_once(':') {
            Some((provider, model)) => {
                let model = model.trim();
                if model.is_empty() {
                    bail!("empty model in profile {name:?}; use e.g. claude:claude-opus-5");
                }
                (provider, Some(model.to_owned()))
            }
            None => (head, None),
        };
        let provider = Provider::parse(provider.trim())?;
        Ok(Selection {
            provider,
            model: model.unwrap_or_else(|| provider.default_model().to_owned()),
            effort: effort.unwrap_or_else(|| provider.default_effort()),
        })
    }

    /// The canonical profile name for this selection; see [`Selection`].
    pub fn name(&self) -> String {
        format!(
            "{}:{}/{}",
            self.provider.as_str(),
            self.model,
            self.effort.as_str()
        )
    }

    /// Build the launchable profile, locating the agent on the host's `PATH`.
    pub fn resolve(&self, layout: &SandboxLayout) -> Result<Profile> {
        let search = std::env::var_os("PATH").unwrap_or_default();
        self.resolve_on_path(layout, &search)
    }

    /// [`Selection::resolve`] against an explicit executable search path.
    pub fn resolve_on_path(&self, layout: &SandboxLayout, search: &OsStr) -> Result<Profile> {
        let executable = resolve_executable_on_path(Path::new(self.provider.executable()), search)?;
        let model = self.model.as_str();
        let effort = self.effort;
        Ok(match self.provider {
            Provider::Codex => codex_appserver(layout, &executable, model, effort),
            Provider::CodexExec => codex(layout, &executable, model, effort),
            Provider::Claude => claude(layout, &executable, model, effort),
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
    /// The name is a [`Selection`]: `provider[:model][/effort]`. Every profile
    /// pins both a model and a reasoning effort — a name that omits either takes
    /// the provider's declared default ([`Provider::default_model`],
    /// [`Provider::default_effort`]), so the resolved profile's own name states
    /// what it launched. `codex:gpt-5.6-terra/xhigh` pins both explicitly.
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

    /// Configure this profile to reopen an existing provider conversation.
    ///
    /// Codex resumes through its app-server protocol, so its command line is
    /// unchanged and the host passes the id to `AppServer`. Claude Code
    /// resumes at process launch time.
    pub fn resume(&mut self, provider: Provider, provider_session_id: &str) -> Result<()> {
        if provider_session_id.trim().is_empty() {
            bail!("cannot resume an empty provider session id");
        }
        match provider {
            Provider::Codex => Ok(()),
            Provider::Claude => {
                self.command.push("--resume".into());
                self.command.push(provider_session_id.to_owned());
                Ok(())
            }
            Provider::CodexExec => {
                bail!("provider codex-exec does not support resuming sessions")
            }
        }
    }
}

/// Which agent produced a session, recorded so a host can persist it
/// alongside a session's journal and later know what to decode the journal
/// with — and, for a human reading the store, which agent and model actually
/// ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMeta {
    /// The provider, model, and effort that launched the session.
    pub selection: Selection,
    /// The wire protocol the agent speaks, and thus the decoder its journal
    /// must be replayed with.
    pub protocol: Protocol,
}

impl SessionMeta {
    /// Capture the provenance of a session launch.
    pub fn new(selection: Selection, protocol: Protocol) -> Self {
        Self {
            selection,
            protocol,
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
    if name
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        if is_executable_file(name) {
            return Ok(name.to_path_buf());
        }
        bail!(
            "agent executable {} is not an executable file",
            name.display()
        );
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
/// `model` and `effort` are always pinned — a profile does not leave either to
/// codex's own configuration, so that what ran is recorded rather than inferred
/// (see [`Selection`]). Both are `-c` config overrides, so they are passed to the
/// `codex` process itself rather than to `app-server`, which inherits them for
/// every thread it starts.
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

/// The name a launched profile carries: exactly [`Selection::name`], so a
/// recorded profile always states the model and effort it ran and re-parses into
/// the selection that produced it.
fn profile_name(provider: Provider, model: &str, effort: Effort) -> String {
    Selection {
        provider,
        model: model.to_owned(),
        effort,
    }
    .name()
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
/// [`resolve_executable`].
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
/// located by [`resolve_executable`]: the common install puts it in
/// `~/.local/bin`, which the sandbox's `PATH` does not contain.
///
/// NOTE: the exact flags and the `stream-json` envelope in [`claude_submission`]
/// must be confirmed against the installed `claude` version; both are isolated
/// here so adapting to a different contract is a localized change plus, if the
/// event schema differs, the [`Protocol::ClaudeJsonl`](crate::event::Protocol)
/// decoder.
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
        let directory =
            std::env::temp_dir().join(format!("genta-agent-bin-{}", std::process::id()));
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
        assert!(profile
            .command
            .iter()
            .any(|arg| arg == r#"model_reasoning_effort="medium""#));
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

    #[test]
    fn unknown_profile_is_rejected() {
        assert!(builtin("gpt5", &SandboxLayout::default()).is_err());
    }

    #[test]
    fn session_meta_captures_the_selection_and_survives_json_round_trip() {
        let selection = Selection {
            provider: Provider::Claude,
            model: "opus".into(),
            effort: Effort::High,
        };
        let meta = SessionMeta::new(selection.clone(), Protocol::ClaudeJsonl);
        assert_eq!(meta.selection, selection);
        assert_eq!(meta.protocol, Protocol::ClaudeJsonl);

        let json = serde_json::to_string(&meta).unwrap();
        let restored: SessionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, meta);
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
    fn empty_claude_model_suffix_is_rejected() {
        assert!(builtin("claude:", &SandboxLayout::default()).is_err());
    }

    /// A selection is the profile name: what a picker composes, what a journal
    /// records, and what re-parses into the same launch. A fully-named profile
    /// round-trips unchanged.
    #[test]
    fn a_selection_round_trips_through_its_profile_name() {
        for (name, expected) in [
            (
                "codex:gpt-5.6-sol/xhigh",
                Selection {
                    provider: Provider::Codex,
                    model: "gpt-5.6-sol".into(),
                    effort: Effort::XHigh,
                },
            ),
            (
                "claude:claude-opus-5/max",
                Selection {
                    provider: Provider::Claude,
                    model: "claude-opus-5".into(),
                    effort: Effort::Max,
                },
            ),
            (
                "codex-exec:gpt-5.6-luna/minimal",
                Selection {
                    provider: Provider::CodexExec,
                    model: "gpt-5.6-luna".into(),
                    effort: Effort::Minimal,
                },
            ),
        ] {
            let parsed = Selection::parse(name).unwrap();
            assert_eq!(parsed, expected, "parsing {name:?}");
            assert_eq!(parsed.name(), name, "round-tripping {name:?}");
        }
    }

    /// A selection may not leave the model or effort unpinned: a shorter profile
    /// name takes the provider's declared defaults, and then names itself in
    /// full, so what a journal records is never "whatever the agent was set to".
    #[test]
    fn a_shorter_profile_name_takes_the_declared_defaults() {
        for (short, full) in [
            ("codex", "codex:gpt-5.6-terra/medium"),
            ("claude", "claude:claude-opus-5/high"),
            ("codex-exec", "codex-exec:gpt-5.6-sol/high"),
            // Whichever half is given is kept; only the missing half defaults.
            (
                "claude:claude-haiku-4-5-20251001",
                "claude:claude-haiku-4-5-20251001/high",
            ),
            ("codex/minimal", "codex:gpt-5.6-terra/minimal"),
        ] {
            let parsed = Selection::parse(short).unwrap();
            assert_eq!(parsed.name(), full, "{short:?} should fill out to {full:?}");
            // Filling in a default is idempotent — the full name re-parses to it.
            assert_eq!(Selection::parse(full).unwrap(), parsed);
        }

        // `Selection::new` is the same defaults by another route.
        for provider in Provider::ALL {
            let selection = Selection::new(provider);
            assert_eq!(selection.model, provider.default_model());
            assert_eq!(selection.effort, provider.default_effort());
            assert_eq!(Selection::parse(provider.as_str()).unwrap(), selection);
        }
    }

    /// Each declared default must be launchable as a model id and effort.
    #[test]
    fn the_declared_defaults_are_offered_by_their_provider() {
        for provider in Provider::ALL {
            assert!(
                provider.efforts().contains(&provider.default_effort()),
                "{provider:?} cannot run its own default effort"
            );
            assert!(
                provider.models().contains(&provider.default_model()),
                "{provider:?} default model is outside its own catalog"
            );
        }
        // Claude Code's default is deliberately not the catalog's lead: the
        // flagship tier is priced above Opus.
        assert_eq!(Provider::Claude.default_model(), "claude-opus-5");
        assert_ne!(
            Provider::Claude.default_model(),
            Provider::Claude.models()[0]
        );
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
        for name in [
            "codex:gpt-5.6-sol/high",
            "claude:claude-opus-5/max",
            "codex-exec:gpt-5.6-terra/low",
        ] {
            let profile = builtin(name, &layout).unwrap();
            assert_eq!(profile.name, name);
        }
        // A shorter name resolves to a profile named after the defaults it took,
        // so `--profile claude` still records the model that ran.
        for short in ["codex", "claude", "codex-exec"] {
            let selection = Selection::parse(short).unwrap();
            let profile = builtin(short, &layout).unwrap();
            assert_eq!(profile.name, selection.name());
            assert_ne!(profile.name, short);
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
            ..codex(
                &SandboxLayout::default(),
                Path::new("codex"),
                Provider::CodexExec.default_model(),
                Provider::CodexExec.default_effort(),
            )
        };
        let encoded = profile.encode_message("one\ntwo");
        assert_eq!(encoded, b"one two\n");
    }
}
