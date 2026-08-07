use crate::{
    effective_policy, ExecutionEvidence, ExecutionIo, ExecutionOutcome, ExecutionRequest,
    Isolation, Mount, MountAccess, ProcessExit, WritableMountMode, DEFAULT_PATH,
};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

/// Concrete filesystem operations used to construct one Bubblewrap sandbox.
/// This is the single translation point from portable mount intent to
/// Bubblewrap's bind and overlay primitives.
struct BwrapMountPlan(Vec<Mount>);

impl BwrapMountPlan {
    fn new(request: &ExecutionRequest) -> Self {
        Self(
            request
                .mounts
                .iter()
                .cloned()
                .map(|mount| match (request.writable_mounts, mount) {
                    (
                        WritableMountMode::Overlay,
                        Mount::Bind {
                            source,
                            destination,
                            access: MountAccess::ReadWrite,
                        },
                    ) => Mount::Overlay {
                        source,
                        destination,
                    },
                    (_, mount) => mount,
                })
                .collect(),
        )
        .with_nested_mounts_last()
    }

    /// Bubblewrap applies mounts in argument order, so a mount whose
    /// destination contains another's must come first or it hides it. A
    /// broad `--read` therefore has to be laid down before the narrower
    /// writable destinations nested inside it. The sort is stable, so
    /// mounts at the same depth keep their configured precedence.
    fn with_nested_mounts_last(mut self) -> Self {
        self.0
            .sort_by_key(|mount| mount.destination().components().count());
        self
    }

    fn mounts(&self) -> &[Mount] {
        &self.0
    }

    fn into_mounts(self) -> Vec<Mount> {
        self.0
    }
}

/// A synchronous Bubblewrap backend using either a prepared filesystem tree
/// or a private root containing the host's system runtime.
#[derive(Clone, Debug)]
pub struct BwrapIsolation {
    pub executable: PathBuf,
    /// A prepared root filesystem. When absent, Driva constructs a private
    /// root containing only the host's read-only system runtime.
    pub rootfs: Option<PathBuf>,
}

impl BwrapIsolation {
    /// Translate a portable request into a Bubblewrap invocation.
    ///
    /// When a prepared rootfs is configured, Bubblewrap cannot create bind
    /// destinations below it, so the working directory, `/proc`, `/dev`, and
    /// every mount destination must already exist there.
    pub fn command(&self, request: &ExecutionRequest) -> Result<Command> {
        let mounts = BwrapMountPlan::new(request);
        self.command_with_mounts(request, &mounts)
    }

    fn command_with_mounts(
        &self,
        request: &ExecutionRequest,
        mounts: &BwrapMountPlan,
    ) -> Result<Command> {
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
        let mut temporary_mounts = Vec::new();
        for mount in mounts.mounts() {
            let Mount::Temporary { destination } = mount else {
                continue;
            };
            let destination = crate::expand_home(destination, "temporary mount destination")?;
            if !temporary_mounts.contains(&destination) {
                temporary_mounts.push(destination);
            }
        }
        temporary_mounts.sort_by_key(|destination| destination.components().count());
        for destination in &temporary_mounts {
            if let Some(rootfs) = &rootfs {
                self.require_rootfs_directory(rootfs, destination, "temporary mount point")?;
            }
        }
        if let Some(rootfs) = &rootfs {
            self.require_rootfs_path_or_temporary(
                rootfs,
                &temporary_mounts,
                &request.working_directory,
                "working directory",
            )?;
            for mount in mounts.mounts() {
                let destination = match mount {
                    Mount::Bind { destination, .. } | Mount::Overlay { destination, .. } => {
                        destination
                    }
                    Mount::Temporary { .. } => continue,
                };
                self.require_rootfs_path_or_temporary(
                    rootfs,
                    &temporary_mounts,
                    destination,
                    "mount destination",
                )?;
            }
        }

        let mut command = Command::new(&self.executable);
        command.arg("--unshare-all");
        if request.new_session {
            command.arg("--new-session");
        }
        command.arg("--die-with-parent");
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
        for destination in &temporary_mounts {
            if destination != Path::new("/tmp") {
                command.arg("--tmpfs").arg(destination);
            }
        }
        if rootfs.is_none() {
            command.arg("--dir").arg(&request.working_directory);
        }
        for mount in mounts.mounts() {
            match mount {
                Mount::Bind {
                    source,
                    destination,
                    access,
                } => {
                    command.arg(match access {
                        MountAccess::ReadOnly => "--ro-bind",
                        MountAccess::ReadWrite => "--bind",
                    });
                    command.arg(source).arg(destination);
                }
                Mount::Overlay {
                    source,
                    destination,
                } if is_regular_file(source) => {
                    // Overlayfs stacks only on directories, so a file source gets a
                    // private copy bound in its place. `run` materialises the copy
                    // before the invocation starts and removes it afterwards.
                    command
                        .arg("--bind")
                        .arg(private_copy_path(destination))
                        .arg(destination);
                }
                Mount::Overlay {
                    source,
                    destination,
                } => {
                    command
                        .arg("--overlay-src")
                        .arg(source)
                        .arg("--tmp-overlay")
                        .arg(destination);
                }
                Mount::Temporary { .. } => continue,
            }
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

    fn require_rootfs_path_or_temporary(
        &self,
        rootfs: &Path,
        temporary_mounts: &[PathBuf],
        path: &Path,
        label: &str,
    ) -> Result<()> {
        if is_nested_beneath(path, Path::new("/tmp"))
            || temporary_mounts
                .iter()
                .any(|base| is_nested_beneath(path, base))
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

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

/// Host directory holding the private copies of overlaid files for this
/// process. One directory per process keeps concurrent runs independent.
fn private_copy_root() -> PathBuf {
    std::env::temp_dir().join(format!("driva-overlay-{}", std::process::id()))
}

/// Map an isolated destination to its private copy on the host. The escaping
/// keeps the mapping injective, so two overlaid files never share a copy.
fn private_copy_path(destination: &Path) -> PathBuf {
    let name: String = destination
        .to_string_lossy()
        .chars()
        .map(|character| match character {
            '%' => "%%".to_string(),
            '/' => "%".to_string(),
            other => other.to_string(),
        })
        .collect();
    private_copy_root().join(name)
}

/// Copy every overlaid file into this process's private directory, so the
/// sandbox writes to the copy and the host source is never mutated. Returns the
/// directory to remove once the invocation finishes.
fn materialize_private_copies(mounts: &[Mount]) -> Result<Option<PathBuf>> {
    let sources: Vec<_> = mounts
        .iter()
        .filter_map(|mount| match mount {
            Mount::Overlay {
                source,
                destination,
            } if is_regular_file(source) => Some((source, destination)),
            _ => None,
        })
        .collect();
    if sources.is_empty() {
        return Ok(None);
    }
    let root = private_copy_root();
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create overlay directory {}", root.display()))?;
    restrict_to_owner(&root)?;
    for (source, destination) in sources {
        let copy = private_copy_path(destination);
        std::fs::copy(source, &copy).with_context(|| {
            format!(
                "failed to copy {} for a discarded-write overlay",
                source.display()
            )
        })?;
        make_owner_writable(&copy)?;
    }
    Ok(Some(root))
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to restrict {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) -> Result<()> {
    Ok(())
}

/// A read-only source copies to a read-only file, which the sandbox could not
/// write to; the copy is private, so widening the owner bits is safe.
#[cfg(unix)]
fn make_owner_writable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?
        .permissions()
        .mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o600))
        .with_context(|| format!("failed to make {} writable", path.display()))
}

#[cfg(not(unix))]
fn make_owner_writable(_path: &Path) -> Result<()> {
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
        let mounts = BwrapMountPlan::new(request);
        let private_copies = materialize_private_copies(mounts.mounts())?;
        let status = self
            .command_with_mounts(request, &mounts)?
            .stdin(Stdio::from(io.stdin))
            .stdout(Stdio::from(io.stdout))
            .stderr(Stdio::from(io.stderr))
            .status()
            .with_context(|| format!("failed to start {}", self.executable.display()));
        if let Some(root) = private_copies {
            let _ = std::fs::remove_dir_all(root);
        }
        let status = status?;
        Ok(ExecutionOutcome {
            exit: ProcessExit::from(status),
            evidence: ExecutionEvidence {
                isolation_backend: "bwrap".into(),
                effective_policy: {
                    let mut policy = effective_policy(request);
                    policy.mounts = mounts.into_mounts();
                    policy
                },
                started_at,
                finished_at: SystemTime::now(),
            },
        })
    }
}
