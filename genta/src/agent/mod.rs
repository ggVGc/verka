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

    /// What this provider declares about itself.
    ///
    /// The one place a provider is matched on. Every accessor below reads a
    /// field of the returned spec, so a new provider needs one arm here and one
    /// module of its own; a new *question* to ask a provider needs one field
    /// and one answer per module, which the compiler demands at each literal.
    fn spec(&self) -> &'static ProviderSpec {
        match self {
            Provider::Codex => &self::codex::APP_SERVER_SPEC,
            Provider::CodexExec => &self::codex::EXEC_SPEC,
            Provider::Claude => &self::claude::SPEC,
        }
    }

    pub fn as_str(&self) -> &'static str {
        self.spec().name
    }

    /// The wire protocol (and therefore provider-specific presentation rules)
    /// used by this provider.
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

    /// Build the launchable profile, locating the agent on the host's `PATH`.
    pub fn resolve(&self, layout: &SandboxLayout) -> Result<Profile> {
        let search = std::env::var_os("PATH").unwrap_or_default();
        self.resolve_on_path(layout, &search)
    }

    /// [`Selection::resolve`] against an explicit executable search path.
    pub fn resolve_on_path(&self, layout: &SandboxLayout, search: &OsStr) -> Result<Profile> {
        let executable = resolve_executable_on_path(Path::new(self.provider.executable()), search)?;
        Ok(self
            .provider
            .spec()
            .ops
            .profile(layout, &executable, &self.model, self.effort))
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
mod tests {
    use super::testing::{agent_bin, builtin};
    use super::*;

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

    #[test]
    fn unknown_profile_is_rejected() {
        assert!(builtin("gpt5", &SandboxLayout::default()).is_err());
    }

    #[test]
    fn empty_model_suffix_is_rejected() {
        assert!(builtin("claude:", &SandboxLayout::default()).is_err());
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

    /// A model id with a `/` in it: the effort is what follows the last one,
    /// and a catalog model named alone takes the default effort rather than
    /// having its own tail read as one. An unlisted id with a `/` is
    /// launchable too, so long as its effort is named.
    #[test]
    fn a_model_id_may_contain_the_effort_separator() {
        let deepseek = "deepseek/deepseek-v4.1-flash";
        let pinned = Selection::parse(&format!("codex:{deepseek}/high")).unwrap();
        assert_eq!(
            (pinned.model.as_str(), pinned.effort),
            (deepseek, Effort::High)
        );
        assert_eq!(pinned.name(), format!("codex:{deepseek}/high"));

        let bare = Selection::parse(&format!("codex:{deepseek}")).unwrap();
        assert_eq!(bare.model, deepseek);
        assert_eq!(bare.effort, Provider::Codex.default_effort());

        let unlisted = Selection::parse("codex:vendor/new-model/low").unwrap();
        assert_eq!(
            (unlisted.model.as_str(), unlisted.effort),
            ("vendor/new-model", Effort::Low)
        );
        // Without the catalog to vouch for it, an unlisted id's tail is an
        // effort, and a misspelt one is still refused by name.
        let error = Selection::parse("codex:vendor/new-model").unwrap_err();
        assert!(
            error.to_string().contains("unknown reasoning effort"),
            "{error}"
        );
    }

    /// A selection may not leave the model or effort unpinned: a shorter profile
    /// name takes the provider's declared defaults, and then names itself in
    /// full, so what a journal records is never "whatever the agent was set to".
    #[test]
    fn a_shorter_profile_name_takes_the_declared_defaults() {
        for provider in Provider::ALL {
            let name = provider.as_str();
            let (model, effort) = (provider.default_model(), provider.default_effort());

            // The bare name fills out to both declared defaults, and filling in
            // a default is idempotent — the full name re-parses to it.
            let parsed = Selection::parse(name).unwrap();
            let full = format!("{name}:{model}/{}", effort.as_str());
            assert_eq!(parsed.name(), full, "{name:?} should fill out to {full:?}");
            assert_eq!(Selection::parse(&full).unwrap(), parsed);

            // Whichever half is given is kept; only the missing half defaults.
            let cheapest = provider.cheapest_model();
            assert_eq!(
                Selection::parse(&format!("{name}:{cheapest}"))
                    .unwrap()
                    .name(),
                format!("{name}:{cheapest}/{}", effort.as_str())
            );
            let cheapest_effort = provider.cheapest_effort().as_str();
            assert_eq!(
                Selection::parse(&format!("{name}/{cheapest_effort}"))
                    .unwrap()
                    .name(),
                format!("{name}:{model}/{cheapest_effort}")
            );

            // `Selection::new` is the same defaults by another route.
            let selection = Selection::new(provider);
            assert_eq!(selection.model, model);
            assert_eq!(selection.effort, effort);
            assert_eq!(parsed, selection);
        }
    }

    /// Each declared default must be launchable as a model id and effort.
    #[test]
    fn the_declared_defaults_are_offered_by_their_provider() {
        for provider in Provider::ALL {
            let model = provider.default_model();
            assert!(
                provider.model(model).is_some(),
                "{provider:?} default model is outside its own catalog"
            );
            assert!(
                provider
                    .efforts_for(model)
                    .contains(&provider.default_effort()),
                "{provider:?} cannot run its own default effort on its default model"
            );
        }
    }

    /// The errand tier is launchable too, and is never the tier a session
    /// itself would get — an errand that cost what the session does would not
    /// be worth routing away from it.
    #[test]
    fn the_cheapest_model_is_offered_by_its_provider_and_is_not_the_default() {
        for provider in Provider::ALL {
            let model = provider.cheapest_model();
            assert!(
                provider.model(model).is_some(),
                "{provider:?} cheapest model is outside its own catalog"
            );
            // Every launch pins an effort, so a model that rejects one cannot
            // run an errand.
            assert!(
                provider.supports_effort(model),
                "{provider:?} routes errands to a model that takes no effort"
            );
            assert!(
                provider
                    .efforts_for(model)
                    .contains(&provider.cheapest_effort()),
                "{provider:?} cannot run its own cheapest effort"
            );
            assert_ne!(
                provider.cheapest_model(),
                provider.default_model(),
                "{provider:?} routes errands to the model its sessions run on"
            );
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

    /// A provider's declared protocol and the one its built profile carries are
    /// two statements of the same fact, and a journal is replayed with the
    /// declared one.
    #[test]
    fn a_built_profile_speaks_the_protocol_its_provider_declares() {
        for provider in Provider::ALL {
            let profile = builtin(provider.as_str(), &SandboxLayout::default()).unwrap();
            assert_eq!(
                profile.protocol,
                provider.protocol(),
                "{provider:?} builds a profile that speaks a protocol it does not declare"
            );
        }
    }

    /// Resuming is per-provider, but an unusable id is refused the same way for
    /// all of them, before any provider-specific work.
    #[test]
    fn an_empty_provider_session_id_is_refused_by_every_provider() {
        for provider in Provider::ALL {
            let mut profile = builtin(provider.as_str(), &SandboxLayout::default()).unwrap();
            let error = profile
                .resume(provider, "  ")
                .expect_err("an empty id cannot name a conversation");
            assert!(format!("{error:#}").contains("empty provider session id"));
        }
    }

    /// Every catalog entry is launchable as written and stays within its
    /// provider's widest ladder, so that ladder is a true fallback for an
    /// unlisted id rather than a second, disagreeing statement.
    #[test]
    fn every_catalog_entry_is_launchable_and_within_the_widest_ladder() {
        for provider in Provider::ALL {
            assert!(!provider.models().is_empty());
            assert!(provider.efforts().is_sorted(), "{provider:?} ladder order");
            for entry in provider.models() {
                // A `Selection` round-trips through one string, so every id
                // must survive it — at every rung, and with the effort left
                // off — even one that carries the grammar's own `/`.
                for id in std::iter::once(&entry.id).chain(entry.aliases) {
                    for effort in provider.efforts() {
                        let selection = Selection {
                            provider,
                            model: (*id).to_owned(),
                            effort: *effort,
                        };
                        assert_eq!(
                            Selection::parse(&selection.name()).unwrap(),
                            selection,
                            "{id} would not survive Selection::name"
                        );
                    }
                    let bare = format!("{}:{id}", provider.as_str());
                    assert_eq!(Selection::parse(&bare).unwrap().model, *id, "{bare}");
                }
                assert!(entry.efforts.is_sorted(), "{} ladder order", entry.id);
                for effort in entry.efforts {
                    assert!(
                        provider.efforts().contains(effort),
                        "{} offers {effort:?}, outside {provider:?}'s widest ladder",
                        entry.id
                    );
                }
                // The default a picker opens on is one of the model's rungs.
                let default = provider.default_effort_for(entry.id);
                assert!(
                    entry.efforts.is_empty() || entry.efforts.contains(&default),
                    "{provider:?} {} opens on a rung it does not have",
                    entry.id
                );
            }
            for effort in provider.efforts() {
                assert_eq!(Effort::parse(effort.as_str()).unwrap(), *effort);
            }
        }
    }

    /// The point of a per-model ladder: a rung one model has and another does
    /// not is offered on the first and not the second, under the same
    /// provider — and an alias is judged as the model it names.
    #[test]
    fn an_effort_ladder_belongs_to_the_model_not_the_agent() {
        let claude = Provider::Claude;
        assert!(claude.efforts_for("claude-opus-5").contains(&Effort::XHigh));
        assert!(!claude
            .efforts_for("claude-opus-4-6")
            .contains(&Effort::XHigh));
        assert_eq!(
            claude.efforts_for("claude-opus-4-5"),
            claude.efforts_for("claude-opus-4-5-20251101")
        );

        let codex = Provider::Codex;
        assert!(codex.efforts_for("gpt-5.6-sol").contains(&Effort::Max));
        assert!(!codex.efforts_for("gpt-5.5").contains(&Effort::Max));
        assert!(!codex.efforts_for("gpt-5.4").contains(&Effort::Max));
    }

    /// Genta's `minimal` rung is on no current codex model, so nothing may
    /// launch on it — including the errand path, which takes the lowest rung.
    #[test]
    fn codex_does_not_offer_a_minimal_effort() {
        for provider in [Provider::Codex, Provider::CodexExec] {
            assert!(!provider.efforts().contains(&Effort::Minimal));
            assert_eq!(provider.cheapest_effort(), Effort::Low);
        }
    }

    /// The two Claude models that predate the effort parameter are offered, but
    /// take no effort — and so are never the errand model.
    #[test]
    fn the_models_without_an_effort_parameter_say_so() {
        for model in [
            "claude-sonnet-4-5-20250929",
            "claude-haiku-4-5-20251001",
            "claude-haiku-4-5",
        ] {
            assert!(!Provider::Claude.supports_effort(model), "{model}");
            assert_eq!(
                Provider::Claude.default_effort_for(model),
                Provider::Claude.default_effort(),
                "{model} still has a placeholder to put in a selection"
            );
        }
    }

    /// A shorter ladder that still has the declared default keeps it: the
    /// default is only stepped down when the model lacks that rung.
    #[test]
    fn a_declared_default_stands_on_a_shorter_ladder_that_has_it() {
        assert_eq!(
            Provider::CodexExec.default_effort_for("gpt-5.5"),
            Provider::CodexExec.default_effort()
        );
        assert_eq!(
            Provider::Claude.default_effort_for("claude-opus-4-5-20251101"),
            Provider::Claude.default_effort()
        );
        assert_eq!(
            Provider::Claude.default_effort_for("claude-opus-5"),
            Provider::Claude.default_effort()
        );
    }

    /// An id newer than the catalog is under-constrained rather than refused:
    /// the agent itself is the authority on its own catalog, and the id is
    /// nobody's in particular.
    #[test]
    fn an_unknown_model_falls_back_to_the_widest_ladder() {
        assert_eq!(
            Provider::Claude.efforts_for("claude-opus-9"),
            Provider::Claude.efforts()
        );
        assert_eq!(
            Provider::Codex.efforts_for("gpt-7-nova"),
            Provider::Codex.efforts()
        );
        assert!(Provider::Claude.could_run("claude-opus-9"));
        assert!(!Provider::Claude.could_run("gpt-6.1-sol"));
        assert!(!Provider::Codex.could_run("claude-haiku-4-5"));
    }
}
