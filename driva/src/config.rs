use crate::{Mount, MountAccess};
use anyhow::{bail, Context, Result};
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
    pub bwrap: BwrapConfig,
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            backend: bwrap_backend(),
            bwrap: BwrapConfig::default(),
        }
    }
}

fn bwrap_backend() -> String {
    "bwrap".into()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BwrapConfig {
    /// A prepared filesystem tree to expose as the sandbox root. When absent,
    /// Bubblewrap uses a private root with read-only host system runtime paths.
    pub rootfs: Option<PathBuf>,
    pub workdir: Option<PathBuf>,
    #[serde(default = "default_bwrap")]
    pub executable: PathBuf,
}

impl Default for BwrapConfig {
    fn default() -> Self {
        Self {
            rootfs: None,
            workdir: None,
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
    #[serde(default)]
    pub kind: MountKind,
    pub source: Option<PathBuf>,
    pub destination: Option<PathBuf>,
    pub access: Option<MountAccess>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MountKind {
    #[default]
    Bind,
    Temporary,
}

impl MountConfig {
    /// Resolve the host source and default an omitted destination to the same
    /// canonical path inside the isolation.
    pub fn resolve(self) -> Result<Mount> {
        match self.kind {
            MountKind::Bind => {
                let source = self.source.context("bind mount requires a source")?;
                let source = crate::canonicalize_mount(&source)
                    .with_context(|| format!("invalid mount source {}", source.display()))?;
                let destination = self.destination.unwrap_or_else(|| source.clone());
                Ok(Mount::Bind {
                    source,
                    destination,
                    access: self.access.unwrap_or_default(),
                })
            }
            MountKind::Temporary => {
                if self.source.is_some() {
                    bail!("temporary mount does not accept a source");
                }
                if self.access.is_some() {
                    bail!("temporary mount does not accept an access mode");
                }
                let destination = self
                    .destination
                    .context("temporary mount requires a destination")?;
                Ok(Mount::Temporary { destination })
            }
        }
    }
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
    /// Prepared filesystem tree used when this template selects Bubblewrap.
    pub rootfs: Option<PathBuf>,
    /// A mount whose resolved destination is also used as the working
    /// directory. Resolution rejects more than one workspace mount.
    #[serde(default, rename = "workspace-mount")]
    pub workspace_mounts: Vec<MountConfig>,
    pub workdir: Option<PathBuf>,
    #[serde(default, rename = "mount")]
    pub mounts: Vec<MountConfig>,
    /// Host directories mounted read-only and prepended to PATH.
    #[serde(default, rename = "path")]
    pub paths: Vec<PathBuf>,
    pub network: Option<bool>,
    pub interactive: Option<bool>,
    /// Start a new terminal session (Bubblewrap's `--new-session`, which
    /// blocks TIOCSTI input injection). Omit for tools that require the
    /// caller's inherited session.
    #[serde(rename = "new-session")]
    pub new_session: Option<bool>,
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

impl TemplateConfig {
    /// Add another template on top of this one.
    ///
    /// Collection-valued grants accumulate, while scalar settings and the
    /// command are replaced when the later template specifies them.
    pub fn overlay(&mut self, later: Self) {
        if !later.description.is_empty() {
            self.description = later.description;
        }
        if !later.command.is_empty() {
            self.command = later.command;
        }
        if later.backend.is_some() {
            self.backend = later.backend;
        }
        if later.rootfs.is_some() {
            self.rootfs = later.rootfs;
        }
        // A workspace mount also selects the workdir. When a later template
        // selects either, retain the earlier grant as an ordinary mount while
        // letting the later template choose the effective workdir.
        if later.workdir.is_some() || !later.workspace_mounts.is_empty() {
            self.mounts.append(&mut self.workspace_mounts);
        }
        if !later.workspace_mounts.is_empty() {
            self.workspace_mounts = later.workspace_mounts;
        }
        if later.workdir.is_some() {
            self.workdir = later.workdir;
        }
        self.mounts.extend(later.mounts);
        self.paths.extend(later.paths);
        if later.network.is_some() {
            self.network = later.network;
        }
        if later.interactive.is_some() {
            self.interactive = later.interactive;
        }
        if later.new_session.is_some() {
            self.new_session = later.new_session;
        }
        self.environment.extend(later.environment);
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
        ("sbt", include_str!("../templates/sbt.toml")),
    ]
    .into_iter()
    .map(|(name, source)| {
        let template = toml::from_str(source)
            .unwrap_or_else(|error| panic!("invalid built-in template {name:?}: {error}"));
        (name.into(), template)
    })
    .collect()
}
