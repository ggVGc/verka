use crate::base::{Base, BaseConfig, RuntimeEntry};
use crate::{
    effective_policy, EnvironmentEntry, EnvironmentOrigin, ExecutionControl, ExecutionEvidence,
    ExecutionIo, ExecutionOutcome, ExecutionRequest, FloorEntry, FloorKind, Isolation, Mount,
    MountAccess, ProcessExit, WritableMountMode, DEFAULT_PATH,
};
use anyhow::{bail, Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, SystemTime};

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
/// or a private root built from the configured base system.
#[derive(Clone, Debug, Default)]
pub struct BwrapIsolation {
    pub executable: PathBuf,
    /// A prepared root filesystem. When absent, Driva constructs a private
    /// root from `base`.
    pub rootfs: Option<PathBuf>,
    /// The capabilities the private root carries (see [`crate::base`]).
    /// Ignored when a prepared rootfs brings its own system.
    pub base: BaseConfig,
}

impl BwrapIsolation {
    /// A backend using the host's `bwrap` and the default base.
    pub fn new() -> Self {
        Self {
            executable: PathBuf::from("bwrap"),
            ..Self::default()
        }
    }
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
        let rootfs = self.resolve_rootfs()?;
        let temporary_mounts = collect_temporary_mounts(mounts)?;
        if let Some(rootfs) = &rootfs {
            self.validate_rootfs_runtime(rootfs)?;
            self.validate_rootfs_paths(rootfs, request, mounts, &temporary_mounts)?;
        }
        // A prepared rootfs brings its own system, so the base — paths and
        // forwarded variables alike — belongs to the private root only.
        let base = match &rootfs {
            Some(_) => None,
            None => Some(crate::base::resolve_base(&self.base)?),
        };

        let mut command = Command::new(&self.executable);
        append_isolation_options(&mut command, request, &environment(request, base.as_ref()));
        append_floor(
            &mut command,
            &floor(rootfs.as_deref(), request, &temporary_mounts),
            base.as_ref(),
        );
        append_mounts(&mut command, mounts);
        command
            .arg("--chdir")
            .arg(&request.working_directory)
            .arg("--")
            .args(&request.command);
        Ok(command)
    }

    /// What this backend will put in the sandbox on its own, for a request,
    /// beyond the mounts the request names and the base it is built from.
    ///
    /// Reported rather than only built so a caller can state the whole of what
    /// a sandbox holds: the tmpfs root, the `/tmp` every execution gets, and a
    /// created working directory are all writable, and all of them are absent
    /// from the mount list. [`Self::command`] renders its invocation from this
    /// same list, so the two cannot disagree.
    pub fn floor(&self, request: &ExecutionRequest) -> Result<Vec<FloorEntry>> {
        let rootfs = self.resolve_rootfs()?;
        let plan = BwrapMountPlan::new(request);
        let temporary_mounts = collect_temporary_mounts(&plan)?;
        Ok(floor(rootfs.as_deref(), request, &temporary_mounts))
    }

    /// Every environment variable a request will run with, in the order they
    /// are set, each carrying the layer that set it.
    ///
    /// The sandbox starts with an empty environment, so this is the whole of
    /// it and not a difference against the host's. Reported from the same list
    /// [`Self::command`] sets the variables from.
    pub fn environment(&self, request: &ExecutionRequest) -> Result<Vec<EnvironmentEntry>> {
        let base = match self.resolve_rootfs()? {
            Some(_) => None,
            None => Some(crate::base::resolve_base(&self.base)?),
        };
        Ok(environment(request, base.as_ref())
            .into_iter()
            .map(|(name, value, origin)| EnvironmentEntry {
                name: name.to_string_lossy().into_owned(),
                value: value.to_string_lossy().into_owned(),
                origin,
            })
            .collect())
    }

    fn resolve_rootfs(&self) -> Result<Option<PathBuf>> {
        self.rootfs
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
            .transpose()
    }

    fn validate_rootfs_runtime(&self, rootfs: &Path) -> Result<()> {
        self.require_rootfs_directory(rootfs, Path::new("/proc"), "proc mount point")?;
        self.require_rootfs_directory(rootfs, Path::new("/dev"), "device mount point")?;
        self.require_rootfs_directory(rootfs, Path::new("/tmp"), "temporary directory")
    }

    fn validate_rootfs_paths(
        &self,
        rootfs: &Path,
        request: &ExecutionRequest,
        mounts: &BwrapMountPlan,
        temporary_mounts: &[PathBuf],
    ) -> Result<()> {
        for destination in temporary_mounts {
            self.require_rootfs_directory(rootfs, destination, "temporary mount point")?;
        }
        self.require_rootfs_path_or_temporary(
            rootfs,
            temporary_mounts,
            &request.working_directory,
            "working directory",
        )?;
        for mount in mounts.mounts() {
            let destination = match mount {
                Mount::Bind { destination, .. } | Mount::Overlay { destination, .. } => destination,
                Mount::Temporary { .. } => continue,
            };
            self.require_rootfs_path_or_temporary(
                rootfs,
                temporary_mounts,
                destination,
                "mount destination",
            )?;
        }
        Ok(())
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

fn collect_temporary_mounts(mounts: &BwrapMountPlan) -> Result<Vec<PathBuf>> {
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
    Ok(temporary_mounts)
}

/// Everything the backend puts in the sandbox that no mount asked for, in the
/// order Bubblewrap is told to lay it down.
///
/// The root comes first because everything else is laid inside it; the working
/// directory comes last because a mount may land on it. A private root's base
/// entries are not here — they are reported on their own terms (see
/// [`crate::base`]) and appended by [`append_floor`] directly after the root
/// they sit in.
fn floor(
    rootfs: Option<&Path>,
    request: &ExecutionRequest,
    temporary_mounts: &[PathBuf],
) -> Vec<FloorEntry> {
    let mut entries = vec![match rootfs {
        Some(rootfs) => FloorEntry {
            kind: FloorKind::RootFs,
            path: PathBuf::from("/"),
            source: Some(rootfs.to_path_buf()),
        },
        None => FloorEntry {
            kind: FloorKind::Tmpfs,
            path: PathBuf::from("/"),
            source: None,
        },
    }];
    entries.extend([
        FloorEntry {
            kind: FloorKind::Proc,
            path: PathBuf::from("/proc"),
            source: None,
        },
        FloorEntry {
            kind: FloorKind::Devices,
            path: PathBuf::from("/dev"),
            source: None,
        },
        // Every execution gets scratch space here, asked for or not: programs
        // assume it exists, and a sandbox without it fails in ways that have
        // nothing to do with its policy.
        FloorEntry {
            kind: FloorKind::Tmpfs,
            path: PathBuf::from("/tmp"),
            source: None,
        },
    ]);
    entries.extend(
        temporary_mounts
            .iter()
            .filter(|destination| *destination != Path::new("/tmp"))
            .map(|destination| FloorEntry {
                kind: FloorKind::Tmpfs,
                path: destination.clone(),
                source: None,
            }),
    );
    // A prepared rootfs cannot have directories created below it, so there the
    // working directory is one the rootfs already carries.
    if rootfs.is_none() {
        entries.push(FloorEntry {
            kind: FloorKind::Directory,
            path: request.working_directory.clone(),
            source: None,
        });
    }
    entries
}

fn append_floor(command: &mut Command, floor: &[FloorEntry], base: Option<&Base>) {
    for entry in floor {
        match entry.kind {
            FloorKind::Tmpfs => {
                command.arg("--tmpfs").arg(&entry.path);
            }
            FloorKind::RootFs => {
                command
                    .arg("--ro-bind")
                    .arg(entry.source.as_ref().expect("a rootfs entry has a source"))
                    .arg(&entry.path);
            }
            FloorKind::Proc => {
                command.arg("--proc").arg(&entry.path);
            }
            FloorKind::Devices => {
                command.arg("--dev").arg(&entry.path);
            }
            FloorKind::Directory => {
                command.arg("--dir").arg(&entry.path);
            }
        }
        // The base is the private root's contents, so it is laid down as soon
        // as that root exists and before anything is mounted over it.
        if entry.path == Path::new("/") {
            if let Some(base) = base {
                append_base(command, base);
            }
        }
    }
}

/// Every variable the sandbox will hold, in the order they are set.
///
/// The environment is cleared first, so this is the whole of it: the backend's
/// own `PATH`, then the host variables the base forwards, then the request's
/// own. Later layers win, and a name set twice appears once — as the layer
/// whose value the command will actually see.
///
/// Values stay as `OsString` here because that is how they are handed to
/// Bubblewrap; [`BwrapIsolation::environment`] is where they become the
/// reportable [`EnvironmentEntry`].
fn environment(
    request: &ExecutionRequest,
    base: Option<&Base>,
) -> Vec<(OsString, OsString, EnvironmentOrigin)> {
    let mut entries = Vec::new();
    if !request.environment.contains_key(OsStr::new("PATH")) {
        entries.push((
            OsString::from("PATH"),
            OsString::from(DEFAULT_PATH),
            EnvironmentOrigin::Backend,
        ));
    }
    // The base is the floor for the environment the way its entries are the
    // floor for the filesystem: a request value takes precedence.
    if let Some(base) = base {
        entries.extend(
            base.environment()
                .into_iter()
                .filter(|(name, _)| !request.environment.contains_key(name))
                .map(|(name, value)| (name, value, EnvironmentOrigin::Base)),
        );
    }
    entries.extend(
        request
            .environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone(), EnvironmentOrigin::Request)),
    );
    entries
}

fn append_isolation_options(
    command: &mut Command,
    request: &ExecutionRequest,
    environment: &[(OsString, OsString, EnvironmentOrigin)],
) {
    command.arg("--unshare-all");
    if request.new_session {
        command.arg("--new-session");
    }
    command.arg("--die-with-parent");
    if request.network {
        command.arg("--share-net");
    }
    command.arg("--clearenv");
    for (name, value, _) in environment {
        command.arg("--setenv").arg(name).arg(value);
    }
}

fn append_mounts(command: &mut Command, mounts: &BwrapMountPlan) {
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
}

/// Lay the resolved base down inside the root the floor has just created,
/// which for a private root is an empty tmpfs.
///
/// This is the single translation point from a declared base to Bubblewrap's
/// primitives, so what [`crate::base::resolve_base`] reports and what the
/// sandbox holds are the same list.
fn append_base(command: &mut Command, base: &Base) {
    for entry in base.entries() {
        match entry {
            RuntimeEntry::ReadOnly { source, path } => {
                command.arg("--ro-bind").arg(source).arg(path);
            }
            RuntimeEntry::Symlink { target, path } => {
                command.arg("--symlink").arg(target).arg(path);
            }
        }
    }
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
        self.run_inner(request, io, None)
    }

    fn run_controlled(
        &self,
        request: &ExecutionRequest,
        io: ExecutionIo,
        control: &ExecutionControl,
    ) -> Result<ExecutionOutcome> {
        self.run_inner(request, io, Some(control))
    }
}

impl BwrapIsolation {
    fn run_inner(
        &self,
        request: &ExecutionRequest,
        io: ExecutionIo,
        control: Option<&ExecutionControl>,
    ) -> Result<ExecutionOutcome> {
        let started_at = SystemTime::now();
        let mounts = BwrapMountPlan::new(request);
        let private_copies = materialize_private_copies(mounts.mounts())?;
        let child = self
            .command_with_mounts(request, &mounts)?
            .stdin(Stdio::from(io.stdin))
            .stdout(Stdio::from(io.stdout))
            .stderr(Stdio::from(io.stderr))
            .spawn()
            .with_context(|| format!("failed to start {}", self.executable.display()));
        let status = child.and_then(|child| wait_for_child(child, control));
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

fn wait_for_child(mut child: Child, control: Option<&ExecutionControl>) -> Result<ExitStatus> {
    let Some(control) = control else {
        return child.wait().context("waiting for Bubblewrap");
    };
    loop {
        if let Some(status) = child.try_wait().context("checking Bubblewrap status")? {
            return Ok(status);
        }
        if control.termination_requested() {
            // Bubblewrap is launched with --die-with-parent, so killing it also
            // tears down the sandbox process it supervises.
            match child.kill() {
                Ok(()) => return child.wait().context("waiting for terminated Bubblewrap"),
                Err(error) => {
                    if let Some(status) = child
                        .try_wait()
                        .context("checking Bubblewrap after termination failed")?
                    {
                        return Ok(status);
                    }
                    return Err(error).context("terminating Bubblewrap");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn controlled_wait_force_terminates_a_running_child() {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let control = Arc::new(ExecutionControl::default());
        let trigger = Arc::clone(&control);
        let terminator = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            trigger.terminate();
        });

        let started = std::time::Instant::now();
        let status = wait_for_child(child, Some(&control)).unwrap();
        terminator.join().unwrap();

        assert!(!status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
