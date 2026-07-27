//! End-to-end proof that the shell and protocol agent share one Bubblewrap
//! sandbox while the existing raw journal remains authoritative.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use styra_server::api::{CreateSession, CreateWorkspace};
use styra_server::event::AgentEvent;
use styra_server::{Client, InteractionUpdate};

fn integration_tools_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
        && Command::new("bwrap")
            .args([
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--unshare-all",
                "--die-with-parent",
                "--",
                "/bin/true",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(path.exists(), "{} did not appear", path.display());
}

fn wait_for_server(client: &Client, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if client.health().is_ok() {
            return;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "styra-server exited before becoming healthy"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("styra-server did not become healthy");
}

#[test]
fn tmux_shell_runs_in_the_live_agent_sandbox_without_replacing_protocol_pipes() {
    if !integration_tools_available() {
        eprintln!("skipping: usable bubblewrap and tmux are required");
        return;
    }

    // Bubblewrap replaces /tmp with a private tmpfs, so the fake agent binary
    // must live at a host path that remains visible through the read-only root.
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("styra-tmux-integration-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let bin = root.join("bin");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let codex = bin.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
    *'"method":"thread/start"'*) printf '%s\n' '{"id":2,"result":{"thread":{"id":"fake-thread"}}}' ;;
    *'"method":"turn/start"'*) printf '%s\n' '{"method":"item/completed","params":{"item":{"type":"agentMessage","text":"fake reply"}}}' ;;
  esac
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();

    let runtime = std::env::temp_dir().join(format!("styra-tmux-runtime-{}", std::process::id()));
    let socket = runtime.join("styra.sock");
    let store = root.join("store");
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_styra-server"))
        .args([
            "--socket",
            &socket.to_string_lossy(),
            "--store",
            &store.to_string_lossy(),
        ])
        .env("PATH", path)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let client = Client::new(&socket);
    wait_for_server(&client, &mut server);

    let owning_workspace = client
        .create_workspace(&CreateWorkspace {
            host_path: workspace.clone(),
            name: Some("tmux test".into()),
        })
        .unwrap();
    let session = client
        .create_session(&CreateSession {
            workspace_id: owning_workspace.id,
            selection: styra_server::agent::Selection::new(styra_server::agent::Provider::Codex),
            network: false,
            templates: Vec::new(),
            message: None,
        })
        .unwrap();
    let shell = client.shell(&session.id).unwrap();
    assert!(shell.socket.exists());
    let control_mode = std::fs::metadata(shell.socket.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(control_mode, 0o700);

    let proof = workspace.join("shell-proof");
    let status = Command::new(&shell.tmux)
        .arg("-S")
        .arg(&shell.socket)
        .args([
            "send-keys",
            "-t",
            "shell",
            "printf shell-ok > shell-proof",
            "C-m",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    wait_for(&proof);
    assert_eq!(std::fs::read_to_string(&proof).unwrap(), "shell-ok");

    client.send_message(&session.id, "hello").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut cursor = 0;
    let mut reply = false;
    while !reply && Instant::now() < deadline {
        let batch = client.updates(&session.id, cursor).unwrap();
        cursor = batch.next;
        reply = batch.updates.iter().any(|update| {
            matches!(
                &update.update,
                InteractionUpdate::Event(AgentEvent::AgentMessage { text }) if text == "fake reply"
            )
        });
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(reply, "the agent protocol reply should still reach Styra");
    let stored = client.stored_session(&session.id).unwrap();
    assert!(stored
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::UserMessage { text } if text == "hello")));
    assert!(stored
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::AgentMessage { text } if text == "fake reply")));
    assert!(
        stored
            .raw
            .iter()
            .all(|line| !line.text.contains("shell-proof")),
        "shell terminal traffic must not enter the agent journal"
    );

    client.stop_interaction(&session.id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while shell.socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !shell.socket.exists(),
        "tmux should end with the live interaction"
    );

    client.shutdown().ok();
    let _ = server.wait();
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(runtime).ok();
}
