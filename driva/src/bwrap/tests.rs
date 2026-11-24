use super::*;
use std::sync::Arc;

#[test]
fn controlled_wait_force_terminates_a_running_child() {
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let control = Arc::new(ExecutionControl::default());
    let trigger = Arc::clone(&control);
    let terminator = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        trigger.terminate();
    });

    let started = std::time::Instant::now();
    let status = wait_for_child(child, Some(&control)).unwrap();
    terminator.join().unwrap();

    assert!(!status.success());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn controlled_wait_publishes_the_child_process_id_while_it_runs() {
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    let control = Arc::new(ExecutionControl::default());
    let observer = Arc::clone(&control);
    let watcher = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while observer.process_id().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let seen = observer.process_id();
        observer.terminate();
        seen
    });

    wait_for_child(child, Some(&control)).unwrap();

    assert_eq!(watcher.join().unwrap(), Some(pid));
    assert_eq!(control.process_id(), None);
}
