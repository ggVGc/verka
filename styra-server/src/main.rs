use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use styra_server::ServerConfig;

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
    // A process re-exec'd by the connect-or-spawn path carries the serve
    // sentinel in its environment; honour it before touching the CLI so the
    // same binary can act as either a hand-launched server or a self-spawned
    // daemon.
    if let Some(result) = styra_server::serve_if_requested() {
        return result;
    }
    let cli = Cli::parse();
    styra_server::run(ServerConfig {
        store: cli.store,
        socket: cli.socket,
    })
}
