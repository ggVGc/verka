//! The `genta` command-line tool: utilities over the coding-agent library.
//!
//! The first command, `render`, turns a recorded session log — the
//! newline-delimited wire events of a codex or Claude Code run, as captured
//! verbatim by hosts like Styra and Orka — into a readable text transcript
//! through the same decoders a live session uses.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use genta::event::{decode_line, AgentEvent, DetailBlock, Protocol};
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
            print!("{}", render(&log, protocol.into(), all));
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

/// Render a whole session log: decode each line and lay the events out as a
/// tagged transcript. Multi-line bodies (agent messages, plans, command
/// output) follow their tag line, indented, so the transcript stays scannable.
fn render(log: &str, protocol: Protocol, all: bool) -> String {
    let mut out = String::new();
    for line in log.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let event = decode_line(protocol, line);
        if !all && matches!(event, AgentEvent::Unknown { .. }) {
            continue;
        }
        render_event(&mut out, &event);
    }
    out
}

fn render_event(out: &mut String, event: &AgentEvent) {
    match event {
        // Prose bodies are worth reading in full; everything else reads best
        // as its one-line summary.
        AgentEvent::AgentMessage { .. } | AgentEvent::PlanUpdated { .. } | AgentEvent::UserMessage { .. } => {
            out.push_str(&format!("{:>8} ", event.tag()));
            let mut first = true;
            for block in event.detail() {
                let text = match &block {
                    DetailBlock::Text(text) => text.clone(),
                    DetailBlock::Code { language, text } => format!(
                        "```{}\n{}```",
                        language.as_deref().unwrap_or_default(),
                        text
                    ),
                };
                for line in text.lines() {
                    if first {
                        first = false;
                    } else {
                        out.push_str("         ");
                    }
                    out.push_str(line);
                    out.push('\n');
                }
            }
            if first {
                out.push('\n');
            }
        }
        _ => {
            out.push_str(&format!("{:>8} {}\n", event.tag(), event.summary()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codex_log_renders_as_a_tagged_transcript() {
        let log = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}\n",
            "{\"type\":\"item.started\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test\",\"status\":\"completed\",\"exit_code\":0}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"All good.\\nTests pass.\"}}\n",
        );
        let text = render(log, Protocol::CodexJsonl, false);
        assert_eq!(
            text,
            "  thread session t-1\n\
             \x20command cargo test\n\
             \x20command cargo test (completed, exit 0)\n\
             \x20  agent All good.\n\
             \x20        Tests pass.\n"
        );
    }

    #[test]
    fn a_claude_log_renders_through_the_claude_decoder() {
        let log = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done.\"}]}}\n",
        );
        let text = render(log, Protocol::ClaudeJsonl, false);
        assert!(text.contains("session s-1"));
        assert!(text.contains("agent Done."));
    }

    #[test]
    fn unknown_events_are_skipped_unless_all_is_requested() {
        let log = "{\"type\":\"future.event\"}\n";
        assert_eq!(render(log, Protocol::CodexJsonl, false), "");
        assert!(render(log, Protocol::CodexJsonl, true).contains("future.event"));
    }
}
