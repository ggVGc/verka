use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// Run an interactive, isolated agent session in a terminal interface.
#[derive(Parser)]
#[command(name = "styra", about, version)]
struct Cli {
    /// Agent profile to launch.
    #[arg(long, default_value = "codex")]
    profile: String,
    /// Host directory mounted writable as the agent workspace.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Permit agent networking (profiles may default this on).
    #[arg(long)]
    network: bool,
    /// Open a captured journal read-only instead of launching an agent.
    #[arg(long, value_name = "SESSION")]
    attach: Option<PathBuf>,
    /// Optional first message, sent to seed the opening turn.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // The terminal application is wired up in a later task; for now this
    // establishes the command-line surface and that the crate builds and runs.
    let prompt = cli.prompt.join(" ");
    eprintln!(
        "styra: profile={} workspace={:?} network={} attach={:?} prompt={:?}",
        cli.profile, cli.workspace, cli.network, cli.attach, prompt
    );
    Ok(())
}
