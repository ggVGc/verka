//! Drive a Styra session through the public Unix-socket API without the TUI.
//!
//! Start `styra-server`, then run:
//!
//! ```sh
//! cargo run --example headless_session -- "Reply with exactly: hello"
//! ```

use std::time::{Duration, Instant};

use styra_server::api::CreateSession;
use styra_server::event::AgentEvent;
use styra_server::{Client, JobUpdate};

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
    let session = client.create_session(&CreateSession {
        profile: "codex".into(),
        workspace: std::env::current_dir()?.canonicalize()?,
        network: false,
        message: Some(prompt),
    })?;
    println!("session {}", session.id);

    let deadline = Instant::now() + Duration::from_secs(240);
    let mut cursor = 0;
    while Instant::now() < deadline {
        let batch = client.updates(&session.id, cursor)?;
        cursor = batch.next;
        for item in batch.updates {
            match item.update {
                JobUpdate::Event(event) => {
                    println!("EVENT  {:<9} {}", event.tag(), event.summary());
                    if matches!(event, AgentEvent::TurnCompleted { .. }) {
                        client.stop_session(&session.id)?;
                        return Ok(());
                    }
                }
                JobUpdate::Raw(raw) => println!("RAW    {:?}: {}", raw.direction, raw.text),
                JobUpdate::Log(entry) => {
                    println!("LOG    {:?}: {}", entry.level, entry.message)
                }
                JobUpdate::Ended(end) => {
                    println!("ENDED  exit={:?} error={:?}", end.exit_code, end.error);
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    client.stop_session(&session.id)?;
    anyhow::bail!("timed out waiting for the session")
}
