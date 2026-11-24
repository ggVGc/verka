use super::*;

#[test]
fn default_socket_directory_and_socket_are_private() {
    let root =
        std::env::temp_dir().join(format!("styra-server-permissions-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    let socket = root.join("styra/styra.sock");
    let listener = bind_socket(&socket, true).unwrap();

    let directory_mode = std::fs::metadata(socket.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let socket_mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(directory_mode, 0o700);
    assert_eq!(socket_mode, 0o600);

    drop(listener);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn only_one_process_owner_can_lock_the_standalone_store() {
    let root = std::env::temp_dir().join(format!(
        "styra-standalone-lock-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();

    let first = lock_standalone_store(&root).unwrap();
    let error = lock_standalone_store(&root).unwrap_err();
    assert!(error
        .to_string()
        .contains("another Styra standalone process"));

    drop(first);
    // Other tests may fork while `first` is open. A fork briefly inherits
    // the flock until exec closes the descriptor, so allow that handoff to
    // finish before asserting that ownership was released.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        match lock_standalone_store(&root) {
            Ok(lock) => {
                drop(lock);
                break;
            }
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("standalone lock was not released: {error:#}"),
        }
    }
    std::fs::remove_dir_all(root).ok();
}
