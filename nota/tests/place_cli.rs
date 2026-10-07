use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

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

fn nota(repository: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nota"))
        .args(args)
        .arg("--repository")
        .arg(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write(repository: &Path, path: &str, lines: &[&str]) {
    let text = lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    std::fs::write(repository.join(path), text).unwrap();
}

/// Each entry's locations as `[path, line, count, status]`, in review order.
fn placed(repository: &Path, at: &str) -> Vec<Value> {
    let review: Value = serde_json::from_str(&nota(
        repository,
        &["show", "--branch", "nota/place", "--json", "--at", at],
    ))
    .unwrap();
    review["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            entry["locations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| json!([l["path"], l["line"], l["count"], l["status"]]))
                .collect()
        })
        .collect()
}

#[test]
fn items_are_placed_in_the_worktree_or_a_revision() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "-b", "main"]);
    git(repository, &["config", "user.name", "Nota test"]);
    git(repository, &["config", "user.email", "nota@example.test"]);
    let subject_lines = ["one", "two", "three", "four", "five", "six", "seven"];
    write(repository, "file.txt", &subject_lines);
    std::fs::write(repository.join("image.bin"), b"\0binary").unwrap();
    git(repository, &["add", "."]);
    git(repository, &["commit", "-m", "Subject"]);
    nota(repository, &["start", "HEAD", "--branch", "nota/place"]);

    // Suggestions are numbered against the review as each found it: the
    // second one's line 7 is the subject's line 6.
    git(repository, &["switch", "-q", "nota/place"]);
    write(
        repository,
        "file.txt",
        &[
            "one", "TWO", "three", "inserted", "four", "five", "six", "seven",
        ],
    );
    git(repository, &["commit", "-qam", "Shout and insert."]);
    write(
        repository,
        "file.txt",
        &[
            "one", "TWO", "three", "inserted", "four", "five", "\tsix", "seven",
        ],
    );
    std::fs::write(repository.join("image.bin"), b"\0changed").unwrap();
    git(repository, &["commit", "-qam", "Indent six."]);
    git(repository, &["switch", "-q", "main"]);
    nota(
        repository,
        &[
            "note",
            "Explain these lines.",
            "--branch",
            "nota/place",
            "--path",
            "file.txt",
            "--lines",
            "4-5",
        ],
    );
    nota(repository, &["note", "General.", "--branch", "nota/place"]);

    // In a checkout of the subject, every hunk is pending at the subject's
    // lines.
    let pending = vec![
        json!([["file.txt", 2, 1, "pending"], ["file.txt", 3, 0, "pending"]]),
        json!([
            ["file.txt", 6, 1, "pending"],
            ["image.bin", null, 0, "unknown"]
        ]),
        json!([["file.txt", 4, 2, "current"]]),
        json!([]),
    ];
    assert_eq!(placed(repository, "worktree"), pending);
    assert_eq!(placed(repository, "main"), pending);

    // Uncommitted edits move items, and make changed lines stale.
    write(
        repository,
        "file.txt",
        &[
            "zero", "one", "two", "three", "four", "FIVE", "SIX", "seven",
        ],
    );
    assert_eq!(
        placed(repository, "worktree"),
        vec![
            json!([["file.txt", 3, 1, "pending"], ["file.txt", 4, 0, "pending"]]),
            json!([
                ["file.txt", 7, 1, "stale"],
                ["image.bin", null, 0, "unknown"]
            ]),
            json!([["file.txt", 5, 2, "changed"]]),
            json!([]),
        ]
    );

    // On the review branch, the suggestions are applied.
    assert_eq!(
        placed(repository, "nota/place"),
        vec![
            json!([["file.txt", 2, 1, "applied"], ["file.txt", 4, 1, "applied"]]),
            json!([
                ["file.txt", 7, 1, "applied"],
                ["image.bin", null, 0, "unknown"]
            ]),
            json!([["file.txt", 5, 2, "current"]]),
            json!([]),
        ]
    );

    // Hunks carry their change, numbered against the suggestion.
    let review: Value = serde_json::from_str(&nota(
        repository,
        &["show", "--branch", "nota/place", "--json", "--at", "main"],
    ))
    .unwrap();
    assert_eq!(
        review["entries"][1]["locations"][0]["hunk"],
        json!({
            "old_start": 7, "old_count": 1, "new_start": 7, "new_count": 1,
            "old": ["six"], "new": ["\tsix"],
        })
    );
    assert_eq!(review["at"], json!(git(repository, &["rev-parse", "main"])));
    assert!(review["entries"][2]["source"].is_object());
}

#[test]
fn a_deletion_is_applied_where_its_neighbours_meet() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "-b", "main"]);
    git(repository, &["config", "user.name", "Nota test"]);
    git(repository, &["config", "user.email", "nota@example.test"]);
    write(repository, "file.txt", &["a", "b", "c"]);
    git(repository, &["add", "."]);
    git(repository, &["commit", "-m", "Subject"]);
    nota(repository, &["start", "HEAD", "--branch", "nota/place"]);
    git(repository, &["switch", "-q", "nota/place"]);
    write(repository, "file.txt", &["a", "c"]);
    git(repository, &["commit", "-qam", "Drop b."]);
    assert_eq!(
        placed(repository, "worktree"),
        vec![json!([["file.txt", 1, 0, "applied"]])]
    );
    assert_eq!(
        placed(repository, "main"),
        vec![json!([["file.txt", 2, 1, "pending"]])]
    );
    write(repository, "file.txt", &["a", "B", "c"]);
    assert_eq!(
        placed(repository, "worktree"),
        vec![json!([["file.txt", 2, 1, "stale"]])]
    );
}
