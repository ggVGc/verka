//! `orka.toml`: Orka-owned coding-agent and isolation policy.
//!
//! Coding-agent profiles live in Orka because they are part of Orka's prompt,
//! workspace, and outcome protocol. Driva receives a fully resolved execution
//! request and contributes no templates or agent-specific behavior.

use crate::agent::{self, OutputFormat, SandboxLayout};
use crate::driva_exec::DrivaExecutor;
use crate::engine::ExecutionPolicy;
use crate::executor::MountSpec;
use anyhow::{bail, Context, Result};
use genta::agent::{Effort, Provider};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "orka.toml";
/// The `orka.toml` written by `orka init`.
///
/// The model and effort are written out rather than left implicit: an attempt's
/// evidence should say which model produced it, and a project that names its own
/// model does not silently move to a different one when Genta's declared default
/// changes. The generated values *are* those defaults, so a fresh project starts
/// where Genta points.
pub fn default_config() -> String {
    let provider = Provider::CodexExec;
    format!(
        "# Genta describes the Codex process; Orka launches it through Driva.\n\
         [agent]\n\
         kind = \"codex\"\n\
         model = \"{}\"\n\
         effort = \"{}\"\n\
         \n\
         [isolation]\n\
         backend = \"bwrap\"\n\
         rootfs = \"/\"\n",
        provider.default_model(),
        provider.default_effort().as_str(),
    )
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub agent: AgentConfig,
    #[serde(default)]
    pub isolation: IsolationConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default, rename = "mount")]
    pub mounts: Vec<MountConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Codex,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    /// An Orka-owned coding-agent profile.
    pub kind: Option<AgentKind>,
    /// Override the executable selected by the profile.
    #[serde(default)]
    pub executable: Option<PathBuf>,
    /// The model attempts run on. Omitted, the profile's provider supplies its
    /// declared default — an attempt is always pinned to a named model, never to
    /// whatever the agent happens to be configured for, so its recorded argv says
    /// what produced the work.
    #[serde(default)]
    pub model: Option<String>,
    /// The reasoning effort attempts run at (`minimal`, `low`, `medium`, `high`,
    /// `xhigh`), defaulting the same way as `model`.
    #[serde(default)]
    pub effort: Option<String>,
    /// A fully literal command, for agents without an Orka profile.
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsolationConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    /// Isolation engine executable override (defaults to the backend name).
    #[serde(default)]
    pub executable: Option<PathBuf>,
    /// Prepared filesystem tree for Bubblewrap. Required for that backend.
    #[serde(default)]
    pub rootfs: Option<PathBuf>,
    /// Rootfs directories replaced by private writable tmpfs mounts.
    #[serde(default)]
    pub tmpfs: Vec<PathBuf>,
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            executable: None,
            rootfs: None,
            tmpfs: Vec::new(),
        }
    }
}

fn default_backend() -> String {
    "bwrap".into()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountConfig {
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub writable: bool,
}

struct ResolvedAgent {
    command: Vec<String>,
    protocol: OutputFormat,
    layout: SandboxLayout,
    mounts: Vec<MountSpec>,
    environment: BTreeMap<String, String>,
    network: bool,
    backend: ResolvedBackend,
}

enum Invocation {
    Agent(genta::agent::Profile),
    Plain(Vec<String>),
}

enum ResolvedBackend {
    Bwrap {
        executable: PathBuf,
        rootfs: PathBuf,
        tmpfs: Vec<PathBuf>,
    },
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        config.resolve()?;
        Ok(config)
    }

    /// Create the default configuration without replacing an existing file.
    pub fn init(path: &Path) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("creating {} (refusing to overwrite it)", path.display()))?;
        file.write_all(default_config().as_bytes())
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn policy(&self) -> Result<ExecutionPolicy> {
        let resolved = self.resolve()?;
        Ok(ExecutionPolicy {
            command: resolved.command,
            protocol: resolved.protocol,
            workspace_destination: resolved.layout.workspace,
            io_destination: resolved.layout.exchange,
            extra_mounts: resolved.mounts,
            environment: resolved.environment,
            network: resolved.network,
        })
    }

    pub fn executor(&self) -> Result<DrivaExecutor> {
        Ok(match self.resolve()?.backend {
            ResolvedBackend::Bwrap {
                executable,
                rootfs,
                tmpfs,
            } => DrivaExecutor::bwrap(executable, rootfs, tmpfs),
        })
    }

    fn resolve(&self) -> Result<ResolvedAgent> {
        let layout = SandboxLayout::default();
        let invocation = self.resolve_invocation(&layout)?;
        let backend = self.resolve_backend()?;

        let (command, protocol, mut mounts, mut environment, network) = match invocation {
            Invocation::Agent(profile) => (
                profile.command,
                OutputFormat::Agent(profile.protocol),
                profile.mounts.into_iter().map(Into::into).collect(),
                profile.environment,
                profile.network,
            ),
            Invocation::Plain(command) => (
                command,
                OutputFormat::Plain,
                Vec::new(),
                BTreeMap::new(),
                false,
            ),
        };
        mounts.extend(self.mounts.iter().map(|mount| MountSpec {
            source: mount.source.clone(),
            destination: mount.destination.clone(),
            writable: mount.writable,
        }));

        environment.extend(self.environment.clone());

        Ok(ResolvedAgent {
            command,
            protocol,
            layout,
            mounts,
            environment,
            network: network || self.network.enabled,
            backend,
        })
    }

    fn resolve_invocation(&self, layout: &SandboxLayout) -> Result<Invocation> {
        match (self.agent.kind, self.agent.command.is_empty()) {
            (Some(_), false) => bail!("agent.kind and agent.command are mutually exclusive"),
            (None, true) => bail!("either agent.kind or agent.command is required"),
            (Some(AgentKind::Codex), true) => {
                let executable = self
                    .agent
                    .executable
                    .as_deref()
                    .unwrap_or_else(|| Path::new("codex"));
                // Genta's `codex-exec` provider declares both defaults; the
                // configuration only overrides them.
                let provider = Provider::CodexExec;
                let model = self
                    .agent
                    .model
                    .as_deref()
                    .unwrap_or_else(|| provider.default_model());
                let effort = match &self.agent.effort {
                    Some(effort) => Effort::parse(effort)?,
                    None => provider.default_effort(),
                };
                if !provider.efforts().contains(&effort) {
                    bail!(
                        "agent.effort {:?} is not accepted by codex; known levels: {}",
                        effort.as_str(),
                        provider
                            .efforts()
                            .iter()
                            .map(|effort| effort.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                Ok(Invocation::Agent(agent::codex(
                    executable, layout, model, effort,
                )?))
            }
            (None, false) => {
                if self.agent.executable.is_some() {
                    bail!("agent.executable requires agent.kind");
                }
                if self.agent.model.is_some() || self.agent.effort.is_some() {
                    bail!("agent.model and agent.effort require agent.kind");
                }
                Ok(Invocation::Plain(self.agent.command.clone()))
            }
        }
    }

    fn resolve_backend(&self) -> Result<ResolvedBackend> {
        let executable = |name: &str| {
            self.isolation
                .executable
                .clone()
                .unwrap_or_else(|| PathBuf::from(name))
        };
        Ok(match self.isolation.backend.as_str() {
            "bwrap" => ResolvedBackend::Bwrap {
                executable: executable("bwrap"),
                rootfs: self
                    .isolation
                    .rootfs
                    .clone()
                    .context("isolation.rootfs is required for the bwrap backend")?,
                tmpfs: self.isolation.tmpfs.clone(),
            },
            other => bail!("unknown isolation backend `{other}` (only bwrap is supported)"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_command_maps_to_the_orka_layout() {
        let config: Config = toml::from_str(
            r#"
            [agent]
            command = ["agent", "--go"]

            [isolation]
            backend = "bwrap"
            rootfs = "/"

            [[mount]]
            source = "/somewhere/context"
            destination = "/context"
            "#,
        )
        .unwrap();
        let policy = config.policy().unwrap();
        assert_eq!(policy.command, vec!["agent", "--go"]);
        assert_eq!(policy.protocol, OutputFormat::Plain);
        assert_eq!(
            policy.workspace_destination,
            PathBuf::from("/tmp/orka/workspace")
        );
        assert_eq!(policy.io_destination, PathBuf::from("/tmp/orka/exchange"));
        assert!(!policy.network, "network denied unless enabled");
        assert_eq!(policy.extra_mounts.len(), 1);
        assert!(!policy.extra_mounts[0].writable, "read-only by default");
    }

    #[test]
    fn codex_profile_is_resolved_from_genta() {
        let config: Config = toml::from_str(
            r#"
            [agent]
            kind = "codex"

            [isolation]
            backend = "bwrap"
            rootfs = "/"
            "#,
        )
        .unwrap();
        let policy = config.policy().unwrap();
        assert_eq!(policy.command.last().unwrap(), agent::AGENT_PROMPT);
        assert_eq!(
            policy.protocol,
            OutputFormat::Agent(genta::event::Protocol::CodexJsonl)
        );
        assert!(policy.command.iter().any(|argument| argument == "--json"));
        assert_eq!(
            policy.workspace_destination,
            PathBuf::from("/tmp/orka/workspace")
        );
        assert_eq!(policy.io_destination, PathBuf::from("/tmp/orka/exchange"));
        assert!(policy.network);
        assert!(policy.extra_mounts.iter().any(|mount| mount.destination
            == Path::new("/tmp/agent-home/.codex")
            && mount.writable));
        assert_eq!(policy.environment["HOME"], "/tmp/agent-home");
        assert!(matches!(
            config.resolve().unwrap().backend,
            ResolvedBackend::Bwrap { .. }
        ));
    }

    /// An attempt is durable evidence, so the model and effort it ran on are
    /// always pinned in its argv — from the configuration when it names them, and
    /// from the provider's declared defaults when it does not.
    #[test]
    fn the_codex_profile_always_pins_a_model_and_an_effort() {
        let base = "[isolation]\nbackend = \"bwrap\"\nrootfs = \"/\"\n";

        let defaulted: Config =
            toml::from_str(&format!("[agent]\nkind = \"codex\"\n{base}")).unwrap();
        let command = defaulted.policy().unwrap().command;
        assert!(command.iter().any(
            |argument| argument == &format!("model={:?}", Provider::CodexExec.default_model())
        ));
        assert!(command
            .iter()
            .any(|argument| argument == r#"model_reasoning_effort="high""#));

        let pinned: Config = toml::from_str(&format!(
            "[agent]\nkind = \"codex\"\nmodel = \"gpt-5.6-luna\"\neffort = \"minimal\"\n{base}"
        ))
        .unwrap();
        let command = pinned.policy().unwrap().command;
        assert!(command
            .iter()
            .any(|argument| argument == r#"model="gpt-5.6-luna""#));
        assert!(command
            .iter()
            .any(|argument| argument == r#"model_reasoning_effort="minimal""#));
    }

    #[test]
    fn an_unusable_effort_or_a_profileless_model_is_rejected() {
        let base = "[isolation]\nbackend = \"bwrap\"\nrootfs = \"/\"\n";

        // Not a level at all.
        let unknown: Config = toml::from_str(&format!(
            "[agent]\nkind = \"codex\"\neffort = \"turbo\"\n{base}"
        ))
        .unwrap();
        assert!(unknown
            .policy()
            .unwrap_err()
            .to_string()
            .contains("unknown reasoning effort"));

        // A level Genta names but codex does not accept.
        let unaccepted: Config = toml::from_str(&format!(
            "[agent]\nkind = \"codex\"\neffort = \"max\"\n{base}"
        ))
        .unwrap();
        assert!(unaccepted
            .policy()
            .unwrap_err()
            .to_string()
            .contains("not accepted by codex"));

        // A literal command has no profile to pin a model on.
        let literal: Config = toml::from_str(&format!(
            "[agent]\ncommand = [\"agent\"]\nmodel = \"gpt-5.6-sol\"\n{base}"
        ))
        .unwrap();
        assert!(literal
            .policy()
            .unwrap_err()
            .to_string()
            .contains("require agent.kind"));
    }

    #[test]
    fn rejects_driva_templates_and_ambiguous_agent_configuration() {
        let unsupported = toml::from_str::<Config>("[agent]\ntemplate = \"codex-exec\"\n");
        assert!(unsupported
            .unwrap_err()
            .to_string()
            .contains("unknown field `template`"));

        let both: Config = toml::from_str(
            "[agent]\nkind = \"codex\"\ncommand = [\"agent\"]\n[isolation]\nbackend = \"bwrap\"\n",
        )
        .unwrap();
        assert!(both
            .policy()
            .unwrap_err()
            .to_string()
            .contains("mutually exclusive"));
    }

    #[test]
    fn bwrap_requires_an_explicit_rootfs() {
        let config: Config = toml::from_str("[agent]\nkind = \"codex\"\n").unwrap();
        assert!(config
            .policy()
            .unwrap_err()
            .to_string()
            .contains("isolation.rootfs is required"));
    }

    #[test]
    fn init_never_overwrites_an_existing_configuration() {
        let dir = std::env::temp_dir().join(format!("orka-config-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);
        Config::init(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), default_config());
        assert!(Config::init(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), default_config());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
