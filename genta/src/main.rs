//! The `genta` command-line tool: utilities over the coding-agent library.
//!
//! The first command, `render`, turns a recorded session log — the
//! newline-delimited wire events of a codex or Claude Code run, as captured
//! verbatim by hosts like Styra and Orka — into a readable text transcript
//! through the same decoders a live session uses.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use genta::event::Protocol;
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "genta", about = "Coding-agent session tooling", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a recorded session log as a text transcript.
    Render {
        /// The log file: one wire event per line, exactly as the agent
        /// emitted them. `-` reads from stdin.
        path: PathBuf,
        /// The wire protocol the log was recorded in.
        #[arg(long, value_enum, default_value_t = Wire::Codex)]
        protocol: Wire,
        /// Also show events with no rendered view (control traffic, unknown
        /// envelopes) instead of skipping them.
        #[arg(long)]
        all: bool,
    },
}

/// Command-line names for the supported wire protocols.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Wire {
    /// `codex exec --json` output.
    Codex,
    /// `codex app-server` JSON-RPC traffic.
    CodexAppServer,
    /// Claude Code `--output-format stream-json` output.
    Claude,
}

impl From<Wire> for Protocol {
    fn from(wire: Wire) -> Protocol {
        match wire {
            Wire::Codex => Protocol::CodexJsonl,
            Wire::CodexAppServer => Protocol::CodexAppServer,
            Wire::Claude => Protocol::ClaudeJsonl,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Render { path, protocol, all } => {
            let log = read_input(&path)?;
            print!("{}", genta::render::render(&log, protocol.into(), all));
        }
    }
    Ok(())
}

fn read_input(path: &PathBuf) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut log = String::new();
        std::io::stdin()
            .read_to_string(&mut log)
            .context("reading the session log from stdin")?;
        Ok(log)
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("reading the session log {}", path.display()))
    }
}
