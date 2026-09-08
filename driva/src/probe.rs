//! Checking a capability by using it, in a sandbox built from it alone.
//!
//! A path list is a guess about a host; a probe is an answer. The capability
//! that names `/run/systemd/resolve` is right on one machine and wrong on the
//! next, and the difference is invisible until something inside the sandbox
//! fails at it — as a network error in an agent, hours from the configuration
//! that caused it. A probe moves that discovery to a command an operator can
//! run on purpose.
//!
//! The kinds are a closed set. Configuration selects a check; it never
//! supplies host code to execute.
//!
//! `resolve` and `connect` run inside the isolation as Driva itself, through
//! the [`PROBE_ENV`] sentinel, so they test the sandbox's own resolver and
//! routing rather than the host's, and need nothing installed to do it.

use crate::base::{BaseConfig, Probe, ResolvedCapability};
use crate::{
    execute, BwrapIsolation, ExecutionIo, ExecutionRequest, Mount, MountAccess, WritableMountMode,
};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

/// Set on the isolated process to ask the Driva binary to answer one probe
/// and exit, rather than parse its public command line.
pub const PROBE_ENV: &str = "DRIVA_PROBE";

/// How long a `connect` probe waits before reporting the host unreachable.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Run the probe when the sentinel is present. Called before the CLI parses,
/// the way the isolated side of any in-process helper has to be.
pub fn exit_if_requested() -> Option<Result<()>> {
    let request = std::env::var(PROBE_ENV).ok()?;
    match run_in_isolation(&request) {
        Ok(()) => std::process::exit(0),
        // The caller reports it and exits non-zero, which is what the probing
        // side reads as "this capability does not work here".
        Err(error) => Some(Err(error)),
    }
}

fn run_in_isolation(request: &str) -> Result<()> {
    match request.split_once(':') {
        Some(("resolve", name)) => {
            let addresses = (name, 0u16)
                .to_socket_addrs()
                .with_context(|| format!("resolving {name}"))?
                .count();
            if addresses == 0 {
                bail!("resolving {name}: no addresses");
            }
            Ok(())
        }
        Some(("connect", target)) => {
            let address = target
                .to_socket_addrs()
                .with_context(|| format!("resolving {target}"))?
                .next()
                .with_context(|| format!("resolving {target}: no addresses"))?;
            TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
                .with_context(|| format!("connecting to {target}"))?;
            Ok(())
        }
        _ => bail!("unknown probe request {request:?}"),
    }
}

/// What one probe said about the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The capability works here.
    Passed,
    /// It does not, with what the sandbox reported.
    Failed(String),
    /// The check could not be made — a `run` probe naming a command this
    /// sandbox does not have. Distinct from failure on purpose: an unprobed
    /// capability is unknown, not broken.
    Unprobed(String),
    /// The capability declares no probe.
    None,
}

impl ProbeOutcome {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Passed => "ok",
            Self::Failed(_) => "FAILED",
            Self::Unprobed(_) => "unprobed",
            Self::None => "no probe",
        }
    }
}

/// Probe one capability in a sandbox holding `core` and that capability only,
/// so a pass or a failure is about this capability and not about what some
/// other one happened to bring along.
pub fn probe_capability(
    executable: &std::path::Path,
    capability: &ResolvedCapability,
) -> Result<ProbeOutcome> {
    let Some(probe) = &capability.probe else {
        return Ok(ProbeOutcome::None);
    };
    let mut include = vec!["core".to_owned()];
    if capability.name != "core" {
        include.push(capability.name.clone());
    }
    let backend = BwrapIsolation {
        executable: executable.to_path_buf(),
        rootfs: None,
        base: BaseConfig { include },
    };
    let (command, mounts) = probe_command(probe)?;
    let request = ExecutionRequest {
        command,
        working_directory: PathBuf::from("/tmp"),
        mounts,
        writable_mounts: WritableMountMode::Direct,
        environment: probe_environment(probe),
        network: probe.needs_network(),
        interactive: false,
        new_session: true,
    };
    let output = tempfile()?;
    let io = ExecutionIo {
        stdin: File::open("/dev/null")?,
        stdout: output.try_clone()?,
        stderr: output.try_clone()?,
    };
    let outcome = match execute(&backend, &request, io) {
        Ok(outcome) => outcome,
        // A sandbox that will not start at all is a failure of this
        // capability's own composition, and the message is the useful part.
        Err(error) => return Ok(ProbeOutcome::Failed(format!("{error:#}"))),
    };
    let reported = read_back(output);
    if outcome.exit.code() == 0 {
        return Ok(ProbeOutcome::Passed);
    }
    // `sh` reports 127 for a command it cannot find, and Bubblewrap passes it
    // through: the check was never made.
    if matches!(probe, Probe::Run(_)) && outcome.exit.code() == 127 {
        return Ok(ProbeOutcome::Unprobed(reported));
    }
    Ok(ProbeOutcome::Failed(reported))
}

impl Probe {
    /// Whether the check is about the network, and so needs it permitted.
    fn needs_network(&self) -> bool {
        matches!(self, Self::Resolve(_) | Self::Connect(_))
    }
}

/// The command a probe runs, and what has to be mounted to run it.
///
/// `resolve` and `connect` are answered by Driva itself inside the sandbox, so
/// its own binary is bound read-only at its host path — the one mount a probe
/// needs, and the reason a probe can report on a host with no tools installed.
fn probe_command(probe: &Probe) -> Result<(Vec<OsString>, Vec<Mount>)> {
    match probe {
        Probe::Run(command) => Ok((command.iter().map(OsString::from).collect(), Vec::new())),
        Probe::Resolve(_) | Probe::Connect(_) => {
            let executable = std::env::current_exe()
                .context("locating the Driva binary to answer the probe inside the sandbox")?;
            Ok((
                vec![executable.clone().into_os_string()],
                vec![Mount::Bind {
                    source: executable.clone(),
                    destination: executable,
                    access: MountAccess::ReadOnly,
                }],
            ))
        }
    }
}

fn probe_environment(probe: &Probe) -> BTreeMap<OsString, OsString> {
    let request = match probe {
        Probe::Resolve(name) => format!("resolve:{name}"),
        Probe::Connect(target) => format!("connect:{target}"),
        Probe::Run(_) => return BTreeMap::new(),
    };
    BTreeMap::from([(OsString::from(PROBE_ENV), OsString::from(request))])
}

use std::fs::File;

/// A file the probe's output is collected in, removed as soon as it is opened
/// so nothing is left behind however the probe ends.
fn tempfile() -> Result<File> {
    let path = std::env::temp_dir().join(format!("driva-probe-{}", std::process::id()));
    let file = File::options()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("creating the probe output file {}", path.display()))?;
    std::fs::remove_file(&path).ok();
    Ok(file)
}

fn read_back(mut file: File) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut text = String::new();
    file.seek(SeekFrom::Start(0)).ok();
    file.read_to_string(&mut text).ok();
    let text = text.trim();
    if text.is_empty() {
        "no output".to_owned()
    } else {
        text.lines().last().unwrap_or(text).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The isolated side answers the two network checks and rejects anything
    /// else, since the sentinel is Driva talking to itself.
    #[test]
    fn the_isolated_side_answers_only_known_probes() {
        assert!(run_in_isolation("nonsense").is_err());
        assert!(run_in_isolation("resolve:localhost").is_ok());
    }

    /// A `run` probe needs nothing mounted; a network probe carries Driva's
    /// own binary in to answer it.
    #[test]
    fn a_network_probe_brings_the_binary_that_answers_it() {
        let (command, mounts) = probe_command(&Probe::Run(vec!["/bin/true".into()])).unwrap();
        assert_eq!(command, vec![OsString::from("/bin/true")]);
        assert!(mounts.is_empty());

        let (command, mounts) = probe_command(&Probe::Resolve("example.com".into())).unwrap();
        assert_eq!(command.len(), 1);
        assert!(matches!(
            &mounts[..],
            [Mount::Bind { source, access: MountAccess::ReadOnly, .. }] if source == &std::env::current_exe().unwrap()
        ));
        assert_eq!(
            probe_environment(&Probe::Resolve("example.com".into()))
                .get(&OsString::from(PROBE_ENV)),
            Some(&OsString::from("resolve:example.com"))
        );
    }
}
