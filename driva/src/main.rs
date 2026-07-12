use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use driva::{
    execute, validate_request, Config, DockerIsolation, ExecutionIo, ExecutionRequest, Mount,
    MountAccess,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

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
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Open /bin/sh in a disposable isolated environment.
    Shell {
        #[command(flatten)]
        policy: PolicyArgs,
    },
}

#[derive(Args, Default)]
struct PolicyArgs {
    /// Add a read-only mount as SOURCE or SOURCE:DESTINATION.
    #[arg(long = "read", value_name = "MOUNT")]
    reads: Vec<String>,
    /// Add a writable mount as SOURCE or SOURCE:DESTINATION.
    #[arg(long = "write", value_name = "MOUNT")]
    writes: Vec<String>,
    /// Permit networking (disabled otherwise).
    #[arg(long)]
    network: bool,
    /// Allocate an interactive terminal.
    #[arg(short, long)]
    interactive: bool,
    /// Print the validated request and Docker invocation without executing it.
    #[arg(long)]
    dry_run: bool,
    /// Override the configured Docker image.
    #[arg(long)]
    image: Option<String>,
    /// Override the isolated working directory.
    #[arg(long)]
    workdir: Option<PathBuf>,
    /// Set an environment variable as NAME=VALUE.
    #[arg(long = "env", value_parser = parse_environment)]
    environment: Vec<(OsString, OsString)>,
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
    if config.isolation.backend != "docker" {
        bail!(
            "unsupported isolation backend {:?}",
            config.isolation.backend
        );
    }

    let (policy, command, shell) = match cli.command {
        Operation::Run { policy, command } => (policy, command, false),
        Operation::Shell { mut policy } => {
            policy.interactive = true;
            (policy, vec![OsString::from("/bin/sh")], true)
        }
    };
    let workdir = policy
        .workdir
        .clone()
        .unwrap_or_else(|| config.isolation.docker.workdir.clone());
    let mut mounts: Vec<Mount> = config
        .mounts
        .into_iter()
        .map(|mount| Mount {
            source: mount.source,
            destination: mount.destination,
            access: mount.access,
        })
        .collect();
    for spec in &policy.reads {
        mounts.push(parse_mount(spec, MountAccess::ReadOnly, &workdir)?);
    }
    for spec in &policy.writes {
        mounts.push(parse_mount(spec, MountAccess::ReadWrite, &workdir)?);
    }
    let mut environment: BTreeMap<OsString, OsString> = config.environment;
    environment.extend(policy.environment);
    let request = ExecutionRequest {
        command,
        working_directory: workdir,
        mounts,
        environment,
        network: policy.network || config.network.enabled,
        interactive: policy.interactive || shell,
    };
    let request = validate_request(&request)?;
    let backend = DockerIsolation {
        executable: config.isolation.docker.executable,
        image: policy.image.unwrap_or(config.isolation.docker.image),
    };
    if policy.dry_run {
        print_dry_run(&backend, &request);
        return Ok(());
    }
    let outcome = execute(&backend, &request, ExecutionIo::inherited()?)?;
    std::process::exit(outcome.exit.code());
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

fn print_dry_run(backend: &DockerIsolation, request: &ExecutionRequest) {
    println!("backend: docker");
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
    let command = backend.command(request);
    print!("invocation:");
    for arg in std::iter::once(command.get_program()).chain(command.get_args()) {
        print!(" {:?}", arg);
    }
    println!();
}
