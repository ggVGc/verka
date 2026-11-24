use super::*;

#[test]
fn summaries_round_trip_and_preserve_repeat_reads() {
    let dir = std::env::temp_dir().join(format!("orka-access-test-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("accesses.v1.jsonl");
    write_access_summary(
        &journal,
        "test",
        &["src/lib.rs".into(), "src/lib.rs".into(), "README.md".into()],
        true,
        None,
    )
    .unwrap();
    let summary = read_access_summary(&journal).unwrap();
    assert!(summary.complete);
    // The timeline keeps every read, in order, each carrying a timestamp.
    let paths: Vec<_> = summary
        .reads
        .iter()
        .map(|read| read.path.as_str())
        .collect();
    assert_eq!(paths, ["src/lib.rs", "src/lib.rs", "README.md"]);
    assert!(summary.reads.iter().all(|read| read.at_ms > 0));
    // Set-oriented callers still see each path once, in first-seen order.
    assert_eq!(summary.distinct_paths(), ["src/lib.rs", "README.md"]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn live_watcher_records_reads_but_not_git_or_outside_files() {
    let dir = std::env::temp_dir().join(format!("orka-access-test-{}", ulid::Ulid::new()));
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "content").unwrap();
    std::fs::write(workspace.join(".git/config"), "git").unwrap();
    let journal = dir.join("accesses.v1.jsonl");

    let recorder = AccessRecorder::start(&workspace, &journal);
    let _ = std::fs::read_to_string(workspace.join("src/lib.rs")).unwrap();
    let _ = std::fs::read_to_string(workspace.join(".git/config")).unwrap();
    recorder.finish().unwrap();

    let summary = read_access_summary(&journal).unwrap();
    assert!(summary.complete, "{:?}", summary.reason);
    // A single read may surface as several kernel events (ACCESS plus
    // CLOSE_NOWRITE); every one is journaled, but only the project file —
    // never `.git` or files outside the workspace.
    assert!(!summary.reads.is_empty());
    assert!(summary.reads.iter().all(|read| read.path == "src/lib.rs"));
    assert!(summary.reads.iter().all(|read| read.at_ms > 0));
    assert_eq!(summary.distinct_paths(), ["src/lib.rs"]);
    std::fs::remove_dir_all(dir).unwrap();
}
