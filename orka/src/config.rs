//! `orka.toml`: per-workbench orchestration policy.
//!
//! Orka's configuration decides *policy* — which agent command runs, which
//! backend and image isolate it, what extra context is exposed. Isolation
//! *mechanics* stay in Driva. An example:
//!
//! ```toml
//! [agent]
//! command = ["claude", "-p", "Follow the instructions in $ORKA_PROMPT"]
//!
//! [isolation]
//! backend = "podman"                  # or "docker"
//! image = "docker.io/library/rust:1.88"
//! workdir = "/workspace"              # workspace mountpoint
//! io = "/orka"                        # exchange-directory mountpoint
//!
//! [network]
//! enabled = false
//!
//! [environment]
//! RUST_BACKTRACE = "1"
//!
//! [[mount]]                           # extra read-only context
//! source = "~/.cargo/registry"
//! destination = "/cargo/registry"
//! ```

use crate::driva_exec::DrivaExecutor;
use crate::engine::ExecutionPolicy;
use crate::executor::MountSpec;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "orka.toml";

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
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentConfig {
    /// The agent command as program and arguments, run inside the isolated
    /// environment with the workspace as its working directory.
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

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if config.agent.command.is_empty() {
            bail!("{}: agent.command must not be empty", path.display());
        }
        Ok(config)
    }

    pub fn policy(&self) -> ExecutionPolicy {
        ExecutionPolicy {
            command: self.agent.command.clone(),
            workspace_destination: self.isolation.workdir.clone(),
            io_destination: self.isolation.io.clone(),
            extra_mounts: self
                .mounts
                .iter()
                .map(|m| MountSpec {
                    source: m.source.clone(),
                    destination: m.destination.clone(),
                    writable: m.writable,
                })
                .collect(),
            environment: self.environment.clone(),
            network: self.network.enabled,
        }
    }

    pub fn executor(&self) -> Result<DrivaExecutor> {
        let executable = |name: &str| {
            self.isolation
                .executable
                .clone()
                .unwrap_or_else(|| PathBuf::from(name))
        };
        match self.isolation.backend.as_str() {
            "podman" => Ok(DrivaExecutor::podman(
                executable("podman"),
                self.isolation.image.clone(),
            )),
            "docker" => Ok(DrivaExecutor::docker(
                executable("docker"),
                self.isolation.image.clone(),
            )),
            other => bail!("unknown isolation backend `{other}` (podman or docker)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_with_defaults_and_maps_to_policy() {
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
        assert_eq!(config.isolation.backend, "podman");
        let policy = config.policy();
        assert_eq!(policy.command, vec!["agent", "--go"]);
        assert_eq!(policy.workspace_destination, PathBuf::from("/workspace"));
        assert_eq!(policy.io_destination, PathBuf::from("/orka"));
        assert!(!policy.network, "network denied unless enabled");
        assert_eq!(policy.extra_mounts.len(), 1);
        assert!(!policy.extra_mounts[0].writable, "read-only by default");
    }

    #[test]
    fn an_unknown_backend_is_refused() {
        let config: Config =
            toml::from_str("[agent]\ncommand = [\"a\"]\n[isolation]\nbackend = \"vm\"\n").unwrap();
        assert!(config.executor().is_err());
    }
}
