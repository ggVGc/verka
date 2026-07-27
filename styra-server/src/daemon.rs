//! The server bootstrap: bind the socket, set up the store, and run the serve
//! loop. Factored out of the `styra-server` binary so any host executable that
//! links this crate can *be* the server — the `styra` client re-execs itself
//! with the [`SERVE_ENV`] sentinel to spawn its own detached daemon rather than
//! shelling out to a separate `styra-server` binary (see [`crate::spawn`]).

use crate::server::{serve, ServerState};
use anyhow::{bail, Context, Result};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// Sentinel env var: when set, a host binary runs as the Styra server instead
/// of its normal entry point, reading the rest of its configuration from the
/// `STYRA_SERVE_*` variables below.
pub const SERVE_ENV: &str = "STYRA_SERVE";
/// Socket path for the serve-from-env path ([`serve_if_requested`]).
pub const SERVE_SOCKET_ENV: &str = "STYRA_SERVE_SOCKET";
/// Store directory for the serve-from-env path (optional; defaults to the
/// private per-user store).
pub const SERVE_STORE_ENV: &str = "STYRA_SERVE_STORE";

/// What a server needs to come up. `None` fields fall back to the private
/// per-user defaults from [`crate::paths`]. The server runs until it is asked
/// to stop ([`crate::api::Request::Shutdown`]) or killed.
#[derive(Default)]
pub struct ServerConfig {
    /// Store containing durable sessions.
    pub store: Option<PathBuf>,
    /// Unix socket to listen on.
    pub socket: Option<PathBuf>,
}

/// If [`SERVE_ENV`] is set, run as the Styra server (configured from the
/// `STYRA_SERVE_*` env vars) and return `Some(result)`; otherwise return `None`
/// so the caller proceeds with its normal entry point.
///
/// Both the `styra-server` binary and the `styra` client call this first, so a
/// process re-exec'd by [`crate::spawn`] becomes a server regardless of which
/// executable it started life as.
pub fn serve_if_requested() -> Option<Result<()>> {
    std::env::var_os(SERVE_ENV)?;
    Some(run(config_from_serve_env()))
}

/// Build a [`ServerConfig`] from the `STYRA_SERVE_*` environment.
fn config_from_serve_env() -> ServerConfig {
    ServerConfig {
        store: std::env::var_os(SERVE_STORE_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        socket: std::env::var_os(SERVE_SOCKET_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    }
}

/// Bring up a server on `config.socket` and serve until it is asked to shut
/// down or killed.
pub fn run(config: ServerConfig) -> Result<()> {
    let (store, private_store) = match config.store {
        Some(path) => (path, false),
        None => (crate::paths::default_store()?, true),
    };
    let (socket, private_socket_directory) = match config.socket {
        Some(path) => (path, false),
        None => (crate::paths::default_socket()?, true),
    };
    if private_store {
        ensure_private_directory(&store)?;
    }
    let listener = bind_socket(&socket, private_socket_directory)?;
    let _socket_guard = SocketGuard(socket.clone());
    println!(
        "styra-server listening on {} (store {})",
        socket.display(),
        store.display()
    );
    let state = ServerState::new(store, socket);
    serve(listener, state)
}

fn bind_socket(path: &Path, private_parent: bool) -> Result<UnixListener> {
    if let Some(parent) = path.parent() {
        if private_parent {
            ensure_private_directory(parent)?;
        } else {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating socket directory {}", parent.display()))?;
        }
    }
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("a Styra server is already listening on {}", path.display());
        }
        std::fs::remove_file(path)
            .with_context(|| format!("removing stale socket {}", path.display()))?;
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding socket {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting socket permissions {}", path.display()))?;
    Ok(listener)
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("creating private directory {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting directory permissions {}", path.display()))
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_directory_and_socket_are_private() {
        let root =
            std::env::temp_dir().join(format!("styra-server-permissions-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let socket = root.join("styra/styra.sock");
        let listener = bind_socket(&socket, true).unwrap();

        let directory_mode = std::fs::metadata(socket.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let socket_mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(socket_mode, 0o600);

        drop(listener);
        std::fs::remove_dir_all(root).ok();
    }
}
