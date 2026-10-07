use nota::{GitTrailerStore, ReviewStore};
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Output};

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

fn show(repository: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nota"))
        .args(["show", "--repository"])
        .arg(repository)
        .args(args)
        .output()
        .unwrap()
}

fn parse(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_shows_a_review_as_json() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "-b", "main"]);
    git(repository, &["config", "user.name", "Nota test"]);
    git(repository, &["config", "user.email", "nota@example.test"]);
    git(repository, &["commit", "--allow-empty", "-m", "Subject"]);
    let store = GitTrailerStore::default();
    let started = store
        .start_review(repository, "HEAD", Some("nota/json"))
        .unwrap();
    assert_eq!(
        parse(&show(repository, &["--branch", "nota/json", "--json"])),
        json!({
            "branch": "nota/json",
            "marker": started.marker,
            "subject": started.subject,
            "entries": [],
        })
    );

    let note = store
        .add_note(
            repository,
            "nota/json",
            "Please explain\n\nthe error handling.",
            None,
        )
        .unwrap();
    git(repository, &["switch", "nota/json"]);
    std::fs::write(repository.join("a.txt"), "A\n").unwrap();
    std::fs::write(repository.join("b.txt"), "B\n").unwrap();
    git(repository, &["add", "a.txt", "b.txt"]);
    git(
        repository,
        &["commit", "-m", "Clarify behavior\n\nIn detail."],
    );
    let suggestion = git(repository, &["rev-parse", "HEAD"]);

    // The checked-out review is the default, as in text output.
    let review = parse(&show(repository, &["--json"]));
    assert_eq!(
        review["entries"],
        json!([
            {
                "commit": note.commit,
                "message": "Please explain\n\nthe error handling.",
                "kind": "note",
                "paths": [],
                "source": null,
            },
            {
                "commit": suggestion,
                "message": "Clarify behavior\n\nIn detail.",
                "kind": "suggestion",
                "paths": ["a.txt", "b.txt"],
                "source": null,
            },
        ])
    );
    assert_eq!(
        parse(&show(repository, &["--branch", "nota/json", "--json"])),
        review
    );
    assert!(!show(repository, &["--branch", "main", "--json"])
        .status
        .success());
}
