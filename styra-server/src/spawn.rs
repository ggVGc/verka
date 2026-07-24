//! Connect-or-spawn: guarantee a Styra server is answering on the socket,
//! starting one as a detached background daemon if it is not.
//!
//! This mirrors how `sbt`/`bloop` keep a warm build server alive across client
//! invocations: a cheap client tries the well-known socket, and only when
//! nothing answers does it launch the heavy long-lived server and detach from
//! it. For Styra the payoff is not a warm JVM but live jobs — an agent turn
//! keeps running while the TUI detaches, and is still there when it reattaches.

use crate::client::Client;
use anyhow::{bail, Context, Result};
use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for a freshly spawned server to start answering before
/// giving up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Return a [`Client`] for `socket`, guaranteeing a server answers on it.
///
/// If a server already responds to a health check, it is used as-is. Otherwise
/// `styra-server` is spawned as a detached daemon bound to `socket`, and this
/// blocks until it comes up (or [`STARTUP_TIMEOUT`] elapses).
pub fn ensure_server(socket: impl Into<PathBuf>) -> Result<Client> {
    let socket = socket.into();
    let client = Client::new(&socket);
    // Fast path: a server is already listening. A stale socket (server gone)
    // fails this check and is cleaned up by the server we spawn below when it
    // binds.
    if client.health().is_ok() {
        return Ok(client);
    }
    spawn_detached(&socket).context("starting the Styra server")?;
    wait_until_healthy(&client, STARTUP_TIMEOUT)?;
    Ok(client)
}

/// Launch `styra-server --socket <socket>` fully detached from this process's
/// terminal so it survives the client exiting and ignores terminal signals.
fn spawn_detached(socket: &Path) -> Result<()> {
    let exe = server_binary();
    let log = server_log_path(socket);
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating server log directory {}", parent.display()))?;
    }
    let out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("opening server log {}", log.display()))?;
    let err = out.try_clone().context("cloning the server log handle")?;

    let mut command = Command::new(&exe);
    command
        .arg("--socket")
        .arg(socket)
        // Nothing on the terminal: stdin is closed and all output goes to the
        // log, so the daemon can never read from or scribble over the TUI.
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    // `setsid` puts the daemon in its own session with no controlling
    // terminal, so a terminal hangup (SIGHUP) or Ctrl-C (SIGINT) aimed at the
    // client's process group never reaches it. Combined with not waiting on
    // the child, it is reparented to init and lives on after the client exits.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("spawning {}", exe.display()))?;
    // Deliberately no `wait`: the daemon outlives us.
    Ok(())
}

/// Environment override for the server binary, for unusual install layouts
/// and tests.
const SERVER_BIN_ENV: &str = "STYRA_SERVER_BIN";

/// Locate the `styra-server` binary. An explicit [`SERVER_BIN_ENV`] wins;
/// otherwise prefer one sitting next to the current executable so a dev build
/// (`target/debug`) or an installed bin directory keeps the client and server
/// paired; otherwise fall back to `PATH`.
fn server_binary() -> PathBuf {
    if let Some(path) = std::env::var_os(SERVER_BIN_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(sibling) = exe.parent().map(|dir| dir.join("styra-server")) {
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("styra-server")
}

/// The daemon's log, kept beside its socket.
fn server_log_path(socket: &Path) -> PathBuf {
    socket
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("styra-server.log")
}

/// Poll the health endpoint until the server answers or `timeout` elapses,
/// backing off from a tight initial interval so a fast startup returns quickly.
fn wait_until_healthy(client: &Client, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_millis(20);
    loop {
        if client.health().is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "the Styra server did not start within {}s; see {}",
                timeout.as_secs(),
                server_log_path(client.socket_path()).display()
            );
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_millis(200));
    }
}
