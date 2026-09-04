use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use driva::{
    execute, validate_request, BwrapIsolation, Config, ExecutionIo, ExecutionRequest, Mount,
    MountAccess, WritableMountMode,
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
        /// Override the template command or supply the executable.
        #[arg(long = "command", value_name = "COMMAND")]
        command_override: Option<OsString>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Open /bin/sh in a disposable isolated environment.
    Shell {
        #[command(flatten)]
        policy: PolicyArgs,
    },
    /// List built-in and project-defined execution templates.
    Templates,
    /// List the base capabilities a private root can be built from, and which
    /// of them this configuration includes.
    Capabilities,
    /// Report whether each included capability works on this host, by building
    /// a sandbox from it and probing it.
    Doctor {
        #[command(flatten)]
        policy: PolicyArgs,
    },
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
    /// Apply a named execution template; may be repeated.
    #[arg(long, value_name = "NAME")]
    template: Vec<String>,
    /// Add a read-only mount as SOURCE or SOURCE:DESTINATION.
    #[arg(long = "read", value_name = "MOUNT")]
    reads: Vec<String>,
    /// Add a writable mount as SOURCE or SOURCE:DESTINATION.
    #[arg(long = "write", value_name = "MOUNT")]
    writes: Vec<String>,
    /// Add a mount as SOURCE or SOURCE:DESTINATION that reads the host
    /// content and is writable, but writes are discarded after execution.
    #[arg(long = "overlay", value_name = "MOUNT")]
    overlays: Vec<String>,
    /// Make every host mount read-only, overriding configuration and templates.
    #[arg(long)]
    no_write: bool,
    /// Turn writable host mounts into overlays: the sandbox reads the host
    /// content and can write to it, but writes are discarded and never reach
    /// the host.
    #[arg(long)]
    overlay_writes: bool,
    /// Add a host directory read-only and prepend it to the isolated PATH.
    #[arg(long = "path", value_name = "DIRECTORY")]
    paths: Vec<PathBuf>,
    /// Add a base capability to the private root; may be repeated
    /// (see `driva capabilities`).
    #[arg(long = "capability", value_name = "NAME")]
    capabilities: Vec<String>,
    /// Leave a base capability out, overriding configuration and templates.
    #[arg(long = "no-capability", value_name = "NAME")]
    no_capabilities: Vec<String>,
    /// Build the private root with no base at all: an empty filesystem holding
    /// only what is mounted into it.
    #[arg(long)]
    no_base: bool,
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
    /// Start a new terminal session, overriding a template
    /// (Bubblewrap's --new-session, which blocks TIOCSTI input injection).
    #[arg(long, conflicts_with = "no_new_session")]
    new_session: bool,
    /// Keep the caller's terminal session instead of starting a new one
    /// (omits Bubblewrap's --new-session, which otherwise blocks TIOCSTI).
    #[arg(long, conflicts_with = "new_session")]
    no_new_session: bool,
    /// Print the validated request and backend invocation without executing it.
    #[arg(long)]
    dry_run: bool,
    /// Override the Bubblewrap root filesystem.
    #[arg(long, value_name = "DIRECTORY")]
    rootfs: Option<PathBuf>,
    /// Add an empty writable filesystem discarded after execution.
    #[arg(long, value_name = "DIRECTORY")]
    temporary: Vec<PathBuf>,
    /// Override the isolated working directory (defaults to a writable current-dir workspace).
    #[arg(long)]
    workdir: Option<PathBuf>,
    /// Inherit environment variables from the host shell.
    #[arg(long)]
    inherit_env: bool,
    /// Set an environment variable as NAME=VALUE.
    #[arg(long = "env", value_parser = parse_environment)]
    environment: Vec<(OsString, OsString)>,
}

#[derive(Debug)]
enum ResolvedBackend {
    Bwrap { rootfs: Option<PathBuf> },
}

impl ResolvedBackend {
    fn name(&self) -> &'static str {
        match self {
            Self::Bwrap { .. } => "bwrap",
        }
    }
}

fn main() {
    // Driva answers its own network probes from inside a sandbox, so the
    // sentinel is handled before the public command line exists.
    if let Some(result) = driva::probe::exit_if_requested() {
        if let Err(error) = result {
            eprintln!("driva: {error:#}");
            std::process::exit(1);
        }
        return;
    }
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
    let (policy, command_override, mut command, shell) = match cli.command {
        Operation::Run {
            policy,
            command_override,
            command,
        } => (policy, command_override, command, false),
        Operation::Shell { mut policy } => {
            policy.interactive = true;
            (policy, None, vec![OsString::from("/bin/sh")], true)
        }
        Operation::Templates => {
            for (name, template) in config.effective_templates() {
                println!("{name}\t{}", template.description);
            }
            return Ok(());
        }
        Operation::Capabilities => return capabilities_command(&config),
        Operation::Doctor { policy } => return doctor_command(&config, &policy),
        Operation::Runtime { command } => return runtime_command(command),
    };
    let mut template: Option<driva::TemplateConfig> = None;
    for name in &policy.template {
        let later = config.template(name).with_context(|| {
            format!("unknown template {name:?}; run `driva templates` to list available templates")
        })?;
        validate_workspace_mounts(&later)?;
        match &mut template {
            Some(template) => template.overlay(later),
            None => template = Some(later),
        }
    }
    let workspace_mount = template
        .as_mut()
        .map(resolve_workspace_mount)
        .transpose()?
        .flatten();
    if !shell {
        if let Some(command_override) = command_override {
            command.insert(0, command_override);
        } else if let Some(template) = &template {
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
    let base = effective_base(&config, template.as_ref(), &policy);
    let configured_workdir = match backend_name {
        "bwrap" => &config.isolation.bwrap.workdir,
        backend => bail!("unsupported isolation backend {backend:?}"),
    };
    let configured_workdir = policy
        .workdir
        .clone()
        .or_else(|| template.as_ref().and_then(|value| value.workdir.clone()))
        .or_else(|| configured_workdir.clone());
    let default_workspace = if configured_workdir.is_none() {
        Some(resolve_default_workspace()?)
    } else {
        None
    };
    let workdir = configured_workdir.unwrap_or_else(|| {
        default_workspace
            .as_ref()
            .expect("default workspace exists when workdir is omitted")
            .destination()
            .to_path_buf()
    });
    let mut mounts: Vec<Mount> = config
        .mounts
        .into_iter()
        .map(driva::MountConfig::resolve)
        .collect::<Result<_>>()?;
    if let Some(template) = &template {
        let template_mounts = template
            .mounts
            .iter()
            .cloned()
            .map(driva::MountConfig::resolve)
            .collect::<Result<Vec<_>>>()?;
        mounts.extend(template_mounts);
    }
    if let Some(workspace_mount) = workspace_mount {
        mounts.push(workspace_mount);
    }
    for spec in &policy.reads {
        mounts.push(parse_mount(spec, MountAccess::ReadOnly, &workdir)?);
    }
    for spec in &policy.writes {
        mounts.push(parse_mount(spec, MountAccess::ReadWrite, &workdir)?);
    }
    for spec in &policy.overlays {
        mounts.push(parse_overlay_mount(spec, &workdir)?);
    }
    mounts.extend(
        policy
            .temporary
            .iter()
            .cloned()
            .map(|destination| Mount::Temporary { destination }),
    );
    let mut environment: BTreeMap<OsString, OsString> = if policy.inherit_env {
        std::env::vars_os().collect()
    } else {
        BTreeMap::new()
    };
    environment.extend(config.environment);
    if let Some(template) = &template {
        let mut template_environment: BTreeMap<OsString, OsString> = template
            .environment
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect();
        driva::expand_environment_home(&mut template_environment)?;
        environment.extend(template_environment);
        if let Some(home) = std::env::var_os("HOME") {
            environment.entry(OsString::from("HOME")).or_insert(home);
        }
    }
    if backend_name == "bwrap" {
        if let Some(term) = std::env::var_os("TERM") {
            environment.entry(OsString::from("TERM")).or_insert(term);
        }
    }
    environment.extend(policy.environment.iter().cloned());
    let mut paths = template
        .as_ref()
        .map(|value| value.paths.clone())
        .unwrap_or_default();
    paths.extend(policy.paths.iter().cloned());
    driva::path_mounts(&paths, &mut mounts, &mut environment)?;
    if let Some(default_workspace) = default_workspace {
        if !mounts.iter().any(|mount| mount.destination() == workdir) {
            mounts.push(default_workspace);
        }
    }
    if policy.no_write {
        for mount in &mut mounts {
            mount.make_read_only();
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
        writable_mounts: if policy.overlay_writes {
            WritableMountMode::Overlay
        } else {
            WritableMountMode::Direct
        },
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
        new_session: if policy.no_new_session {
            false
        } else if policy.new_session {
            true
        } else {
            template
                .as_ref()
                .and_then(|value| value.new_session)
                // An interactive shell must remain in the terminal's foreground
                // session so terminal-generated signals, notably Ctrl-C, reach
                // the command it is currently running instead of terminating
                // Driva itself. Other commands retain the safer Bubblewrap
                // default of a new session unless explicitly overridden.
                .unwrap_or(!shell)
        },
    };
    let request = validate_request(&request)?;
    match backend {
        ResolvedBackend::Bwrap { rootfs } => {
            let backend = BwrapIsolation {
                executable: config.isolation.bwrap.executable,
                rootfs,
                base,
            };
            let invocation = backend.command(&request).with_context(|| {
                if policy.template.iter().any(|name| name == "codex-runtime") {
                    "Codex runtime is unavailable; run `driva runtime install codex@VERSION`"
                } else {
                    "failed to construct Bubblewrap invocation"
                }
            })?;
            finish("bwrap", &backend, invocation, &request, policy.dry_run)
        }
    }
}

/// The base system one invocation runs on.
///
/// Configuration states the list, a template adds what its command requires,
/// and the command line has the last word — the same precedence the mount and
/// network policy already follow. A template may ask for a capability but
/// never defines one: what a capability *means* on this host is the
/// configuration's business, so a template cannot quietly widen the root.
fn effective_base(
    config: &Config,
    template: Option<&driva::TemplateConfig>,
    policy: &PolicyArgs,
) -> driva::BaseConfig {
    let mut base = config.base();
    if policy.no_base {
        base.include.clear();
        return base;
    }
    for name in template.iter().flat_map(|value| value.capabilities.iter()) {
        base.include(name);
    }
    for name in &policy.capabilities {
        base.include(name);
    }
    for name in &policy.no_capabilities {
        base.exclude(name);
    }
    base
}

/// List the capabilities available here, marking the ones the effective base
/// includes and in what order they are laid down.
fn capabilities_command(config: &Config) -> Result<()> {
    let base = config.base();
    for (name, capability) in &base.definitions {
        let position = base.include.iter().position(|included| included == name);
        let marker = match position {
            Some(index) => format!("{}", index + 1),
            None => "-".to_owned(),
        };
        println!("{marker}\t{name}\t{}", capability.description);
    }
    Ok(())
}

/// Report whether each included capability works on this host.
///
/// A capability is a claim about where this machine keeps something, and the
/// only honest test of a claim is to build a sandbox from it and use it. The
/// exit status is non-zero when a probe fails, so this is usable as a check.
fn doctor_command(config: &Config, policy: &PolicyArgs) -> Result<()> {
    let declared = effective_base(config, None, policy);
    let base = driva::resolve_base(&declared)?;
    let executable = &config.isolation.bwrap.executable;
    let mut failed = false;
    for capability in &base.capabilities {
        let outcome = driva::probe::probe_capability(executable, &declared, capability)?;
        println!(
            "{:<14} {:<9} {} path(s){}",
            capability.name,
            outcome.label(),
            capability.entries.len(),
            match capability.environment.len() {
                0 => String::new(),
                count => format!(", {count} forwarded variable(s)"),
            }
        );
        for entry in &capability.entries {
            match entry {
                driva::RuntimeEntry::ReadOnly { source, path } if source != path => {
                    println!("               {} → {}", path.display(), source.display())
                }
                entry => println!("               {}", entry.path().display()),
            }
        }
        match outcome {
            driva::probe::ProbeOutcome::Failed(reason) => {
                failed = true;
                println!("               probe: {reason}");
                let suggestions = driva::probe::suggestions(&base, capability);
                if !suggestions.is_empty() {
                    println!(
                        "               this host also has {}, which no capability carries.",
                        suggestions
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    println!(
                        "               add it with:\n\
                         \x20                [capability.{}]\n\
                         \x20                path = [{}]",
                        capability.name,
                        suggestions
                            .iter()
                            .map(|path| format!("{{ at = \"{}\" }}", path.display()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            driva::probe::ProbeOutcome::Unprobed(reason) => {
                println!("               probe: not made ({reason})")
            }
            driva::probe::ProbeOutcome::Passed | driva::probe::ProbeOutcome::None => {}
        }
    }
    if failed {
        bail!("a capability of the sandbox base does not work on this host");
    }
    Ok(())
}

fn resolve_backend(
    name: &str,
    policy: &PolicyArgs,
    template: Option<&driva::TemplateConfig>,
    config: &Config,
) -> Result<ResolvedBackend> {
    let template_rootfs = template.and_then(|value| value.rootfs.clone());
    match name {
        "bwrap" => Ok(ResolvedBackend::Bwrap {
            rootfs: policy
                .rootfs
                .clone()
                .or(template_rootfs)
                .or_else(|| config.isolation.bwrap.rootfs.clone()),
        }),
        backend => bail!("unsupported isolation backend {backend:?}"),
    }
}

/// Resolve a template's workspace mount and use its destination as the
/// isolated working directory.
fn resolve_workspace_mount(template: &mut driva::TemplateConfig) -> Result<Option<Mount>> {
    let Some(workspace_mount) = template.workspace_mounts.pop() else {
        return Ok(None);
    };
    let workspace_mount = workspace_mount.resolve()?;
    let Mount::Bind { destination, .. } = &workspace_mount else {
        bail!("workspace-mount must be a bind mount");
    };
    template.workdir = Some(destination.clone());
    Ok(Some(workspace_mount))
}

fn validate_workspace_mounts(template: &driva::TemplateConfig) -> Result<()> {
    if template.workspace_mounts.len() > 1 {
        bail!("a template may contain at most one workspace-mount");
    }
    Ok(())
}

/// Use the canonical current directory as a writable same-path workspace when
/// no working directory was selected by the CLI, template, or backend config.
fn resolve_default_workspace() -> Result<Mount> {
    driva::MountConfig {
        kind: driva::MountKind::Bind,
        source: Some(PathBuf::from(".")),
        destination: None,
        access: Some(MountAccess::ReadWrite),
    }
    .resolve()
    .context("failed to resolve the current directory as the default workspace")
}

fn runtime_command(command: RuntimeOperation) -> Result<()> {
    let store = driva::RuntimeStore::new(driva::RuntimeStore::default_path()?);
    match command {
        RuntimeOperation::Install { runtime, image } => {
            let spec = driva::RuntimeSpec::parse(&runtime)?;
            println!("Preparing {} from {image}...", spec.display());
            let resolved = store.install_codex(&spec, &image, Path::new("podman"))?;
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
    let (source, destination) = parse_mount_spec(spec, workdir)?;
    Ok(Mount::Bind {
        source,
        destination,
        access,
    })
}

fn parse_overlay_mount(spec: &str, workdir: &Path) -> Result<Mount> {
    let (source, destination) = parse_mount_spec(spec, workdir)?;
    Ok(Mount::Overlay {
        source,
        destination,
    })
}

fn parse_mount_spec(spec: &str, workdir: &Path) -> Result<(PathBuf, PathBuf)> {
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
    Ok((source, destination))
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
        match mount {
            Mount::Bind {
                source,
                destination,
                access,
            } => println!(
                "mount: {} -> {} ({})",
                source.display(),
                destination.display(),
                if *access == MountAccess::ReadOnly {
                    "read-only"
                } else if request.writable_mounts == WritableMountMode::Overlay {
                    "overlay, writes discarded"
                } else {
                    "read-write"
                }
            ),
            Mount::Temporary { destination } => {
                println!("mount: temporary -> {} (read-write)", destination.display())
            }
            Mount::Overlay {
                source,
                destination,
            } => println!(
                "mount: {} -> {} (overlay, writes discarded)",
                source.display(),
                destination.display()
            ),
        }
    }
    print!("invocation:");
    for arg in std::iter::once(command.get_program()).chain(command.get_args()) {
        print!(" {:?}", arg);
    }
    println!();
}
