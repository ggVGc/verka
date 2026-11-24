use super::*;
use std::time::Instant;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repository() -> TempDir {
    let root = std::env::temp_dir().join(format!("orka-file-change-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&root).unwrap();
    checked(&root, &["init", "-q"]).unwrap();
    checked(&root, &["config", "user.name", "test"]).unwrap();
    checked(&root, &["config", "user.email", "test@example.com"]).unwrap();
    std::fs::write(root.join("source.rs"), "base\n").unwrap();
    checked(&root, &["add", "source.rs"]).unwrap();
    checked(&root, &["commit", "-qm", "base"]).unwrap();
    TempDir(root)
}

fn append_event(raw: &Path, id: &str) {
    append_event_at(raw, id, "/tmp/orka/workspace/source.rs");
}

fn append_event_at(raw: &Path, id: &str, path: &str) {
    let mut output = OpenOptions::new().append(true).open(raw).unwrap();
    writeln!(
            output,
            r#"{{"type":"item.completed","item":{{"id":"{id}","type":"file_change","changes":[{{"path":"{path}"}}]}}}}"#
        )
        .unwrap();
}

fn wait_for_records(journal: &Path, count: usize) {
    let started = Instant::now();
    while read_checkpoints(journal).unwrap().len() < count {
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn every_file_change_gets_a_commit_without_moving_the_execution_head_or_index() {
    let repository = repository();
    let raw = repository.0.join("events.raw.jsonl");
    let journal = repository.0.join("file-changes.v1.jsonl");
    std::fs::write(&raw, b"").unwrap();
    let head = checked(&repository.0, &["rev-parse", "HEAD"]).unwrap();
    let recorder = FileChangeRecorder::start(
        &repository.0,
        Path::new("/tmp/orka/workspace"),
        Vec::new(),
        &raw,
        &journal,
        "refs/orka/file-changes/test",
    )
    .unwrap();

    std::fs::write(repository.0.join("source.rs"), "first\n").unwrap();
    append_event(&raw, "change-1");
    wait_for_records(&journal, 1);
    std::fs::write(repository.0.join("source.rs"), "second\n").unwrap();
    append_event(&raw, "change-2");
    wait_for_records(&journal, 2);
    recorder.finish().unwrap();

    let records = read_checkpoints(&journal).unwrap();
    let first = records[0].commit.as_deref().unwrap();
    let second = records[1].commit.as_deref().unwrap();
    assert_eq!(
        checked(&repository.0, &["show", &format!("{first}:source.rs")]).unwrap(),
        "first"
    );
    assert_eq!(
        checked(&repository.0, &["show", &format!("{second}:source.rs")]).unwrap(),
        "second"
    );
    assert_eq!(
        checked(&repository.0, &["rev-parse", &format!("{second}^")]).unwrap(),
        first
    );
    assert_eq!(
        checked(&repository.0, &["rev-parse", "HEAD"]).unwrap(),
        head
    );
    assert_eq!(
        checked(&repository.0, &["rev-parse", "refs/orka/file-changes/test"]).unwrap(),
        second
    );
    assert!(checked(&repository.0, &["status", "--porcelain"])
        .unwrap()
        .lines()
        .any(|line| line == "M source.rs"));
    checked(&repository.0, &["diff", "--cached", "--quiet"]).unwrap();
}

#[test]
fn an_outcome_file_change_is_not_treated_as_a_project_checkpoint_failure() {
    let repository = repository();
    let raw = repository.0.join("events.raw.jsonl");
    let journal = repository.0.join("file-changes.v1.jsonl");
    std::fs::write(&raw, b"").unwrap();
    let outcome = PathBuf::from("/tmp/orka/exchange/outcome.toml");
    let recorder = FileChangeRecorder::start(
        &repository.0,
        Path::new("/tmp/orka/workspace"),
        vec![outcome.clone()],
        &raw,
        &journal,
        "refs/orka/file-changes/test",
    )
    .unwrap();

    append_event_at(&raw, "outcome-change", outcome.to_str().unwrap());
    wait_for_records(&journal, 1);
    recorder.finish().unwrap();

    assert_eq!(
        read_checkpoints(&journal).unwrap(),
        vec![FileChangeCheckpoint {
            schema: FILE_CHANGE_SCHEMA,
            sequence: 1,
            event_id: "outcome-change".into(),
            paths: Vec::new(),
            commit: None,
            error: None,
        }]
    );
}
