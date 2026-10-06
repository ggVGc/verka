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
    /// Run the server in this process instead of talking to a daemon: no
    /// socket is used or bound, and no other client can attach. Interactions
    /// end when the interface exits. Its durable state is kept separately from
    /// the daemon's store and is available on the next standalone run.
    #[arg(long, conflicts_with_all = ["daemon", "stop", "socket"])]
    pub standalone: bool,
    /// Host directory mounted writable at its canonical path (default: cwd).
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    /// Permit agent networking (providers may default this on). Adds to the
    /// saved default policy, which is itself layered over the Workspace's
    /// standing one; the driva view (`d`) can still change it before an
    /// interaction starts.
    #[arg(long)]
    pub network: bool,
    /// Apply a Driva execution template to the agent sandbox (see `driva
    /// templates`); may be repeated to layer several, e.g. a `rust` toolchain.
    /// Replaces the saved default list, since the order given is the layering.
    /// Layered after the Workspace's own templates, not instead of them: use
    /// `I` in the driva view to launch without those.
    #[arg(long = "template", value_name = "NAME")]
    pub template: Vec<String>,
    /// Open a captured journal read-only instead of launching an agent: with
    /// a path, that session directly; bare (no path), a picker to browse and
    /// choose one from the server's store.
    #[arg(long, num_args = 0..=1, value_name = "SESSION")]
    pub view: Option<Option<PathBuf>>,
    /// Never stop at the Workspace list: in a directory without a Workspace,
    /// go straight to the interaction view, on the live interaction that
    /// would be landed on if any is running, else in the Workspace accessed
    /// most recently. The live-interaction list is opened whenever any
    /// interaction is live.
    #[arg(long, conflicts_with_all = ["view", "workspace"])]
    pub skip_workspace_list: bool,
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
        /// Live Styra session to attach to; omit to browse live sessions.
        #[arg(long)]
        session: Option<String>,
    },
    /// List the Git worktrees Styra knows of, and the sessions working in
    /// each. Includes branches whose checkout was cleaned up, and directories
    /// in the worktree parent that no session records. Changes nothing.
    Worktrees {
        /// List every Workspace, not only the one covering the current
        /// directory.
        #[arg(long)]
        all: bool,
    },
    /// Delete the Git worktrees of completed sessions whose checkouts have
    /// nothing uncommitted in them, keeping their branches. Each such session
    /// is left recording its branch alone, and resuming it checks that branch
    /// out again. A checkout with uncommitted work, or one a live interaction
    /// is still using, is reported and left alone.
    CleanWorktrees {
        /// Clean every Workspace, not only the one covering the current
        /// directory.
        #[arg(long)]
        all: bool,
    },
}
