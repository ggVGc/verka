//! With no live jobs, the server should retire itself after its idle timeout
//! and clean up its socket.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use styra_server::Client;

#[test]
fn exits_after_idle_with_no_jobs() {
    let root = std::env::temp_dir().join(format!("styra-idle-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("styra.sock");

    let mut child = Command::new(env!("CARGO_BIN_EXE_styra-server"))
        .arg("--socket")
        .arg(&socket)
        .arg("--store")
        .arg(root.join("store"))
        .arg("--idle-timeout")
        .arg("1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning styra-server");

    let client = Client::new(&socket);
    let up = Instant::now() + Duration::from_secs(5);
    while client.health().is_err() {
        assert!(Instant::now() < up, "server never came up");
        std::thread::sleep(Duration::from_millis(50));
    }

    // No jobs were ever created, so the idle clock should run out and the
    // process should exit on its own.
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if child.try_wait().expect("polling the server").is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "server did not idle-exit");
        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(!socket.exists(), "idle exit should remove the socket");
    std::fs::remove_dir_all(&root).ok();
}
