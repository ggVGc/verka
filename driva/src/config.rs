use crate::MountAccess;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub isolation: IsolationConfig,
    #[serde(default, rename = "mount")]
    pub mounts: Vec<MountConfig>,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default, deserialize_with = "deserialize_environment")]
    pub environment: BTreeMap<OsString, OsString>,
    /// Project-defined execution templates, keyed by the name used by
    /// `driva run --template NAME`.
    #[serde(default, rename = "template")]
    pub templates: BTreeMap<String, TemplateConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsolationConfig {
    #[serde(default = "bwrap_backend")]
    pub backend: String,
    #[serde(default)]
    pub docker: DockerConfig,
    #[serde(default)]
    pub podman: PodmanConfig,
    #[serde(default)]
    pub bwrap: BwrapConfig,
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            backend: bwrap_backend(),
            docker: DockerConfig::default(),
            podman: PodmanConfig::default(),
            bwrap: BwrapConfig::default(),
        }
    }
}

fn bwrap_backend() -> String {
    "bwrap".into()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    "docker.io/library/busybox:latest".into()
}
fn default_workdir() -> PathBuf {
    PathBuf::from("/tmp")
}
fn default_docker() -> PathBuf {
    PathBuf::from("docker")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct BwrapConfig {
    /// A prepared filesystem tree to expose as the sandbox root. When absent,
    /// Bubblewrap uses a private root with read-only host system runtime paths.
    pub rootfs: Option<PathBuf>,
    #[serde(default = "default_workdir")]
    pub workdir: PathBuf,
    #[serde(default = "default_bwrap")]
    pub executable: PathBuf,
}

impl Default for BwrapConfig {
    fn default() -> Self {
        Self {
            rootfs: None,
            workdir: default_workdir(),
            executable: default_bwrap(),
        }
    }
}

fn default_bwrap() -> PathBuf {
    PathBuf::from("bwrap")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountConfig {
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub access: MountAccess,
}

/// A reusable overlay for an execution request.
///
/// Templates deliberately use the same policy vocabulary as the command
/// line. They may grant capabilities, so selecting one is explicit.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub command: Vec<String>,
    pub backend: Option<String>,
    pub image: Option<String>,
    /// Prepared filesystem tree used when this template selects Bubblewrap.
    pub rootfs: Option<PathBuf>,
    /// Rootfs directories replaced with private writable tmpfs mounts.
    #[serde(default)]
    pub tmpfs: Vec<PathBuf>,
    /// Sandbox directory below which the canonical host project path is
    /// mounted writable and used as the working directory.
    pub workspace_root: Option<PathBuf>,
    /// Pass the derived workspace path to Codex as a trusted project.
    #[serde(default)]
    pub codex_trust_workspace: bool,
    pub workdir: Option<PathBuf>,
    #[serde(default, rename = "mount")]
    pub mounts: Vec<MountConfig>,
    /// Host directories mounted read-only and prepended to PATH.
    #[serde(default, rename = "path")]
    pub paths: Vec<PathBuf>,
    pub network: Option<bool>,
    pub interactive: Option<bool>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    #[serde(default)]
    pub enabled: bool,
}

fn deserialize_environment<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<OsString, OsString>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = BTreeMap::<String, String>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect())
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

    /// Return a project template, falling back to Driva's built-ins.
    /// A project definition with the same name replaces the built-in.
    pub fn template(&self, name: &str) -> Option<TemplateConfig> {
        self.templates
            .get(name)
            .cloned()
            .or_else(|| builtin_templates().remove(name))
    }

    /// Return all effective templates for discovery and help output.
    pub fn effective_templates(&self) -> BTreeMap<String, TemplateConfig> {
        let mut templates = builtin_templates();
        templates.extend(self.templates.clone());
        templates
    }
}

fn builtin_templates() -> BTreeMap<String, TemplateConfig> {
    [
        ("claude", include_str!("../templates/claude.toml")),
        ("claude-exec", include_str!("../templates/claude-exec.toml")),
        ("codex", include_str!("../templates/codex.toml")),
        ("codex-exec", include_str!("../templates/codex-exec.toml")),
        (
            "codex-runtime",
            include_str!("../templates/codex-runtime.toml"),
        ),
    ]
    .into_iter()
    .map(|(name, source)| {
        let template = toml::from_str(source)
            .unwrap_or_else(|error| panic!("invalid built-in template {name:?}: {error}"));
        (name.into(), template)
    })
    .collect()
}
