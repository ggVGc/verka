use serde_json::{json, Value};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn git(repository: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn nota(repository: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nota"))
        .args(args)
        .arg("--repository")
        .arg(repository)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn succeed(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn fail(output: Output) -> String {
    assert!(!output.status.success());
    String::from_utf8(output.stderr).unwrap()
}

/// A repository whose `main` holds `file.txt` with lines one to five, under
/// review on `nota/lines`.
fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "-b", "main"]);
    git(repository, &["config", "user.name", "Nota test"]);
    git(repository, &["config", "user.email", "nota@example.test"]);
    std::fs::write(repository.join("file.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    git(repository, &["add", "file.txt"]);
    git(repository, &["commit", "-m", "Subject"]);
    succeed(nota(
        repository,
        &["start", "HEAD", "--branch", "nota/lines"],
        "",
    ));
    directory
}

fn sources(repository: &Path) -> Value {
    let review: Value = serde_json::from_str(&succeed(nota(
        repository,
        &["show", "--branch", "nota/lines", "--json"],
        "",
    )))
    .unwrap();
    review["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["source"].clone())
        .collect()
}

#[test]
fn a_note_records_its_lines_against_a_commit() {
    let directory = repository();
    let repository = directory.path();
    let head = git(repository, &["rev-parse", "HEAD"]);
    let note = |args: &[&str], stdin: &str| {
        let mut all = vec!["note", "A note.", "--branch", "nota/lines"];
        all.extend(args);
        succeed(nota(repository, &all, stdin));
    };

    note(&["--path", "file.txt", "--lines", "2-3"], "");
    // Uncommitted edits move the lines; they are recorded as HEAD has them.
    std::fs::write(
        repository.join("file.txt"),
        "zero\none\ntwo\nthree\nfour\nFIVE\n",
    )
    .unwrap();
    note(&["--path", "file.txt", "--lines", "4"], "");
    // So do unsaved ones given as contents; a new line goes to the line
    // before it.
    note(
        &["--path", "file.txt", "--lines", "2-3", "--contents", "-"],
        "one\nnew\ntwo\nthree\nfour\nfive\n",
    );
    // A changed line keeps its place.
    note(&["--path", "file.txt", "--lines", "6"], "");
    note(&[], "");

    let at = |first: usize, last: usize| json!({ "revision": head, "path": "file.txt", "first": first, "last": last });
    assert_eq!(
        sources(repository),
        json!([at(2, 3), at(3, 3), at(1, 2), at(5, 5), null])
    );
    let message = git(
        repository,
        &["log", "-1", "--skip=1", "--format=%B", "nota/lines"],
    );
    assert!(
        message.ends_with(&format!(
            "Nota-Note: true\nNota-Source: {head}:file.txt:5-5"
        )),
        "{message}"
    );
}

#[test]
fn a_note_rejects_lines_that_no_commit_has() {
    let directory = repository();
    let repository = directory.path();
    let note = |args: &[&str]| {
        let mut all = vec!["note", "A note.", "--branch", "nota/lines"];
        all.extend(args);
        fail(nota(repository, &all, ""))
    };
    std::fs::write(repository.join("new.txt"), "new\n").unwrap();
    assert!(note(&["--path", "new.txt", "--lines", "1"]).contains("is not in"));
    assert!(note(&["--path", "file.txt", "--lines", "3-2"]).contains("is not"));
    assert!(note(&["--path", "file.txt"]).contains("--lines"));
    assert!(note(&[
        "--path",
        "file.txt",
        "--lines",
        "1",
        "--revision",
        "missing"
    ])
    .contains("missing"));
}

#[test]
fn loading_rejects_a_malformed_source() {
    let directory = repository();
    let repository = directory.path();
    let tip = git(
        repository,
        &[
            "commit-tree",
            "nota/lines^{tree}",
            "-p",
            "nota/lines",
            "-m",
            "Bad.\n\nNota-Note: true\nNota-Source: file.txt:1",
        ],
    );
    git(repository, &["update-ref", "refs/heads/nota/lines", &tip]);
    let error = fail(nota(repository, &["show", "--branch", "nota/lines"], ""));
    assert!(error.contains("invalid source"), "{error}");
}
