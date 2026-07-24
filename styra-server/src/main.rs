use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;
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
    /// Exit after this many seconds with no live jobs and no client activity
    /// (0 keeps the server running until it is killed).
    #[arg(long, default_value_t = styra_server::daemon::DEFAULT_IDLE_TIMEOUT_SECS)]
    idle_timeout: u64,
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
        idle_timeout: Duration::from_secs(cli.idle_timeout),
    })
}
