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
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
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
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
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
    /// Print the validated request and backend invocation without executing it.
    #[arg(long)]
    dry_run: bool,
    /// Override the configured container image.
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
    let operation = cli.command;
    if !matches!(
        operation,
        Operation::Run { .. } | Operation::Shell { .. } | Operation::Start { .. }
    ) {
        return lifecycle(&config, operation);
    }
    let (policy, command, shell, durable) = match operation {
        Operation::Run { policy, command } => (policy, command, false, false),
        Operation::Shell { mut policy } => {
            policy.interactive = true;
            (policy, vec![OsString::from("/bin/sh")], true, false)
        }
        Operation::Start { policy, command } => (policy, command, false, true),
        _ => unreachable!(),
    };
    let configured_workdir = match config.isolation.backend.as_str() {
        "podman" => &config.isolation.podman.workdir,
        "docker" => &config.isolation.docker.workdir,
        "bwrap" => &config.isolation.bwrap.workdir,
        backend => bail!("unsupported isolation backend {backend:?}"),
    };
    let workdir = policy
        .workdir
        .clone()
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
    match config.isolation.backend.as_str() {
        "podman" => {
            let backend = PodmanIsolation {
                executable: config.isolation.podman.executable,
                image: policy.image.unwrap_or(config.isolation.podman.image),
            };
            let invocation = backend.command(&request);
            if durable {
                start_session(&backend, request, policy.dry_run, invocation)
            } else {
                finish("podman", &backend, invocation, &request, policy.dry_run)
            }
        }
        "docker" => {
            let backend = DockerIsolation {
                executable: config.isolation.docker.executable,
                image: policy.image.unwrap_or(config.isolation.docker.image),
            };
            let invocation = backend.command(&request);
            if durable {
                start_session(&backend, request, policy.dry_run, invocation)
            } else {
                finish("docker", &backend, invocation, &request, policy.dry_run)
            }
        }
        "bwrap" => {
            if durable {
                bail!("Bubblewrap does not support durable sessions; use `driva run` or `driva shell`");
            }
            if policy.image.is_some() {
                bail!("--image is not supported by the Bubblewrap backend; configure isolation.bwrap.rootfs");
            }
            let backend = BwrapIsolation {
                executable: config.isolation.bwrap.executable,
                rootfs: config
                    .isolation
                    .bwrap
                    .rootfs
                    .context("Bubblewrap requires isolation.bwrap.rootfs")?,
            };
            let invocation = backend.command(&request)?;
            finish("bwrap", &backend, invocation, &request, policy.dry_run)
        }
        _ => unreachable!("backend was validated above"),
    }
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
