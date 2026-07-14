//! `orka.toml`: per-workbench orchestration policy.
//!
//! The default delegates the agent runtime and isolation details to Driva's
//! non-interactive Codex template:
//!
//! ```toml
//! [agent]
//! template = "codex-exec"
//! ```
//!
//! A literal command and the older Orka-native isolation settings remain
//! supported for small custom images.

use crate::driva_exec::DrivaExecutor;
use crate::engine::ExecutionPolicy;
use crate::executor::MountSpec;
use anyhow::{bail, Context, Result};
use driva::{MountAccess, TemplateConfig};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "orka.toml";
pub const DEFAULT_CONFIG: &str = r#"# Orka delegates the agent runtime and its isolation policy to Driva.
# Install the Codex runtime first with: driva runtime install codex@latest
[agent]
template = "codex-exec"
"#;

const TEMPLATE_PROMPT: &str =
    "Read and follow the instructions in the file named by the ORKA_PROMPT environment variable.";

#[derive(Clone, Debug, Deserialize)]
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
    #[serde(skip)]
    driva: driva::Config,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentConfig {
    /// A Driva template name, such as `codex-exec` or `claude-exec`.
    pub template: Option<String>,
    /// A literal command for the legacy Orka-native configuration form.
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IsolationConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_image")]
    pub image: String,
    /// Engine executable override (defaults to the backend's name in PATH).
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default = "default_workdir")]
    pub workdir: PathBuf,
    #[serde(default = "default_io")]
    pub io: PathBuf,
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            image: default_image(),
            executable: None,
            workdir: default_workdir(),
            io: default_io(),
        }
    }
}

fn default_backend() -> String {
    "podman".into()
}
fn default_image() -> String {
    "docker.io/library/busybox:latest".into()
}
fn default_workdir() -> PathBuf {
    PathBuf::from("/workspace")
}
fn default_io() -> PathBuf {
    PathBuf::from("/orka")
}

fn template_io() -> PathBuf {
    // Driva's prepared Bubblewrap rootfs deliberately exposes `/tmp` as a
    // writable tmpfs. Keeping the exchange mount below it avoids requiring
    // every agent rootfs to contain an Orka-specific top-level directory.
    PathBuf::from("/tmp/orka")
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MountConfig {
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub writable: bool,
}

struct ResolvedAgent {
    command: Vec<String>,
    workspace: PathBuf,
    io: PathBuf,
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
        let mut config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let driva_path = path.with_file_name("driva.toml");
        if driva_path.exists() {
            config.driva = driva::Config::load(&driva_path)?;
        }
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
            workspace_destination: resolved.workspace,
            io_destination: resolved.io,
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
        match (&self.agent.template, self.agent.command.is_empty()) {
            (Some(_), false) => bail!("agent.template and agent.command are mutually exclusive"),
            (None, true) => bail!("either agent.template or agent.command is required"),
            (Some(name), true) => self.resolve_template(name),
            (None, false) => self.resolve_literal(),
        }
    }

    fn resolve_literal(&self) -> Result<ResolvedAgent> {
        let executable = |name: &str| {
            self.isolation
                .executable
                .clone()
                .unwrap_or_else(|| PathBuf::from(name))
        };
        let backend = match self.isolation.backend.as_str() {
            "podman" => ResolvedBackend::Podman {
                executable: executable("podman"),
                image: self.isolation.image.clone(),
            },
            "docker" => ResolvedBackend::Docker {
                executable: executable("docker"),
                image: self.isolation.image.clone(),
            },
            other => bail!("unknown isolation backend `{other}` (podman or docker)"),
        };
        Ok(ResolvedAgent {
            command: self.agent.command.clone(),
            workspace: self.isolation.workdir.clone(),
            io: self.isolation.io.clone(),
            mounts: self
                .mounts
                .iter()
                .map(|mount| MountSpec {
                    source: mount.source.clone(),
                    destination: mount.destination.clone(),
                    writable: mount.writable,
                })
                .collect(),
            environment: self.environment.clone(),
            network: self.network.enabled,
            backend,
        })
    }

    fn resolve_template(&self, name: &str) -> Result<ResolvedAgent> {
        let template = self
            .driva
            .template(name)
            .with_context(|| format!("unknown Driva template `{name}`"))?;
        if template.interactive {
            bail!(
                "Driva template `{name}` is interactive; Orka requires a non-interactive template"
            );
        }
        if template.command.is_empty() {
            bail!("Driva template `{name}` has no command");
        }
        let workspace = template.workdir.clone().unwrap_or_else(default_workdir);
        let backend = resolve_template_backend(name, &template, &self.driva)?;
        let mut command = template.command;
        command.push(TEMPLATE_PROMPT.into());
        let mounts = template
            .mounts
            .into_iter()
            // Orka supplies the concrete attempt worktree at this destination.
            .filter(|mount| mount.destination != workspace)
            .map(|mount| MountSpec {
                source: mount.source,
                destination: mount.destination,
                writable: mount.access == MountAccess::ReadWrite,
            })
            .collect();
        Ok(ResolvedAgent {
            command,
            workspace,
            io: template_io(),
            mounts,
            environment: template.environment,
            network: template.network,
            backend,
        })
    }
}

fn resolve_template_backend(
    name: &str,
    template: &TemplateConfig,
    config: &driva::Config,
) -> Result<ResolvedBackend> {
    let backend = template
        .backend
        .as_deref()
        .unwrap_or(&config.isolation.backend);
    Ok(match backend {
        "podman" => ResolvedBackend::Podman {
            executable: config.isolation.podman.executable.clone(),
            image: template
                .image
                .clone()
                .unwrap_or_else(|| config.isolation.podman.image.clone()),
        },
        "docker" => ResolvedBackend::Docker {
            executable: config.isolation.docker.executable.clone(),
            image: template
                .image
                .clone()
                .unwrap_or_else(|| config.isolation.docker.image.clone()),
        },
        "bwrap" => ResolvedBackend::Bwrap {
            executable: config.isolation.bwrap.executable.clone(),
            rootfs: template
                .rootfs
                .clone()
                .or_else(|| config.isolation.bwrap.rootfs.clone())
                .with_context(|| {
                    format!("Driva template `{name}` selects bwrap without a rootfs")
                })?,
            tmpfs: template.tmpfs.clone(),
        },
        other => bail!("Driva template `{name}` selects unknown backend `{other}`"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_command_parses_with_defaults_and_maps_to_policy() {
        let config: Config = toml::from_str(
            r#"
            [agent]
            command = ["agent", "--go"]

            [[mount]]
            source = "/somewhere/context"
            destination = "/context"
            "#,
        )
        .unwrap();
        let policy = config.policy().unwrap();
        assert_eq!(policy.command, vec!["agent", "--go"]);
        assert_eq!(policy.workspace_destination, PathBuf::from("/workspace"));
        assert!(!policy.network, "network denied unless enabled");
        assert_eq!(policy.extra_mounts.len(), 1);
        assert!(!policy.extra_mounts[0].writable, "read-only by default");
    }

    #[test]
    fn codex_template_maps_runtime_and_replaces_its_workspace_mount() {
        let config: Config = toml::from_str("[agent]\ntemplate = \"codex-exec\"\n").unwrap();
        let policy = config.policy().unwrap();
        assert_eq!(policy.command.last().unwrap(), TEMPLATE_PROMPT);
        assert_eq!(policy.workspace_destination, PathBuf::from("/workspace"));
        assert_eq!(policy.io_destination, PathBuf::from("/tmp/orka"));
        assert!(policy.network);
        assert!(policy
            .extra_mounts
            .iter()
            .all(|mount| mount.destination != Path::new("/workspace")));
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
    fn rejects_ambiguous_or_interactive_agent_configuration() {
        let both: Config =
            toml::from_str("[agent]\ntemplate = \"codex-exec\"\ncommand = [\"agent\"]\n").unwrap();
        assert!(both
            .policy()
            .unwrap_err()
            .to_string()
            .contains("mutually exclusive"));

        let interactive: Config = toml::from_str("[agent]\ntemplate = \"codex\"\n").unwrap();
        assert!(interactive
            .policy()
            .unwrap_err()
            .to_string()
            .contains("interactive"));
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
