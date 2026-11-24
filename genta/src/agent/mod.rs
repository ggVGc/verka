//! Agent profiles: how each coding agent is launched and spoken to.
//!
//! A profile names the isolated command, the wire protocol it speaks, the
//! sandbox policy it needs, and how an operator message is encoded as one
//! protocol input line. The host's executor (Driva) stays an uninterpreted
//! transport; interpretation of the streams belongs here and in
//! [`crate::event`].
//!
//! This module owns the vocabulary every provider shares — [`Provider`],
//! [`Effort`], [`Selection`], [`Profile`] — while each provider's own facts and
//! command lines live in its own private module (`codex`, `claude`). The two
//! are joined at exactly one point, `Provider::spec`, so adding an agent means
//! writing one module and one match arm rather than editing a dozen matches
//! scattered by operation. The `spec` module states why that single match keeps
//! the guarantee a `match` per question used to give.
//!
//! [`Provider`] stays an enum rather than becoming a trait object, and the
//! localization is arranged around that: it is a `Copy`, serializable value
//! that a [`Selection`] records in a journal, that a picker enumerates through
//! [`Provider::ALL`], and that downstream hosts match on to attach policy of
//! their own (which providers styra offers interactively, say). The model and
//! effort catalogs are not such policy: they are facts about the agents, and
//! [`Provider::models`] is their one statement. What moved into the provider
//! modules is the implementation, not the identity.
//!
//! A note on the enum's doc comments: styra's client generator parses this
//! file's type declarations and copies their doc comments into the Lua and
//! Elixir bindings, so rationale meant for Rust readers belongs here in the
//! module docs rather than on [`Provider`] itself.

mod claude;
mod codex;
mod path;
mod spec;

pub use self::claude::claude;
pub use self::codex::{codex, codex_appserver, codex_exec, codex_exec_command};
pub use self::path::{resolve_executable, resolve_executable_on_path};

pub(crate) use self::claude::claude_submission;

pub use self::spec::ModelSpec;

use self::codex::codex_submission;
use self::spec::ProviderSpec;
use crate::event::Protocol;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

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

/// How a host intends to drive an agent, independently of its identity,
/// model, and reasoning effort. The resolved profile declares the actual wire
/// protocol: Codex uses app-server for conversations and exec for batches;
/// Claude uses stream-json for both (a batch host closes stdin after its prompt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    Conversation,
    Batch,
}

/// Which coding agent a session launches. Model, reasoning effort, and execution
/// mode are chosen separately; a provider is only the agent itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    /// Codex: app-server conversations or one-shot batch execution.
    Codex,
    /// Multi-turn Claude Code over bidirectional `stream-json`.
    Claude,
}

impl Provider {
    /// Every provider, in the order a picker should offer them.
    pub const ALL: [Provider; 2] = [Provider::Codex, Provider::Claude];

    /// What this provider declares about itself.
    ///
    /// The one place a provider is matched on. Every accessor below reads a
    /// field of the returned spec, so a new provider needs one arm here and one
    /// module of its own; a new *question* to ask a provider needs one field
    /// and one answer per module, which the compiler demands at each literal.
    fn spec(&self) -> &'static ProviderSpec {
        match self {
            Provider::Codex => &self::codex::SPEC,
            Provider::Claude => &self::claude::SPEC,
        }
    }

    pub fn as_str(&self) -> &'static str {
        self.spec().name
    }

    /// The wire protocol (and therefore provider-specific presentation rules)
    /// used by this provider in conversation mode. Batch profiles declare
    /// their actual protocol on [`Profile::protocol`].
    pub fn protocol(&self) -> Protocol {
        self.spec().protocol
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
    pub fn executable(&self) -> &'static str {
        self.spec().executable
    }

    /// Models worth offering in a picker, most capable first, each with the
    /// effort ladder it accepts.
    ///
    /// This is the one catalog: hosts build their pickers from it rather than
    /// keeping their own. It is not a closed set, though: both agents accept any
    /// model id they know, so a [`Selection`] still carries a free-form string.
    /// What is *installed* — and which ids the operator's account may use — is
    /// the agent's business, not Genta's; an unknown model fails in the agent,
    /// where the authoritative catalog lives. Each provider's catalog, and how
    /// it was drawn up, is stated with that provider.
    pub fn models(&self) -> &'static [ModelSpec] {
        self.spec().models
    }

    /// The catalog entry for `model`, by id or alias.
    pub fn model(&self, model: &str) -> Option<&'static ModelSpec> {
        self.models().iter().find(|entry| entry.answers_to(model))
    }

    /// Whether this agent could be the one running `model`.
    ///
    /// A catalog is not a closed set — every agent accepts any model id it
    /// knows — so this only rules out ids another agent *declares*. That is
    /// enough for the case it exists for: a session converted from one agent
    /// to the other replays the history it came from, the old agent's own
    /// model reports included, and those name models this one cannot be
    /// running. An unlisted id is nobody's in particular and so is allowed.
    pub fn could_run(&self, model: &str) -> bool {
        self.model(model).is_some()
            || !Provider::ALL
                .iter()
                .any(|provider| provider.model(model).is_some())
    }

    /// The provider's widest effort ladder, lowest first: the union of its
    /// models' ladders, and what a model outside the catalog is assumed to
    /// accept. A launch of a known model is judged by [`Provider::efforts_for`]
    /// instead.
    pub fn efforts(&self) -> &'static [Effort] {
        self.spec().efforts
    }

    /// The reasoning-effort rungs `model` accepts, lowest first.
    ///
    /// Empty means the model takes no effort setting at all — Claude Sonnet 4.5
    /// and Haiku 4.5 predate the parameter and reject it — which is a different
    /// thing from a short ladder. See [`Provider::supports_effort`].
    ///
    /// A model outside the catalog gets the provider's widest ladder: an
    /// unknown id is nobody's in particular, so it is under-constrained rather
    /// than rejected, and the agent itself is the authority that will reject a
    /// rung it does not have.
    pub fn efforts_for(&self, model: &str) -> &'static [Effort] {
        self.model(model)
            .map_or(self.efforts(), |entry| entry.efforts)
    }

    /// Whether `model` takes a reasoning effort at all.
    pub fn supports_effort(&self, model: &str) -> bool {
        !self.efforts_for(model).is_empty()
    }

    /// The effort a launch of `model` takes when nothing named one.
    ///
    /// The declared default ([`Provider::default_effort`]) where the model has
    /// that rung; otherwise the highest rung below it, so a model with a shorter
    /// ladder is stepped down rather than pushed to an end of the scale it did
    /// not ask for. A model that takes no effort at all still needs a value to
    /// put in a [`Selection`], and the declared default is that placeholder.
    pub fn default_effort_for(&self, model: &str) -> Effort {
        let declared = self.default_effort();
        let efforts = self.efforts_for(model);
        if efforts.is_empty() || efforts.contains(&declared) {
            return declared;
        }
        efforts
            .iter()
            .copied()
            .rfind(|effort| *effort < declared)
            .or_else(|| efforts.last().copied())
            .unwrap_or(declared)
    }

    /// The model a [`Selection`] takes when a profile name omits one.
    ///
    /// Declared rather than read off the front of [`Provider::models`], so that
    /// reordering a catalog cannot silently move every unpinned launch to a
    /// different model — and so the choice can differ from the catalog's lead,
    /// as Claude Code's does.
    pub fn default_model(&self) -> &'static str {
        self.spec().default_model
    }

    /// The reasoning effort a [`Selection`] takes when a profile name omits one.
    pub fn default_effort(&self) -> Effort {
        self.spec().default_effort
    }

    /// The least expensive model this agent can launch correctly, for the
    /// incidental one-shot errands a host does around a session rather than
    /// for the session's own work — naming a branch from its first prompt, say.
    ///
    /// Such an errand is a sentence of text in and a few words out, so the
    /// small tier does it as well as the large one and at a fraction of the
    /// price. It is deliberately a separate question from
    /// [`Provider::default_model`]: an operator's unpinned *launch* should
    /// still get a capable model. And it always takes an effort setting, since
    /// every launch pins one — which can rule out the very cheapest model.
    pub fn cheapest_model(&self) -> &'static str {
        self.spec().cheapest_model
    }

    /// The lowest reasoning effort the errand model accepts, which is what
    /// those same errands ask for.
    pub fn cheapest_effort(&self) -> Effort {
        self.cheapest_effort_for(self.cheapest_model())
    }

    /// The lowest effort `model` accepts. [`Provider::efforts_for`] is ordered
    /// lowest first, so a ladder that gains a rung below the current floor
    /// moves this with it. A model with no ladder falls back to its default
    /// placeholder.
    pub fn cheapest_effort_for(&self, model: &str) -> Effort {
        self.efforts_for(model)
            .first()
            .copied()
            .unwrap_or_else(|| self.default_effort_for(model))
    }
}

/// How much reasoning the model is asked to spend per turn.
///
/// One vocabulary across providers, ordered lowest first, since the ladders
/// coincide in the middle; [`Provider::efforts_for`] narrows it to what a given
/// model accepts. Passed to codex as its `model_reasoning_effort` config
/// override and to Claude Code as `--effort`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
/// model and effort ran. The host chooses execution mode separately when
/// resolving the selection. Parsing accepts
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
    ///
    /// A model id may itself contain `/` (`deepseek/deepseek-v4.1-flash`), so
    /// the effort is what follows the *last* one. Without the catalog that
    /// leaves `codex:vendor/model` ambiguous with a misspelt effort, as in
    /// `claude:opus/turbo`; a whole name that is a catalog model is therefore
    /// read as the model with the effort omitted, and anything else must end
    /// in a known effort. An unlisted model with a `/` in it can still be
    /// launched by naming its effort, which [`Selection::name`] always does.
    pub fn parse(name: &str) -> Result<Selection> {
        let names_catalog_model = name.split_once(':').is_some_and(|(provider, model)| {
            Provider::parse(provider.trim())
                .is_ok_and(|provider| provider.model(model.trim()).is_some())
        });
        let (head, effort) = match name.rsplit_once('/') {
            Some(_) if names_catalog_model => (name, None),
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

    /// Build a conversation profile, locating the agent on the host's `PATH`.
    pub fn resolve(&self, layout: &SandboxLayout) -> Result<Profile> {
        self.resolve_for(ExecutionMode::Conversation, layout)
    }

    /// Build a profile for the requested execution mode. A batch host sends
    /// one prompt, closes stdin, and reads the answer until the process exits.
    pub fn resolve_for(&self, mode: ExecutionMode, layout: &SandboxLayout) -> Result<Profile> {
        let search = std::env::var_os("PATH").unwrap_or_default();
        self.resolve_for_on_path(mode, layout, &search)
    }

    /// [`Selection::resolve`] against an explicit executable search path.
    pub fn resolve_on_path(&self, layout: &SandboxLayout, search: &OsStr) -> Result<Profile> {
        self.resolve_for_on_path(ExecutionMode::Conversation, layout, search)
    }

    /// [`Selection::resolve_for`] against an explicit executable search path.
    pub fn resolve_for_on_path(
        &self,
        mode: ExecutionMode,
        layout: &SandboxLayout,
        search: &OsStr,
    ) -> Result<Profile> {
        let executable = resolve_executable_on_path(Path::new(self.provider.executable()), search)?;
        Ok(self
            .provider
            .spec()
            .ops
            .profile(mode, layout, &executable, &self.model, self.effort))
    }
}

/// The name a launched profile carries: exactly [`Selection::name`], so a
/// recorded profile always states the model and effort it ran and re-parses into
/// the selection that produced it.
pub(crate) fn profile_name(provider: Provider, model: &str, effort: Effort) -> String {
    Selection {
        provider,
        model: model.to_owned(),
        effort,
    }
    .name()
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
    /// The host closes stdin after one prompt and runs the agent to completion.
    /// Further messages are not supported; resuming makes that one prompt the
    /// next turn of an existing conversation.
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
    /// What that takes is the provider's own business: a launch flag, a
    /// protocol handshake the host performs afterwards, or nothing it can do at
    /// all. An empty id is rejected here, before any provider sees it.
    pub fn resume(&mut self, provider: Provider, provider_session_id: &str) -> Result<()> {
        if provider_session_id.trim().is_empty() {
            bail!("cannot resume an empty provider session id");
        }
        provider.spec().ops.resume(self, provider_session_id)
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

/// Shared by the provider modules' own tests, so each can resolve a real
/// profile without depending on what the machine running the tests happens to
/// have installed.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A search path holding stub `codex` and `claude` binaries.
    pub(crate) fn agent_bin() -> PathBuf {
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
    pub(crate) fn stub(name: &str) -> String {
        agent_bin().join(name).to_string_lossy().into_owned()
    }

    pub(crate) fn builtin(name: &str, layout: &SandboxLayout) -> Result<Profile> {
        Profile::builtin_on_path(name, layout, agent_bin().as_os_str())
    }
}

#[cfg(test)]
mod tests;
