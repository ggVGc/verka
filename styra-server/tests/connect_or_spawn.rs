//! `ensure_server` should spawn a detached daemon when none is listening,
//! reuse it on a second call, and leave it running after the caller exits.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use styra_server::ensure_server;

/// Isolate XDG dirs and point the spawner at the freshly built binary.
fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!("styra-spawn-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("STYRA_SERVER_BIN", env!("CARGO_BIN_EXE_styra-server"));
    // Keep the daemon's store out of the real state dir.
    std::env::set_var("XDG_STATE_HOME", root.join("state"));
    root
}

#[test]
fn spawns_reuses_and_outlives_the_caller() {
    let root = scratch();
    let socket = root.join("styra.sock");

    // No server yet: this must spawn one and wait for it to answer.
    let client = ensure_server(&socket).expect("first ensure_server should spawn a daemon");
    assert!(socket.exists(), "the daemon should have bound the socket");
    client.health().expect("spawned daemon should be healthy");

    // A second call finds the running daemon and reuses it (fast path).
    let again = ensure_server(&socket).expect("second ensure_server should reuse the daemon");
    again.health().expect("daemon should still be healthy");

    // The live-jobs listing round-trips over the socket; a fresh daemon has
    // none running yet.
    let jobs = client.list_jobs().expect("listing live jobs should succeed");
    assert!(jobs.is_empty(), "a fresh daemon should report no live jobs");

    // The daemon is detached, so it is still serving after we would have exited.
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        client.health().expect("detached daemon should keep serving");
    }

    // Best-effort cleanup: stop serving by removing the socket and the store.
    std::fs::remove_dir_all(&root).ok();
}
