use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Run an interactive, isolated agent session in a terminal interface.
#[derive(Parser)]
#[command(name = "styra", about, version)]
pub struct Cli {
    /// Styra server Unix socket (default: $XDG_RUNTIME_DIR/styra/styra.sock).
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,
    /// Start the Styra daemon in the background and exit, without opening the
    /// interface. A no-op if one is already listening on the socket.
    #[arg(short = 'd', long = "daemon", conflicts_with = "stop")]
    pub daemon: bool,
    /// Stop the Styra daemon listening on the socket (if any) and exit. Any
    /// live interactions it owns are ended with it.
    #[arg(long)]
    pub stop: bool,
    /// Host directory mounted writable as the agent workspace (default: cwd).
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    /// Permit agent networking (providers may default this on).
    #[arg(long)]
    pub network: bool,
    /// Apply a Driva execution template to the agent sandbox (see `driva
    /// templates`); may be repeated to layer several, e.g. a `rust` toolchain.
    #[arg(long = "template", value_name = "NAME")]
    pub template: Vec<String>,
    /// Open a captured journal read-only instead of launching an agent: with
    /// a path, that session directly; bare (no path), a picker to browse and
    /// choose one from the server's store.
    #[arg(long, num_args = 0..=1, value_name = "SESSION")]
    pub view: Option<Option<PathBuf>>,
    #[command(subcommand)]
    pub command: Option<CliCommand>,
    /// Optional first message, sent to seed the opening turn.
    #[arg(trailing_var_arg = true)]
    pub prompt: Vec<String>,
}

#[derive(Subcommand)]
pub enum CliCommand {
    /// Attach to the persistent shell inside a live session's sandbox.
    Shell {
        /// Live Styra session to attach to.
        #[arg(long)]
        session: String,
    },
}
