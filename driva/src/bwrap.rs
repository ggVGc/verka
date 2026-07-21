use crate::{
    effective_policy, ExecutionEvidence, ExecutionIo, ExecutionOutcome, ExecutionRequest,
    Isolation, MountAccess, ProcessExit, DEFAULT_PATH,
};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

/// A synchronous Bubblewrap backend using either a prepared filesystem tree
/// or a private root containing the host's system runtime.
#[derive(Clone, Debug)]
pub struct BwrapIsolation {
    pub executable: PathBuf,
    /// A prepared root filesystem. When absent, Driva constructs a private
    /// root containing only the host's read-only system runtime.
    pub rootfs: Option<PathBuf>,
    pub tmpfs: Vec<PathBuf>,
}

impl BwrapIsolation {
    /// Translate a portable request into a Bubblewrap invocation.
    ///
    /// When a prepared rootfs is configured, Bubblewrap cannot create bind
    /// destinations below it, so the working directory, `/proc`, `/dev`, and
    /// every mount destination must already exist there.
    pub fn command(&self, request: &ExecutionRequest) -> Result<Command> {
        let rootfs = self
            .rootfs
            .as_deref()
            .map(|configured| {
                let configured = expand_home(configured)?;
                let rootfs = configured.canonicalize().with_context(|| {
                    format!("invalid Bubblewrap rootfs {}", configured.display())
                })?;
                if !rootfs.is_dir() {
                    bail!("Bubblewrap rootfs is not a directory: {}", rootfs.display());
                }
                Ok(rootfs)
            })
            .transpose()?;

        if let Some(rootfs) = &rootfs {
            self.require_rootfs_directory(rootfs, Path::new("/proc"), "proc mount point")?;
            self.require_rootfs_directory(rootfs, Path::new("/dev"), "device mount point")?;
            self.require_rootfs_directory(rootfs, Path::new("/tmp"), "temporary directory")?;
        }
        let mut tmpfs = Vec::new();
        for destination in &self.tmpfs {
            let destination = expand_home(destination)?;
            if !tmpfs.contains(&destination) {
                tmpfs.push(destination);
            }
        }
        for destination in &tmpfs {
            if let Some(rootfs) = &rootfs {
                self.require_rootfs_directory(rootfs, destination, "tmpfs mount point")?;
            }
        }
        if let Some(rootfs) = &rootfs {
            self.require_rootfs_path_or_tmpfs(
                rootfs,
                &tmpfs,
                &request.working_directory,
                "working directory",
            )?;
            for mount in &request.mounts {
                self.require_rootfs_path_or_tmpfs(
                    rootfs,
                    &tmpfs,
                    &mount.destination,
                    "mount destination",
                )?;
            }
        }

        let mut command = Command::new(&self.executable);
        command
            .arg("--unshare-all")
            .arg("--new-session")
            .arg("--die-with-parent");
        if request.network {
            command.arg("--share-net");
        }
        command
            .arg("--clearenv")
            .arg("--setenv")
            .arg("PATH")
            .arg(DEFAULT_PATH);
        for (key, value) in &request.environment {
            command.arg("--setenv").arg(key).arg(value);
        }
        if let Some(rootfs) = &rootfs {
            command.arg("--ro-bind").arg(rootfs).arg("/");
        } else {
            append_host_runtime(&mut command)?;
        }
        command
            .arg("--proc")
            .arg("/proc")
            .arg("--dev")
            .arg("/dev")
            .arg("--tmpfs")
            .arg("/tmp");
        for destination in &tmpfs {
            command.arg("--tmpfs").arg(destination);
        }
        if rootfs.is_none() {
            command.arg("--dir").arg(&request.working_directory);
        }
        for mount in &request.mounts {
            command.arg(match mount.access {
                MountAccess::ReadOnly => "--ro-bind",
                MountAccess::ReadWrite => "--bind",
            });
            command.arg(&mount.source).arg(&mount.destination);
        }
        command
            .arg("--chdir")
            .arg(&request.working_directory)
            .arg("--")
            .args(&request.command);
        Ok(command)
    }

    fn require_rootfs_directory(&self, rootfs: &Path, path: &Path, label: &str) -> Result<()> {
        let resolved = self.require_rootfs_path(rootfs, path, label)?;
        if !resolved.is_dir() {
            bail!(
                "Bubblewrap {label} is not a directory in the rootfs: {}",
                path.display()
            );
        }
        Ok(())
    }

    fn require_rootfs_path_or_tmpfs(
        &self,
        rootfs: &Path,
        tmpfs: &[PathBuf],
        path: &Path,
        label: &str,
    ) -> Result<()> {
        if is_nested_beneath(path, Path::new("/tmp"))
            || tmpfs.iter().any(|base| is_nested_beneath(path, base))
        {
            return Ok(());
        }
        self.require_rootfs_path(rootfs, path, label).map(|_| ())
    }

    fn require_rootfs_path(&self, rootfs: &Path, path: &Path, label: &str) -> Result<PathBuf> {
        let relative = path
            .strip_prefix("/")
            .with_context(|| format!("Bubblewrap {label} must be absolute: {}", path.display()))?;
        let candidate = rootfs.join(relative);
        let resolved = candidate.canonicalize().with_context(|| {
            format!(
                "Bubblewrap {label} does not exist in the rootfs: {}",
                path.display()
            )
        })?;
        if !resolved.starts_with(rootfs) {
            bail!(
                "Bubblewrap {label} escapes the rootfs through a symlink: {}",
                path.display()
            );
        }
        Ok(resolved)
    }
}

/// Construct a useful base filesystem without exposing the host root, home,
/// current directory, or other data paths. The small set of conventional
/// system paths is enough to run the host's `/bin/sh` and normal OS tools.
fn append_host_runtime(command: &mut Command) -> Result<()> {
    command.arg("--tmpfs").arg("/");

    for path in [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/nix/store",
        "/gnu/store",
    ] {
        append_runtime_path(command, Path::new(path))?;
    }
    for path in [
        "/etc/alternatives",
        "/etc/ca-certificates",
        "/etc/group",
        "/etc/hosts",
        "/etc/ld.so.cache",
        "/etc/ld.so.conf",
        "/etc/ld.so.conf.d",
        "/etc/localtime",
        "/etc/nsswitch.conf",
        "/etc/passwd",
        "/etc/pki",
        "/etc/protocols",
        "/etc/resolv.conf",
        "/etc/services",
        "/etc/ssl",
    ] {
        append_runtime_path(command, Path::new(path))?;
    }
    Ok(())
}

fn append_runtime_path(command: &mut Command, path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect host runtime path {}", path.display()))
        }
    };
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path)
            .with_context(|| format!("failed to read host runtime link {}", path.display()))?;
        command.arg("--symlink").arg(target).arg(path);
    } else {
        command.arg("--ro-bind").arg(path).arg(path);
    }
    Ok(())
}

fn is_nested_beneath(path: &Path, base: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(base) else {
        return false;
    };
    relative.components().next().is_some()
        && relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn expand_home(path: &Path) -> Result<PathBuf> {
    if path == Path::new("~") || path.starts_with("~/") {
        let home = std::env::var_os("HOME").context("HOME is not set; cannot expand rootfs")?;
        Ok(PathBuf::from(home).join(path.strip_prefix("~").expect("prefix checked")))
    } else {
        Ok(path.to_path_buf())
    }
}

impl Isolation for BwrapIsolation {
    fn run(&self, request: &ExecutionRequest, io: ExecutionIo) -> Result<ExecutionOutcome> {
        let started_at = SystemTime::now();
        let status = self
            .command(request)?
            .stdin(Stdio::from(io.stdin))
            .stdout(Stdio::from(io.stdout))
            .stderr(Stdio::from(io.stderr))
            .status()
            .with_context(|| format!("failed to start {}", self.executable.display()))?;
        Ok(ExecutionOutcome {
            exit: ProcessExit::from(status),
            evidence: ExecutionEvidence {
                isolation_backend: "bwrap".into(),
                effective_policy: effective_policy(request),
                started_at,
                finished_at: SystemTime::now(),
            },
        })
    }
}
