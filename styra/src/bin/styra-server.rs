use anyhow::{bail, Context, Result};
use clap::Parser;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use styra::server::{serve, ServerState};

#[derive(Parser)]
#[command(
    name = "styra-server",
    about = "Run the Styra local JSON server",
    version
)]
struct Cli {
    /// Store containing durable sessions (default: $XDG_STATE_HOME/styra).
    #[arg(long)]
    store: Option<PathBuf>,
    /// Unix socket path (default: $XDG_RUNTIME_DIR/styra/styra.sock).
    #[arg(long)]
    socket: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (store, private_store) = match cli.store {
        Some(path) => (path, false),
        None => (styra::paths::default_store()?, true),
    };
    let (socket, private_socket_directory) = match cli.socket {
        Some(path) => (path, false),
        None => (styra::paths::default_socket()?, true),
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
    serve(listener, ServerState::new(store))
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
        let root = std::env::temp_dir().join(format!(
            "styra-server-permissions-{}",
            std::process::id()
        ));
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
