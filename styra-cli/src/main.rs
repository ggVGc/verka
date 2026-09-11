//! A deliberately small, script-friendly client for `styra-server`.
//!
//! It owns only the Unix-socket JSONL transport; the request and response
//! vocabulary comes from `styra-protocol`, so this binary stays compatible
//! with the server without depending on its implementation crate.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use styra_protocol::{Request, Response, WireResponse};

#[derive(Parser)]
#[command(name = "styractl", about = "Control a running styra-server", version)]
struct Cli {
    /// Styra server Unix socket (default: $XDG_RUNTIME_DIR/styra/styra.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the server is reachable.
    Health,
    /// List live interactions as JSON.
    Interactions,
    /// List durable sessions in a Workspace as JSON.
    Sessions {
        #[arg(long)]
        workspace: String,
    },
    /// Send one message to a live session.
    Send {
        session: String,
        #[arg(trailing_var_arg = true, required = true)]
        message: Vec<String>,
    },
    /// Interrupt the turn running in a live session.
    Interrupt { session: String },
    /// Stop a live session's agent process.
    Stop { session: String },
    /// Stop a live session and remove it from the live-interaction list.
    Close { session: String },
    /// Ask the server to exit.
    Shutdown,
    /// Attach to a session and emit its updates as JSON Lines until interrupted.
    Follow {
        session: String,
        /// Resume after this update sequence instead of replaying history.
        #[arg(long, default_value_t = 0)]
        after: u64,
        /// Include the verbatim provider wire lines.
        #[arg(long)]
        raw: bool,
        /// Polling interval in milliseconds.
        #[arg(long, default_value_t = 250)]
        interval: u64,
    },
    /// Send any protocol request encoded as JSON and print its JSON response.
    /// Pass - to read the request from standard input.
    Request { json: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or(default_socket()?);
    match cli.command {
        Command::Health => print_response(exchange(&socket, Request::Health)?)?,
        Command::Interactions => print_response(exchange(&socket, Request::ListInteractions)?)?,
        Command::Sessions { workspace } => print_response(exchange(
            &socket,
            Request::ListSessions {
                workspace_id: workspace,
            },
        )?)?,
        Command::Send { session, message } => print_response(exchange(
            &socket,
            Request::SendMessage {
                id: session,
                message: styra_protocol::SendMessage::new(message.join(" ")),
            },
        )?)?,
        Command::Interrupt { session } => print_response(exchange(
            &socket,
            Request::InterruptInteraction { id: session },
        )?)?,
        Command::Stop { session } => {
            print_response(exchange(&socket, Request::StopInteraction { id: session })?)?
        }
        Command::Close { session } => print_response(exchange(
            &socket,
            Request::CloseInteraction { id: session },
        )?)?,
        Command::Shutdown => print_response(exchange(&socket, Request::Shutdown)?)?,
        Command::Follow {
            session,
            after,
            raw,
            interval,
        } => follow(
            &socket,
            &session,
            after,
            raw,
            Duration::from_millis(interval),
        )?,
        Command::Request { json } => {
            let json = if json == "-" {
                let mut input = String::new();
                io::stdin().read_to_string(&mut input)?;
                input
            } else {
                json
            };
            let request = serde_json::from_str(&json).context("parsing protocol request JSON")?;
            print_response(exchange(&socket, request)?)?;
        }
    }
    Ok(())
}

fn follow(
    socket: &std::path::Path,
    id: &str,
    mut after: u64,
    raw: bool,
    interval: Duration,
) -> Result<()> {
    loop {
        let response = exchange(
            socket,
            Request::Updates {
                id: id.to_owned(),
                after,
                raw,
            },
        )?;
        let Response::Updates(updates) = response else {
            bail!("Styra protocol error: expected updates response");
        };
        for update in updates.updates {
            println!("{}", serde_json::to_string(&update)?);
        }
        io::stdout().flush()?;
        after = updates.next;
        thread::sleep(interval);
    }
}

fn exchange(socket: &std::path::Path, request: Request) -> Result<Response> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to Styra socket {}", socket.display()))?;
    serde_json::to_writer(&mut stream, &request).context("encoding Styra request")?;
    stream.write_all(b"\n").context("writing Styra request")?;
    stream.flush().context("flushing Styra request")?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("reading Styra response")?;
    if line.is_empty() {
        bail!("Styra server closed the connection without a response");
    }
    match serde_json::from_str::<WireResponse>(&line).context("parsing Styra response")? {
        WireResponse::Ok { response } => Ok(response),
        WireResponse::Error { error } => bail!("Styra server: {error}"),
    }
}

fn print_response(response: Response) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

fn default_socket() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|path| !path.is_empty())
        .context("XDG_RUNTIME_DIR is not set; pass --socket explicitly")?;
    Ok(PathBuf::from(runtime).join("styra/styra.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_request_has_the_protocol_shape() {
        let request = Request::SendMessage {
            id: "styra-1".into(),
            message: styra_protocol::SendMessage::new("hello"),
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["operation"], "send_message");
        assert_eq!(value["data"]["id"], "styra-1");
        assert_eq!(value["data"]["message"]["text"], "hello");
    }
}
