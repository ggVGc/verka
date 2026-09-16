//! Ask one question and get a typed answer back, through the public
//! Unix-socket API.
//!
//! This is the whole shape of a one-shot: create a session whose seed message
//! carries a contract, poll the update stream as any client would, and read the
//! parsed answer once the turn completes. The session is left in the store, so
//! the same question can be reopened in the Styra interface and continued.
//!
//! Start `styra-server`, then run:
//!
//! ```sh
//! cargo run --example typed_answer -- files "which files decode agent events?"
//! ```

use std::time::{Duration, Instant};

use styra_server::event::AgentEvent;
use styra_server::protocol::{AnswerValue, Contract, CreateSession, CreateWorkspace};
use styra_server::{Client, InteractionUpdate};

fn contract_named(name: &str) -> anyhow::Result<Contract> {
    Ok(match name {
        "text" => Contract::Text,
        "lines" => Contract::Lines,
        "files" => Contract::Files,
        "json" => Contract::Json,
        other => anyhow::bail!("unknown contract {other:?}: text, lines, files, or json"),
    })
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let contract = contract_named(&args.next().unwrap_or_else(|| "text".into()))?;
    let prompt = args.collect::<Vec<_>>().join(" ");
    anyhow::ensure!(
        !prompt.trim().is_empty(),
        "usage: typed_answer CONTRACT PROMPT"
    );

    let client = Client::new(styra_server::paths::default_socket()?);
    client.health()?;
    let workspace = client.create_workspace(&CreateWorkspace {
        host_path: std::env::current_dir()?.canonicalize()?,
        name: Some("typed-answer".into()),
        git_repository: None,
    })?;
    let session = client.create_session(&CreateSession {
        workspace_id: workspace.id,
        selection: styra_server::agent::Selection::new(styra_server::agent::Provider::Codex),
        launch: Default::default(),
        message: Some(prompt),
        name: None,
        // The server frames the seed message with the contract's instructions
        // and records the contract, so nothing below has to restate the shape.
        contract: Some(contract),
    })?;
    eprintln!("session {} asking for {}", session.id, contract.as_str());

    let deadline = Instant::now() + Duration::from_secs(600);
    let mut cursor = 0;
    while Instant::now() < deadline {
        let batch = client.updates(&session.id, cursor)?;
        cursor = batch.next;
        for item in batch.updates {
            match item.update {
                InteractionUpdate::Event(AgentEvent::TurnCompleted { .. }) => {
                    let answer = client.turn_answer(&session.id)?;
                    client.stop_interaction(&session.id)?;
                    // A reply that missed the contract still came back with
                    // something in it, so show that rather than only the
                    // complaint about its shape.
                    let Some(value) = &answer.value else {
                        eprintln!("{}", answer.error.unwrap_or_default());
                        println!("{}", answer.source);
                        std::process::exit(1);
                    };
                    report(value);
                    return Ok(());
                }
                InteractionUpdate::Event(event) => {
                    eprintln!("  {:<9} {}", event.tag(), event.summary())
                }
                InteractionUpdate::Ended(end) => {
                    anyhow::bail!("the session ended before answering: {end:?}")
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    client.stop_interaction(&session.id)?;
    anyhow::bail!("timed out waiting for the answer")
}

fn report(value: &AnswerValue) {
    match value {
        AnswerValue::Text(text) => println!("{text}"),
        AnswerValue::Lines(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        // Printed the way an editor's error format wants it, which is most of
        // why this contract exists.
        AnswerValue::Files(files) => {
            for file in files {
                if file.description.is_empty() {
                    println!("{}", file.located());
                } else {
                    println!("{}: {}", file.located(), file.description);
                }
            }
        }
        AnswerValue::Json(json) => {
            println!("{}", serde_json::to_string_pretty(json).unwrap_or_default())
        }
    }
}
