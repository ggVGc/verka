use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const CODEX_PACKAGE: &str = "@openai/codex";
const DEFAULT_BUILD_IMAGE: &str = "docker.io/library/node:22-bookworm";

/// A pinned runtime artifact requested as `codex@VERSION`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSpec {
    pub name: String,
    pub version: String,
}

impl RuntimeSpec {
    pub fn parse(value: &str) -> Result<Self> {
        let (name, version) = value
            .split_once('@')
            .with_context(|| format!("runtime must be NAME@VERSION, got {value:?}"))?;
        if name != "codex" {
            bail!("unsupported runtime {name:?}; only codex@VERSION is currently supported");
        }
        if version != "latest" && !is_pinned_version(version) {
            bail!("runtime version must be an exact version or latest, got {version:?}");
        }
        Ok(Self {
            name: name.into(),
            version: version.into(),
        })
    }

    pub fn display(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }

    pub fn is_floating(&self) -> bool {
        self.version == "latest"
    }

    fn resolved(name: &str, version: &str) -> Result<Self> {
        if !is_pinned_version(version) {
            bail!("npm resolved an invalid runtime version {version:?}");
        }
        Ok(Self {
            name: name.into(),
            version: version.into(),
        })
    }
}

fn is_pinned_version(version: &str) -> bool {
    !version.is_empty()
        && version.starts_with(|character: char| character.is_ascii_digit())
        && version.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+')
        })
}

/// Versioned prepared runtime artifacts used as read-only Bubblewrap roots.
#[derive(Clone, Debug)]
pub struct RuntimeStore {
    root: PathBuf,
}

impl RuntimeStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_path() -> Result<PathBuf> {
        let home =
            std::env::var_os("HOME").context("HOME is not set; cannot locate runtime store")?;
        Ok(PathBuf::from(home).join(".local/share/driva/runtimes"))
    }

    pub fn default_build_image() -> &'static str {
        DEFAULT_BUILD_IMAGE
    }

    pub fn rootfs(&self, spec: &RuntimeSpec) -> PathBuf {
        self.root
            .join(&spec.name)
            .join(&spec.version)
            .join("rootfs")
    }

    pub fn current_rootfs(&self, name: &str) -> PathBuf {
        self.root.join(name).join("current/rootfs")
    }

    pub fn install_codex(
        &self,
        spec: &RuntimeSpec,
        image: &str,
        podman: &Path,
    ) -> Result<RuntimeSpec> {
        if spec.name != "codex" {
            bail!("unsupported runtime {:?}", spec.name);
        }
        let family = self.root.join(&spec.name);
        fs::create_dir_all(&family)
            .with_context(|| format!("failed to create runtime store {}", family.display()))?;
        let requested_destination = family.join(&spec.version);
        if !spec.is_floating() && requested_destination.exists() {
            prepare_mount_targets(&requested_destination.join("rootfs"))?;
            self.activate(spec)?;
            return Ok(spec.clone());
        }

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let staging = family.join(format!(".install-{}-{nonce}", std::process::id()));
        fs::create_dir(&staging)
            .with_context(|| format!("failed to create staging directory {}", staging.display()))?;

        let resolved = match self.build_codex_rootfs(spec, image, podman, &staging) {
            Ok(resolved) => resolved,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let destination = family.join(&resolved.version);
        if destination.exists() {
            fs::remove_dir_all(&staging)?;
            prepare_mount_targets(&destination.join("rootfs"))?;
            self.activate(&resolved)?;
            return Ok(resolved);
        }
        if let Err(error) = fs::rename(&staging, &destination) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error).with_context(|| {
                format!(
                    "failed to publish runtime {} to {}",
                    resolved.display(),
                    destination.display()
                )
            });
        }
        self.activate(&resolved)?;
        Ok(resolved)
    }

    fn build_codex_rootfs(
        &self,
        spec: &RuntimeSpec,
        image: &str,
        podman: &Path,
        staging: &Path,
    ) -> Result<RuntimeSpec> {
        let selector = if spec.is_floating() {
            format!("{CODEX_PACKAGE}@latest")
        } else {
            format!("{CODEX_PACKAGE}@{}", spec.version)
        };
        let script = format!(
            "resolved=$(npm view {selector} version) && \
             npm install --global --prefix /usr/local \"{CODEX_PACKAGE}@$resolved\" && \
             printf '%s' \"$resolved\" > /driva-runtime-version && \
             mkdir -p /driva /root/.codex /proc /dev /tmp && \
             touch /root/.codex/auth.json && rm -rf /root/.npm"
        );
        let cidfile = staging.join("container.cid");
        let created = Command::new(podman)
            .arg("create")
            .arg("--cidfile")
            .arg(&cidfile)
            .args([image, "/bin/sh", "-ceu", &script])
            .status()
            .with_context(|| format!("failed to start {}", podman.display()))?;
        if !created.success() {
            bail!("Podman could not create the Codex build container: {created}");
        }
        let container = fs::read_to_string(&cidfile)
            .context("Podman did not write the Codex build container id")?
            .trim()
            .to_owned();
        fs::remove_file(&cidfile)?;
        if container.is_empty() {
            bail!("Podman returned an empty container id");
        }

        let build_result = (|| {
            let status = Command::new(podman)
                .args(["start", "--attach", &container])
                .status()
                .context("failed to run the Codex build container")?;
            if !status.success() {
                bail!("Codex runtime installation failed with {status}");
            }

            let resolved_path = staging.join("resolved-version");
            let container_version = format!("{container}:/driva-runtime-version");
            let status = Command::new(podman)
                .arg("cp")
                .arg(&container_version)
                .arg(&resolved_path)
                .status()
                .context("failed to read the resolved Codex version")?;
            if !status.success() {
                bail!("Codex version resolution failed with {status}");
            }
            let resolved_version = fs::read_to_string(&resolved_path)?;
            fs::remove_file(&resolved_path)?;
            let resolved = RuntimeSpec::resolved(&spec.name, resolved_version.trim())?;

            let archive = staging.join("rootfs.tar");
            let status = Command::new(podman)
                .arg("export")
                .arg("--output")
                .arg(&archive)
                .arg(&container)
                .status()
                .context("failed to export the Codex runtime")?;
            if !status.success() {
                bail!("Codex runtime export failed with {status}");
            }

            let rootfs = staging.join("rootfs");
            fs::create_dir(&rootfs)?;
            let status = Command::new("tar")
                .args(["--extract", "--no-same-owner", "--file"])
                .arg(&archive)
                .arg("--directory")
                .arg(&rootfs)
                .status()
                .context("failed to extract the Codex runtime; is tar installed?")?;
            if !status.success() {
                bail!("Codex runtime extraction failed with {status}");
            }
            fs::remove_file(&archive)?;
            prepare_mount_targets(&rootfs)?;
            let marker = rootfs.join("driva-runtime-version");
            if marker.exists() {
                fs::remove_file(marker)?;
            }
            fs::write(
                staging.join("manifest.toml"),
                format!(
                    "name = {:?}\nversion = {:?}\nimage = {:?}\npackage = {:?}\n",
                    resolved.name, resolved.version, image, CODEX_PACKAGE
                ),
            )?;
            Ok(resolved)
        })();

        let cleanup = Command::new(podman)
            .args(["rm", "--force", &container])
            .stdout(Stdio::null())
            .status();
        match cleanup {
            Ok(status) if !status.success() => {
                eprintln!("driva: warning: failed to remove build container {container}: {status}");
            }
            Err(error) => {
                eprintln!("driva: warning: failed to remove build container {container}: {error}");
            }
            Ok(_) => {}
        }
        build_result
    }

    pub fn activate(&self, spec: &RuntimeSpec) -> Result<()> {
        let family = self.root.join(&spec.name);
        let destination = family.join(&spec.version);
        if !destination.join("rootfs").is_dir() {
            bail!("runtime {} is not installed", spec.display());
        }
        let temporary = family.join(format!(".current-{}", std::process::id()));
        remove_path_if_present(&temporary)?;
        create_symlink(Path::new(&spec.version), &temporary)?;
        let current = family.join("current");
        match fs::symlink_metadata(&current) {
            Ok(metadata) if !metadata.file_type().is_symlink() => {
                remove_path_if_present(&temporary)?;
                bail!(
                    "runtime current path is not a symlink: {}",
                    current.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::rename(&temporary, &current).context("failed to activate installed runtime")?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<(RuntimeSpec, bool)>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut runtimes = Vec::new();
        for family in fs::read_dir(&self.root)? {
            let family = family?;
            if !family.file_type()?.is_dir() {
                continue;
            }
            let name = family.file_name().to_string_lossy().into_owned();
            let current = fs::read_link(family.path().join("current")).ok();
            for version in fs::read_dir(family.path())? {
                let version = version?;
                if !version.file_type()?.is_dir() || version.file_name() == OsStr::new("current") {
                    continue;
                }
                let value = version.file_name().to_string_lossy().into_owned();
                if value.starts_with('.') || !version.path().join("rootfs").is_dir() {
                    continue;
                }
                let active = current.as_deref() == Some(Path::new(&value));
                runtimes.push((
                    RuntimeSpec {
                        name: name.clone(),
                        version: value,
                    },
                    active,
                ));
            }
        }
        runtimes.sort_by(|left, right| left.0.display().cmp(&right.0.display()));
        Ok(runtimes)
    }

    pub fn remove(&self, spec: &RuntimeSpec) -> Result<()> {
        let family = self.root.join(&spec.name);
        let destination = family.join(&spec.version);
        if !destination.is_dir() {
            bail!("runtime {} is not installed", spec.display());
        }
        let current = family.join("current");
        if fs::read_link(&current).ok().as_deref() == Some(Path::new(&spec.version)) {
            remove_path_if_present(&current)?;
        }
        fs::remove_dir_all(&destination)
            .with_context(|| format!("failed to remove runtime {}", spec.display()))?;
        Ok(())
    }
}

fn prepare_mount_targets(rootfs: &Path) -> Result<()> {
    for directory in [
        "proc",
        "dev",
        "tmp",
        "driva",
        "root/.codex",
        "etc",
        "etc/driva",
        "usr/local/bin",
    ] {
        fs::create_dir_all(rootfs.join(directory))?;
    }
    for file in ["root/.codex/auth.json", "etc/resolv.conf"] {
        let path = rootfs.join(file);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => fs::remove_file(&path)?,
            Ok(metadata) if metadata.is_file() => continue,
            Ok(_) => bail!("runtime mount target is not a file: {}", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::write(&path, "")?;
    }
    let codex = rootfs.join("usr/local/bin/codex");
    if !codex.is_file() {
        bail!("prepared runtime does not contain {}", codex.display());
    }
    let wrapper = rootfs.join("usr/local/bin/driva-codex");
    fs::write(
        &wrapper,
        "#!/bin/sh\nset -eu\nexec /usr/local/bin/codex \"$@\"\n",
    )?;
    set_executable(&wrapper)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    bail!("prepared Bubblewrap runtimes are supported only on Unix")
}

fn remove_path_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)?
        }
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) -> Result<()> {
    bail!("prepared Bubblewrap runtimes are supported only on Unix")
}
