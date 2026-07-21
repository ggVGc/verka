//! `orka.toml`: Orka-owned coding-agent and isolation policy.
//!
//! Coding-agent profiles live in Orka because they are part of Orka's prompt,
//! workspace, and outcome protocol. Driva receives a fully resolved execution
//! request and contributes no templates or agent-specific behavior.

use crate::agent::{self, AgentInvocation, AgentProtocol, SandboxLayout};
use crate::driva_exec::DrivaExecutor;
use crate::engine::ExecutionPolicy;
use crate::executor::MountSpec;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "orka.toml";
pub const DEFAULT_CONFIG: &str = r#"# Orka owns the Codex invocation and delegates only isolation to Driva.
[agent]
kind = "codex"

[isolation]
backend = "bwrap"
rootfs = "/"
tmpfs = ["/root"]
"#;

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
    /// A fully literal command, for agents without an Orka profile.
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsolationConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_image")]
    pub image: String,
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
            image: default_image(),
            executable: None,
            rootfs: None,
            tmpfs: Vec::new(),
        }
    }
}

fn default_backend() -> String {
    "bwrap".into()
}

fn default_image() -> String {
    "docker.io/library/busybox:latest".into()
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
    protocol: AgentProtocol,
    layout: SandboxLayout,
    mounts: Vec<MountSpec>,
    environment: BTreeMap<String, String>,
    network: bool,
    backend: ResolvedBackend,
}

enum ResolvedBackend {
    Podman {
        executable: PathBuf,
        image: String,
    },
    Docker {
        executable: PathBuf,
        image: String,
    },
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
        file.write_all(DEFAULT_CONFIG.as_bytes())
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
            ResolvedBackend::Podman { executable, image } => {
                DrivaExecutor::podman(executable, image)
            }
            ResolvedBackend::Docker { executable, image } => {
                DrivaExecutor::docker(executable, image)
            }
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

        let mut mounts = invocation.mounts;
        mounts.extend(self.mounts.iter().map(|mount| MountSpec {
            source: mount.source.clone(),
            destination: mount.destination.clone(),
            writable: mount.writable,
        }));

        let mut environment = invocation.environment;
        environment.extend(self.environment.clone());

        Ok(ResolvedAgent {
            command: invocation.command,
            protocol: invocation.protocol,
            layout,
            mounts,
            environment,
            network: invocation.network || self.network.enabled,
            backend,
        })
    }

    fn resolve_invocation(&self, layout: &SandboxLayout) -> Result<AgentInvocation> {
        match (self.agent.kind, self.agent.command.is_empty()) {
            (Some(_), false) => bail!("agent.kind and agent.command are mutually exclusive"),
            (None, true) => bail!("either agent.kind or agent.command is required"),
            (Some(AgentKind::Codex), true) => {
                let executable = self
                    .agent
                    .executable
                    .as_deref()
                    .unwrap_or_else(|| Path::new("codex"));
                agent::codex(executable, layout)
            }
            (None, false) => {
                if self.agent.executable.is_some() {
                    bail!("agent.executable requires agent.kind");
                }
                Ok(AgentInvocation {
                    command: self.agent.command.clone(),
                    protocol: AgentProtocol::Plain,
                    mounts: Vec::new(),
                    environment: BTreeMap::new(),
                    network: false,
                })
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
            "podman" => ResolvedBackend::Podman {
                executable: executable("podman"),
                image: self.isolation.image.clone(),
            },
            "docker" => ResolvedBackend::Docker {
                executable: executable("docker"),
                image: self.isolation.image.clone(),
            },
            "bwrap" => ResolvedBackend::Bwrap {
                executable: executable("bwrap"),
                rootfs: self
                    .isolation
                    .rootfs
                    .clone()
                    .context("isolation.rootfs is required for the bwrap backend")?,
                tmpfs: self.isolation.tmpfs.clone(),
            },
            other => bail!("unknown isolation backend `{other}` (bwrap, podman, or docker)"),
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
            backend = "podman"

            [[mount]]
            source = "/somewhere/context"
            destination = "/context"
            "#,
        )
        .unwrap();
        let policy = config.policy().unwrap();
        assert_eq!(policy.command, vec!["agent", "--go"]);
        assert_eq!(policy.protocol, AgentProtocol::Plain);
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
    fn codex_profile_is_owned_and_resolved_by_orka() {
        let config: Config = toml::from_str(
            r#"
            [agent]
            kind = "codex"

            [isolation]
            backend = "bwrap"
            rootfs = "/"
            tmpfs = ["/root"]
            "#,
        )
        .unwrap();
        let policy = config.policy().unwrap();
        assert_eq!(policy.command.last().unwrap(), agent::AGENT_PROMPT);
        assert_eq!(policy.protocol, AgentProtocol::CodexJsonl);
        assert!(policy.command.iter().any(|argument| argument == "--json"));
        assert_eq!(
            policy.workspace_destination,
            PathBuf::from("/tmp/orka/workspace")
        );
        assert_eq!(policy.io_destination, PathBuf::from("/tmp/orka/exchange"));
        assert!(policy.network);
        assert!(policy
            .extra_mounts
            .iter()
            .any(|mount| mount.destination == Path::new("/root/.codex/auth.json")));
        assert!(matches!(
            config.resolve().unwrap().backend,
            ResolvedBackend::Bwrap { .. }
        ));
    }

    #[test]
    fn rejects_driva_templates_and_ambiguous_agent_configuration() {
        let old = toml::from_str::<Config>("[agent]\ntemplate = \"codex-exec\"\n");
        assert!(old
            .unwrap_err()
            .to_string()
            .contains("unknown field `template`"));

        let both: Config = toml::from_str(
            "[agent]\nkind = \"codex\"\ncommand = [\"agent\"]\n[isolation]\nbackend = \"podman\"\n",
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
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        assert!(Config::init(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
