use super::*;
use crate::test_support::{git, repository};

/// Waits a while for the watch to notice a change, as the status is
/// asked only now and then.
fn notices(watch: &mut Watch) -> bool {
    let deadline = std::time::Instant::now() + STATUS_INTERVAL * 3;
    while std::time::Instant::now() < deadline {
        if watch.changed() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(target_os = "linux")]
#[test]
fn a_shown_file_written_again_is_noticed_at_once() {
    let (_temp, root) = repository("styra-diff-watch-files");
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/shown.txt"), "one\n").unwrap();
    std::fs::write(root.join("src/other.txt"), "one\n").unwrap();
    let mut watch = Watch::new(&root);
    watch.follow(["src/shown.txt"]);
    assert!(!watch.changed());

    std::fs::write(root.join("src/other.txt"), "two\n").unwrap();
    assert!(!watch.files.as_mut().unwrap().changed());
    std::fs::write(root.join("src/shown.txt"), "two\n").unwrap();
    assert!(watch.files.as_mut().unwrap().changed());

    // Saved as an editor that writes a new file over the old one does.
    std::fs::write(root.join("src/.shown.txt.swp"), "three\n").unwrap();
    std::fs::rename(root.join("src/.shown.txt.swp"), root.join("src/shown.txt")).unwrap();
    assert!(watch.files.as_mut().unwrap().changed());

    watch.follow(["src/other.txt"]);
    std::fs::write(root.join("src/shown.txt"), "four\n").unwrap();
    assert!(!watch.files.as_mut().unwrap().changed());
}

#[test]
fn a_file_newly_changed_or_a_commit_is_noticed_by_the_status() {
    let (_temp, root) = repository("styra-diff-watch-status");
    let mut watch = Watch::new(&root);
    assert!(!watch.changed());

    std::fs::write(root.join("new.txt"), "new\n").unwrap();
    assert!(notices(&mut watch));
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "new"]);
    assert!(notices(&mut watch));
}
