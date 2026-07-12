use crate::MountAccess;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub isolation: IsolationConfig,
    #[serde(default, rename = "mount")]
    pub mounts: Vec<MountConfig>,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub environment: BTreeMap<OsString, OsString>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct IsolationConfig {
    #[serde(default = "podman_backend")]
    pub backend: String,
    #[serde(default)]
    pub docker: DockerConfig,
    #[serde(default)]
    pub podman: PodmanConfig,
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            backend: podman_backend(),
            docker: DockerConfig::default(),
            podman: PodmanConfig::default(),
        }
    }
}

fn podman_backend() -> String {
    "podman".into()
}

#[derive(Clone, Debug, Deserialize)]
pub struct DockerConfig {
    #[serde(default = "default_image")]
    pub image: String,
    #[serde(default = "default_workdir")]
    pub workdir: PathBuf,
    #[serde(default = "default_docker")]
    pub executable: PathBuf,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image: default_image(),
            workdir: default_workdir(),
            executable: default_docker(),
        }
    }
}
fn default_image() -> String {
    "rust:latest".into()
}
fn default_workdir() -> PathBuf {
    PathBuf::from("/workspace")
}
fn default_docker() -> PathBuf {
    PathBuf::from("docker")
}

#[derive(Clone, Debug, Deserialize)]
pub struct PodmanConfig {
    #[serde(default = "default_image")]
    pub image: String,
    #[serde(default = "default_workdir")]
    pub workdir: PathBuf,
    #[serde(default = "default_podman")]
    pub executable: PathBuf,
}

impl Default for PodmanConfig {
    fn default() -> Self {
        Self {
            image: default_image(),
            workdir: default_workdir(),
            executable: default_podman(),
        }
    }
}

fn default_podman() -> PathBuf {
    PathBuf::from("podman")
}

#[derive(Clone, Debug, Deserialize)]
pub struct MountConfig {
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub access: MountAccess,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub enabled: bool,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn discover() -> Result<Self> {
        let path = Path::new("driva.toml");
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }
}
