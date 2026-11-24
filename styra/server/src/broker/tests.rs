use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn broker_preserves_agent_stdio_and_cleans_up_tmux() {
    let root = std::env::temp_dir().join(format!("styra-broker-test-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let fake_tmux = root.join("tmux");
    std::fs::write(
            &fake_tmux,
            "#!/bin/sh\nsocket=\"$2\"\ncase \"$3\" in\nnew-session) : > \"$socket\" ;;\nkill-server) rm -f \"$socket\" ;;\nesac\n",
        )
        .unwrap();
    std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
    let socket = root.join("tmux.sock");
    let config = BrokerConfig {
        agent_command: vec!["/bin/sh".into(), "-c".into(), "exit 7".into()],
        tmux: fake_tmux,
        tmux_socket: socket.clone(),
        workdir: root.clone(),
    };

    let status = run(config).unwrap();
    assert_eq!(status.code(), Some(7));
    assert!(!socket.exists(), "the broker should stop its tmux server");
    std::fs::remove_dir_all(root).ok();
}
