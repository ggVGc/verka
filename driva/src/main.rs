use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use driva::{
    execute, validate_request, BwrapIsolation, Config, DockerIsolation, ExecutionIo,
    ExecutionRequest, Mount, MountAccess, PodmanIsolation,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(about = "Run a command with explicit, deny-by-default isolation")]
struct Cli {
    /// Configuration file (defaults to ./driva.toml when present).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand)]
enum Operation {
    /// Run a command in a disposable isolated environment.
    Run {
        #[command(flatten)]
        policy: PolicyArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Open /bin/sh in a disposable isolated environment.
    Shell {
        #[command(flatten)]
        policy: PolicyArgs,
    },
    /// Start a durable isolated session and print its id.
    Start {
        #[command(flatten)]
        policy: PolicyArgs,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Attach the terminal to a durable session.
    Attach { session: driva::SessionId },
    /// Inspect backend-authoritative session state.
    Inspect { session: driva::SessionId },
    /// Wait for a session and return its exit status.
    Wait { session: driva::SessionId },
    /// Gracefully terminate a session and return its exit status.
    Terminate {
        session: driva::SessionId,
        #[arg(long, default_value_t = 10)]
        grace: u64,
    },
    /// Remove a session resource and its local record after confirming absence.
    Remove { session: driva::SessionId },
    /// List recorded sessions and their current backend states.
    List,
    /// Rediscover and inspect recorded sessions.
    Recover,
    /// List built-in and project-defined execution templates.
    Templates,
    /// Manage prepared read-only runtimes for Bubblewrap templates.
    Runtime {
        #[command(subcommand)]
        command: RuntimeOperation,
    },
}

#[derive(Subcommand)]
enum RuntimeOperation {
    /// Build and install a runtime such as codex@latest.
    Install {
        /// Runtime selector in NAME@VERSION form; VERSION may be latest.
        runtime: String,
        /// Container image used to prepare the runtime filesystem.
        #[arg(
            long,
            default_value_t = driva::RuntimeStore::default_build_image().to_owned()
        )]
        image: String,
    },
    /// List installed runtime versions.
    List,
    /// Remove an installed runtime version.
    Remove {
        /// Pinned runtime in NAME@VERSION form.
        runtime: String,
    },
}

#[derive(Args, Default)]
struct PolicyArgs {
    /// Apply a named execution template.
    #[arg(long, value_name = "NAME")]
    template: Option<String>,
    /// Add a read-only mount as SOURCE or SOURCE:DESTINATION.
    #[arg(long = "read", value_name = "MOUNT")]
    reads: Vec<String>,
    /// Add a writable mount as SOURCE or SOURCE:DESTINATION.
    #[arg(long = "write", value_name = "MOUNT")]
    writes: Vec<String>,
    /// Make every host mount read-only, overriding configuration and templates.
    #[arg(long)]
    no_write: bool,
    /// Add a host directory read-only and prepend it to the isolated PATH.
    #[arg(long = "path", value_name = "DIRECTORY")]
    paths: Vec<PathBuf>,
    /// Select the isolation backend.
    #[arg(long, value_name = "BACKEND")]
    backend: Option<String>,
    /// Permit networking (disabled otherwise).
    #[arg(long, conflicts_with = "no_network")]
    network: bool,
    /// Disable networking, overriding configuration and templates.
    #[arg(long, conflicts_with = "network")]
    no_network: bool,
    /// Allocate an interactive terminal.
    #[arg(short, long, conflicts_with = "no_interactive")]
    interactive: bool,
    /// Disable interactivity, overriding a template.
    #[arg(long, conflicts_with = "interactive")]
    no_interactive: bool,
    /// Print the validated request and backend invocation without executing it.
    #[arg(long)]
    dry_run: bool,
    /// Override the configured container image.
    #[arg(long)]
    image: Option<String>,
    /// Override the Bubblewrap root filesystem.
    #[arg(long, value_name = "DIRECTORY")]
    rootfs: Option<PathBuf>,
    /// Add a private writable Bubblewrap tmpfs mount.
    #[arg(long, value_name = "DIRECTORY")]
    tmpfs: Vec<PathBuf>,
    /// Override the isolated working directory.
    #[arg(long)]
    workdir: Option<PathBuf>,
    /// Set an environment variable as NAME=VALUE.
    #[arg(long = "env", value_parser = parse_environment)]
    environment: Vec<(OsString, OsString)>,
}

#[derive(Debug)]
enum ResolvedBackend {
    Podman {
        image: String,
    },
    Docker {
        image: String,
    },
    Bwrap {
        rootfs: Option<PathBuf>,
        tmpfs: Vec<PathBuf>,
    },
}

impl ResolvedBackend {
    fn name(&self) -> &'static str {
        match self {
            Self::Podman { .. } => "podman",
            Self::Docker { .. } => "docker",
            Self::Bwrap { .. } => "bwrap",
        }
    }
}

fn main() {
    if let Err(error) = real_main() {
        eprintln!("driva: {error:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let config = match cli.config {
        Some(ref path) => Config::load(path)?,
        None => Config::discover()?,
    };
    let operation = cli.command;
    if !matches!(
        operation,
        Operation::Run { .. } | Operation::Shell { .. } | Operation::Start { .. }
    ) {
        return lifecycle(&config, operation);
    }
    let (policy, mut command, shell, durable) = match operation {
        Operation::Run { policy, command } => (policy, command, false, false),
        Operation::Shell { mut policy } => {
            policy.interactive = true;
            (policy, vec![OsString::from("/bin/sh")], true, false)
        }
        Operation::Start { policy, command } => (policy, command, false, true),
        _ => unreachable!(),
    };
    let mut template = policy
        .template
        .as_deref()
        .map(|name| {
            config.template(name).with_context(|| {
                format!(
                    "unknown template {name:?}; run `driva templates` to list available templates"
                )
            })
        })
        .transpose()?;
    if let Some(template) = &mut template {
        resolve_workspace(template)?;
    }
    if !shell {
        if let Some(template) = &template {
            let mut template_command: Vec<OsString> =
                template.command.iter().map(OsString::from).collect();
            template_command.append(&mut command);
            command = template_command;
        }
    }
    let requested_backend = policy
        .backend
        .as_deref()
        .or_else(|| template.as_ref().and_then(|value| value.backend.as_deref()))
        .unwrap_or(&config.isolation.backend);
    let backend = resolve_backend(requested_backend, &policy, template.as_ref(), &config)?;
    let backend_name = backend.name();
    let configured_workdir = match backend_name {
        "podman" => &config.isolation.podman.workdir,
        "docker" => &config.isolation.docker.workdir,
        "bwrap" => &config.isolation.bwrap.workdir,
        backend => bail!("unsupported isolation backend {backend:?}"),
    };
    let workdir = policy
        .workdir
        .clone()
        .or_else(|| template.as_ref().and_then(|value| value.workdir.clone()))
        .unwrap_or_else(|| configured_workdir.clone());
    let mut mounts: Vec<Mount> = config
        .mounts
        .into_iter()
        .map(|mount| Mount {
            source: mount.source,
            destination: mount.destination,
            access: mount.access,
        })
        .collect();
    if let Some(template) = &template {
        mounts.extend(template.mounts.iter().cloned().map(|mount| Mount {
            source: mount.source,
            destination: mount.destination,
            access: mount.access,
        }));
    }
    for spec in &policy.reads {
        mounts.push(parse_mount(spec, MountAccess::ReadOnly, &workdir)?);
    }
    for spec in &policy.writes {
        mounts.push(parse_mount(spec, MountAccess::ReadWrite, &workdir)?);
    }
    let mut environment: BTreeMap<OsString, OsString> = config.environment;
    if let Some(template) = &template {
        environment.extend(
            template
                .environment
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        );
    }
    environment.extend(policy.environment.iter().cloned());
    let mut paths = template
        .as_ref()
        .map(|value| value.paths.clone())
        .unwrap_or_default();
    paths.extend(policy.paths.iter().cloned());
    add_path_directories(&paths, &mut mounts, &mut environment)?;
    if policy.no_write {
        for mount in &mut mounts {
            mount.access = MountAccess::ReadOnly;
        }
    }
    if shell {
        environment
            .entry(OsString::from("HOME"))
            .or_insert_with(|| OsString::from("/tmp"));
        environment
            .entry(OsString::from("TERM"))
            .or_insert_with(|| OsString::from("xterm-256color"));
    }
    let request = ExecutionRequest {
        command,
        working_directory: workdir,
        mounts,
        environment,
        network: if policy.no_network {
            false
        } else if policy.network {
            true
        } else {
            template
                .as_ref()
                .and_then(|value| value.network)
                .unwrap_or(config.network.enabled)
        },
        interactive: shell
            || if policy.no_interactive {
                false
            } else if policy.interactive {
                true
            } else {
                template
                    .as_ref()
                    .and_then(|value| value.interactive)
                    .unwrap_or(false)
            },
    };
    let request = validate_request(&request)?;
    match backend {
        ResolvedBackend::Podman { image } => {
            let backend = PodmanIsolation {
                executable: config.isolation.podman.executable,
                image,
            };
            let invocation = backend.command(&request);
            if durable {
                start_session(&backend, request, policy.dry_run, invocation)
            } else {
                finish("podman", &backend, invocation, &request, policy.dry_run)
            }
        }
        ResolvedBackend::Docker { image } => {
            let backend = DockerIsolation {
                executable: config.isolation.docker.executable,
                image,
            };
            let invocation = backend.command(&request);
            if durable {
                start_session(&backend, request, policy.dry_run, invocation)
            } else {
                finish("docker", &backend, invocation, &request, policy.dry_run)
            }
        }
        ResolvedBackend::Bwrap { rootfs, tmpfs } => {
            if durable {
                bail!("Bubblewrap does not support durable sessions; use `driva run` or `driva shell`");
            }
            let backend = BwrapIsolation {
                executable: config.isolation.bwrap.executable,
                rootfs,
                tmpfs,
            };
            let invocation = backend.command(&request).with_context(|| {
                if matches!(policy.template.as_deref(), Some("codex" | "codex-exec")) {
                    "Codex runtime is unavailable; run `driva runtime install codex@VERSION`"
                } else {
                    "failed to construct Bubblewrap invocation"
                }
            })?;
            finish("bwrap", &backend, invocation, &request, policy.dry_run)
        }
    }
}

fn resolve_backend(
    name: &str,
    policy: &PolicyArgs,
    template: Option<&driva::TemplateConfig>,
    config: &Config,
) -> Result<ResolvedBackend> {
    let template_image = template.and_then(|value| value.image.clone());
    let template_rootfs = template.and_then(|value| value.rootfs.clone());
    let template_tmpfs = template
        .map(|value| value.tmpfs.clone())
        .unwrap_or_default();

    match name {
        "podman" | "docker" => {
            if policy.rootfs.is_some() || template_rootfs.is_some() {
                bail!("--rootfs is only supported by the Bubblewrap backend");
            }
            if !policy.tmpfs.is_empty() || !template_tmpfs.is_empty() {
                bail!("--tmpfs is only supported by the Bubblewrap backend");
            }
            let configured_image = if name == "podman" {
                &config.isolation.podman.image
            } else {
                &config.isolation.docker.image
            };
            let image = policy
                .image
                .clone()
                .or(template_image)
                .unwrap_or_else(|| configured_image.clone());
            Ok(if name == "podman" {
                ResolvedBackend::Podman { image }
            } else {
                ResolvedBackend::Docker { image }
            })
        }
        "bwrap" => {
            if policy.image.is_some() || template_image.is_some() {
                bail!("--image is not supported by the Bubblewrap backend; use --rootfs");
            }
            let mut tmpfs = template_tmpfs;
            for path in &policy.tmpfs {
                if !tmpfs.contains(path) {
                    tmpfs.push(path.clone());
                }
            }
            Ok(ResolvedBackend::Bwrap {
                rootfs: policy
                    .rootfs
                    .clone()
                    .or(template_rootfs)
                    .or_else(|| config.isolation.bwrap.rootfs.clone()),
                tmpfs,
            })
        }
        backend => bail!("unsupported isolation backend {backend:?}"),
    }
}

/// Mount a template workspace below its configured sandbox root while
/// preserving the canonical host path.
fn resolve_workspace(template: &mut driva::TemplateConfig) -> Result<()> {
    let Some(root) = &template.workspace_root else {
        if template.codex_trust_workspace {
            bail!("codex_trust_workspace requires workspace_root");
        }
        return Ok(());
    };
    if !root.is_absolute() {
        bail!(
            "template workspace_root must be absolute: {}",
            root.display()
        );
    }
    let host_path = std::fs::canonicalize(".").context("failed to resolve the current project")?;
    let relative = host_path
        .strip_prefix("/")
        .context("the current project path is not absolute")?;
    let destination = root.join(relative);
    template.workdir = Some(destination.clone());
    template.mounts.push(driva::MountConfig {
        source: PathBuf::from("."),
        destination: destination.clone(),
        access: MountAccess::ReadWrite,
    });

    if template.codex_trust_workspace {
        let destination = destination
            .to_str()
            .context("the current project path is not valid UTF-8")?;
        if template.command.is_empty() {
            bail!("codex_trust_workspace requires a template command");
        }
        template.command.splice(
            1..1,
            [
                "-c".to_owned(),
                format!("projects.{destination:?}.trust_level=\"trusted\""),
            ],
        );
    }
    Ok(())
}

fn runner_backend(config: &Config) -> Result<Box<dyn driva::DurableIsolation>> {
    Ok(match config.isolation.backend.as_str() {
        "podman" => Box::new(PodmanIsolation {
            executable: config.isolation.podman.executable.clone(),
            image: config.isolation.podman.image.clone(),
        }),
        "docker" => Box::new(DockerIsolation {
            executable: config.isolation.docker.executable.clone(),
            image: config.isolation.docker.image.clone(),
        }),
        "bwrap" => bail!(
            "Bubblewrap does not support durable session commands; use `driva run` or `driva shell`"
        ),
        b => bail!("unsupported isolation backend {b:?}"),
    })
}

fn lifecycle(config: &Config, operation: Operation) -> Result<()> {
    if matches!(operation, Operation::Templates) {
        for (name, template) in config.effective_templates() {
            println!("{name}\t{}", template.description);
        }
        return Ok(());
    }
    if let Operation::Runtime { command } = operation {
        return runtime_command(config, command);
    }
    let backend = runner_backend(config)?;
    let runner = driva::SessionRunner::new(
        backend.as_ref(),
        driva::SessionStore::new(driva::SessionStore::default_path()),
    );
    match operation {
        Operation::Attach { session } => {
            let exit = runner.attach(&session, ExecutionIo::inherited()?)?;
            std::process::exit(exit.code())
        }
        Operation::Inspect { session } => {
            let s = runner.inspect(&session)?;
            println!("{} {} {}", s.record.id, s.record.backend, s.observed);
        }
        Operation::Wait { session } => {
            let o = runner.wait(&session)?;
            std::process::exit(o.exit.code())
        }
        Operation::Terminate { session, grace } => {
            let o = runner.terminate(&session, std::time::Duration::from_secs(grace))?;
            println!(
                "session {session} stopped (exit {}); use `driva remove {session}` to delete it",
                o.exit.code()
            );
            std::process::exit(o.exit.code())
        }
        Operation::Remove { session } => {
            let o = runner.remove(&session)?;
            if o.state != driva::ObservedProcessState::Missing {
                bail!("backend resource still present: {}", o.state)
            }
            println!("session {session} removed");
        }
        Operation::List => {
            for r in runner.store.list()? {
                let state = runner.inspect(&r.id)?.observed;
                println!("{}\t{}\t{}{}", r.id, r.backend, state, incomplete_note(&r))
            }
        }
        Operation::Recover => {
            for s in runner.recover()? {
                println!(
                    "{}\t{}\t{}{}",
                    s.record.id,
                    s.record.backend,
                    s.observed,
                    incomplete_note(&s.record)
                )
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn runtime_command(config: &Config, command: RuntimeOperation) -> Result<()> {
    let store = driva::RuntimeStore::new(driva::RuntimeStore::default_path()?);
    match command {
        RuntimeOperation::Install { runtime, image } => {
            let spec = driva::RuntimeSpec::parse(&runtime)?;
            println!("Preparing {} from {image}...", spec.display());
            let resolved =
                store.install_codex(&spec, &image, &config.isolation.podman.executable)?;
            println!("Installed and activated {}", resolved.display());
        }
        RuntimeOperation::List => {
            for (spec, active) in store.list()? {
                println!(
                    "{}{}",
                    spec.display(),
                    if active { "\tcurrent" } else { "" }
                );
            }
        }
        RuntimeOperation::Remove { runtime } => {
            let spec = driva::RuntimeSpec::parse(&runtime)?;
            if spec.is_floating() {
                bail!("runtime remove requires a concrete version, not latest");
            }
            store.remove(&spec)?;
            println!("Removed {}", spec.display());
        }
    }
    Ok(())
}

fn incomplete_note(record: &driva::SessionRecord) -> &'static str {
    if record.metadata_incomplete {
        "\t(recovered; metadata incomplete)"
    } else {
        ""
    }
}

fn start_session(
    backend: &dyn driva::DurableIsolation,
    request: ExecutionRequest,
    dry_run: bool,
    invocation: Command,
) -> Result<()> {
    if dry_run {
        print_dry_run(backend.backend_name(), invocation, &request);
        return Ok(());
    }
    let runner = driva::SessionRunner::new(
        backend,
        driva::SessionStore::new(driva::SessionStore::default_path()),
    );
    println!("{}", runner.start(request)?.record.id);
    Ok(())
}

fn finish(
    name: &str,
    backend: &dyn driva::Isolation,
    invocation: Command,
    request: &ExecutionRequest,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        print_dry_run(name, invocation, request);
        Ok(())
    } else {
        let outcome = execute(backend, request, ExecutionIo::inherited()?)?;
        std::process::exit(outcome.exit.code());
    }
}

fn parse_environment(value: &str) -> Result<(OsString, OsString), String> {
    let (key, value) = value
        .split_once('=')
        .ok_or_else(|| "expected NAME=VALUE".to_string())?;
    if key.is_empty() {
        return Err("environment variable name cannot be empty".into());
    }
    Ok((key.into(), value.into()))
}

fn parse_mount(spec: &str, access: MountAccess, workdir: &Path) -> Result<Mount> {
    let (source, explicit_destination) = match spec.split_once(':') {
        Some((source, destination)) if !destination.is_empty() => {
            (source, Some(PathBuf::from(destination)))
        }
        _ => (spec, None),
    };
    if source.is_empty() {
        bail!("mount source cannot be empty");
    }
    let source = PathBuf::from(source);
    let destination = explicit_destination.unwrap_or_else(|| {
        if source == Path::new(".") {
            workdir.to_path_buf()
        } else if source.is_absolute() {
            source.clone()
        } else {
            workdir.join(&source)
        }
    });
    Ok(Mount {
        source,
        destination,
        access,
    })
}

/// Mount PATH additions at their canonical host locations so tools that find
/// adjacent state through the executable path keep working inside isolation.
fn add_path_directories(
    directories: &[PathBuf],
    mounts: &mut Vec<Mount>,
    environment: &mut BTreeMap<OsString, OsString>,
) -> Result<()> {
    if directories.is_empty() {
        return Ok(());
    }

    let mut path = OsString::new();
    for (index, directory) in directories.iter().enumerate() {
        let expanded = expand_home(directory)?;
        let source = expanded
            .canonicalize()
            .with_context(|| format!("invalid PATH directory {}", directory.display()))?;
        if !source.is_dir() {
            bail!("PATH addition is not a directory: {}", directory.display());
        }
        let destination = source.clone();
        if index > 0 {
            path.push(":");
        }
        path.push(&destination);
        mounts.push(Mount {
            source,
            destination,
            access: MountAccess::ReadOnly,
        });
    }

    let key = OsString::from("PATH");
    if let Some(configured) = environment.get(&key) {
        if !configured.is_empty() {
            path.push(":");
            path.push(configured);
        }
    } else {
        path.push(":");
        path.push(driva::DEFAULT_PATH);
    }
    environment.insert(key, path);
    Ok(())
}

fn expand_home(path: &Path) -> Result<PathBuf> {
    if path == Path::new("~") || path.starts_with("~/") {
        let home =
            std::env::var_os("HOME").context("HOME is not set; cannot expand PATH directory")?;
        Ok(PathBuf::from(home).join(path.strip_prefix("~").expect("prefix checked")))
    } else {
        Ok(path.to_path_buf())
    }
}

fn print_dry_run(name: &str, command: Command, request: &ExecutionRequest) {
    println!("backend: {name}");
    println!(
        "network: {}",
        if request.network {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("interactive: {}", request.interactive);
    println!("working-directory: {}", request.working_directory.display());
    for mount in &request.mounts {
        println!(
            "mount: {} -> {} ({})",
            mount.source.display(),
            mount.destination.display(),
            if mount.access == MountAccess::ReadOnly {
                "read-only"
            } else {
                "read-write"
            }
        );
    }
    print!("invocation:");
    for arg in std::iter::once(command.get_program()).chain(command.get_args()) {
        print!(" {:?}", arg);
    }
    println!();
}
