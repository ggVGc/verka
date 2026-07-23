//! Drive a real codex session through Styra's `Session` without the terminal
//! UI, printing every update. This is a manual harness for verifying the live
//! pipeline (Driva launch, pipe capture, protocol handshake, decode, journal)
//! against an installed codex; it needs a codex login, network, and bubblewrap.
//!
//! Usage, from a workspace directory you are willing to expose to the agent:
//!
//! ```sh
//! cargo run --example headless_session                       # codex (app-server), two turns
//! cargo run --example headless_session -- codex-exec "..."   # one-shot exec profile
//! ```

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use styra::agent::{MountSpec, Profile, SandboxLayout};
use styra::event::AgentEvent;
use styra::journal::Journal;
use styra::session::{Session, SessionSpec, SessionUpdate};

/// Print updates until the predicate matches one (or the timeout passes).
fn wait_for(
    updates: &Receiver<SessionUpdate>,
    what: &str,
    matches: impl Fn(&SessionUpdate) -> bool,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(240);
    while std::time::Instant::now() < deadline {
        match updates.recv_timeout(Duration::from_secs(1)) {
            Ok(update) => {
                match &update {
                    SessionUpdate::Event(event) => {
                        println!("EVENT  {:<9} {}", event.tag(), event.summary())
                    }
                    SessionUpdate::Raw(raw) => println!("RAW    {:?}: {}", raw.direction, raw.text),
                    SessionUpdate::Log(entry) => {
                        println!("LOG    {:?}: {}", entry.level, entry.message)
                    }
                    SessionUpdate::Ended(end) => {
                        println!("ENDED  exit={:?} error={:?}", end.exit_code, end.error)
                    }
                }
                if matches(&update) {
                    return true;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    println!("(gave up waiting for {what})");
    false
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1).peekable();
    let profile_name = match args.peek().map(String::as_str) {
        Some("codex") | Some("codex-exec") => args.next().unwrap(),
        _ => "codex".to_string(),
    };
    let prompt = {
        let joined = args.collect::<Vec<_>>().join(" ");
        if joined.trim().is_empty() {
            "Reply with exactly the word: hello. Do not run any commands.".to_string()
        } else {
            joined
        }
    };

    let layout = SandboxLayout::default();
    let profile = Profile::builtin(&profile_name, &layout)?;
    let single_turn = profile.single_turn;
    let workspace = std::env::current_dir()?.canonicalize()?;
    let store = std::env::temp_dir().join("styra-headless");
    let (journal, id) = Journal::create_in_store(&store, &profile)?;
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

    println!("session {id}; profile {profile_name}; prompt: {prompt:?}\n");
    let (session, updates) = Session::spawn(spec, backend, journal, id, diagnostics)?;
    session.send(&prompt)?;

    let turn_done = |update: &SessionUpdate| {
        matches!(update, SessionUpdate::Event(AgentEvent::TurnCompleted { .. }))
    };
    if single_turn {
        wait_for(&updates, "session end", |u| matches!(u, SessionUpdate::Ended(_)));
    } else {
        // Two turns prove the bidirectional protocol: the second message goes
        // out over the same process after the first turn completes.
        if wait_for(&updates, "first turn", turn_done) {
            println!("\n--- second turn ---\n");
            session.send("Now reply with exactly the word: goodbye.")?;
            wait_for(&updates, "second turn", turn_done);
        }
        session.stop();
        wait_for(&updates, "session end", |u| matches!(u, SessionUpdate::Ended(_)));
    }
    drop(session);
    Ok(())
}
