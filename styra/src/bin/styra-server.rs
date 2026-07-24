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
    /// Store containing durable Styra sessions (default: $XDG_CONFIG_HOME/styra).
    #[arg(long)]
    store: Option<PathBuf>,
    /// Unix socket path (default: <store>/styra.sock).
    #[arg(long)]
    socket: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = match cli.store {
        Some(path) => path,
        None => styra::paths::default_store()?,
    };
    let socket = cli.socket.unwrap_or_else(|| store.join("styra.sock"));
    let listener = bind_socket(&socket)?;
    let _socket_guard = SocketGuard(socket.clone());
    println!(
        "styra-server listening on {} (store {})",
        socket.display(),
        store.display()
    );
    serve(listener, ServerState::new(store))
}

fn bind_socket(path: &Path) -> Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating socket directory {}", parent.display()))?;
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

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}
