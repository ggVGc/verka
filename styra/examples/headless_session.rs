//! Drive a real codex session through Styra's `Session` without the terminal
//! UI, printing every update. This is a manual harness for verifying the live
//! pipeline (Driva launch, pipe capture, decode, journal) against an installed
//! codex; it needs a codex login, network, and bubblewrap.
//!
//! Run from a workspace directory you are willing to expose to the agent:
//!
//! ```sh
//! cargo run --example headless_session -- "Reply with exactly: hello"
//! ```

use std::path::PathBuf;
use std::time::Duration;

use styra::agent::{MountSpec, Profile, SandboxLayout};
use styra::journal::Journal;
use styra::session::{Session, SessionSpec, SessionUpdate};

fn main() -> anyhow::Result<()> {
    let prompt = {
        let joined = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        if joined.trim().is_empty() {
            "Reply with exactly the word: hello. Do not run any commands.".to_string()
        } else {
            joined
        }
    };

    let layout = SandboxLayout::default();
    let profile = Profile::builtin("codex", &layout)?;
    let workspace = std::env::current_dir()?.canonicalize()?;
    let store = std::env::temp_dir().join("styra-headless");
    let (journal, id) = Journal::create_in_store(&store)?;
    let diagnostics = journal
        .path()
        .parent()
        .unwrap_or(&store)
        .join("diagnostics.log");

    let spec = SessionSpec {
        profile,
        working_directory: layout.workspace.clone(),
        workspace: MountSpec {
            source: workspace,
            destination: layout.workspace.clone(),
            writable: true,
        },
        temporary_mounts: Vec::new(),
    };
    let backend = Box::new(driva::BwrapIsolation {
        executable: "bwrap".into(),
        rootfs: Some(PathBuf::from("/")),
    });

    println!("session {id}; prompt: {prompt:?}\n");
    let (session, updates) = Session::spawn(spec, backend, journal, id, diagnostics)?;
    session.send(&prompt)?;

    loop {
        match updates.recv_timeout(Duration::from_secs(240)) {
            Ok(SessionUpdate::Event(event)) => {
                println!("EVENT  {:<9} {}", event.tag(), event.summary());
            }
            Ok(SessionUpdate::Raw(raw)) => println!("RAW    {:?}: {}", raw.direction, raw.text),
            Ok(SessionUpdate::Log(entry)) => println!("LOG    {:?}: {}", entry.level, entry.message),
            Ok(SessionUpdate::Ended(end)) => {
                println!("ENDED  exit={:?} error={:?}", end.exit_code, end.error);
                break;
            }
            Err(error) => {
                println!("(stopped waiting: {error})");
                break;
            }
        }
    }
    drop(session);
    Ok(())
}
