//! Drive a Styra session through the public Unix-socket API without the TUI.
//!
//! Start `styra-server`, then run:
//!
//! ```sh
//! cargo run --example headless_session -- "Reply with exactly: hello"
//! ```

use std::time::{Duration, Instant};

use styra_server::event::AgentEvent;
use styra_server::protocol::{CreateSession, CreateWorkspace};
use styra_server::{Client, InteractionUpdate};

fn main() -> anyhow::Result<()> {
    let prompt = {
        let joined = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        if joined.trim().is_empty() {
            "Reply with exactly the word: hello. Do not run any commands.".to_string()
        } else {
            joined
        }
    };
    let socket = styra_server::paths::default_socket()?;
    let client = Client::new(socket);
    client.health()?;
    let workspace = client.create_workspace(&CreateWorkspace {
        host_path: std::env::current_dir()?.canonicalize()?,
        name: Some("headless".into()),
    })?;
    let session = client.create_session(&CreateSession {
        workspace_id: workspace.id,
        selection: styra_server::agent::Selection::new(styra_server::agent::Provider::Codex),
        network: false,
        templates: Vec::new(),
        mounts: Vec::new(),
        message: Some(prompt),
        name: None,
    })?;
    println!("session {}", session.id);

    let deadline = Instant::now() + Duration::from_secs(240);
    let mut cursor = 0;
    while Instant::now() < deadline {
        let batch = client.updates(&session.id, cursor)?;
        cursor = batch.next;
        for item in batch.updates {
            match item.update {
                InteractionUpdate::Event(event) => {
                    println!("EVENT  {:<9} {}", event.tag(), event.summary());
                    if matches!(event, AgentEvent::TurnCompleted { .. }) {
                        client.stop_interaction(&session.id)?;
                        return Ok(());
                    }
                }
                InteractionUpdate::Raw(raw) => println!("RAW    {:?}: {}", raw.direction, raw.text),
                InteractionUpdate::Log(entry) => {
                    println!("LOG    {:?}: {}", entry.level, entry.message)
                }
                InteractionUpdate::WorkingDirectoryChanged(directory) => {
                    println!("CWD    {}", directory.display())
                }
                InteractionUpdate::Ended(end) => {
                    println!("ENDED  exit={:?} error={:?}", end.exit_code, end.error);
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    client.stop_interaction(&session.id)?;
    anyhow::bail!("timed out waiting for the session")
}
