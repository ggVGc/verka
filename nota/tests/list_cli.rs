use nota::{GitTrailerStore, ReviewStore};
use serde_json::Value;
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

fn list(repository: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nota"))
        .args(["list", "--repository"])
        .arg(repository)
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_lists_local_reviews_from_git_and_linked_worktrees() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "-b", "main"]);
    git(repository, &["config", "user.name", "Nota test"]);
    git(repository, &["config", "user.email", "nota@example.test"]);
    git(repository, &["commit", "--allow-empty", "-m", "Subject"]);
    let empty = json(&list(repository, &["--json"]));
    assert_eq!(empty["reviews"], serde_json::json!([]));
    assert_eq!(empty["diagnostics"], serde_json::json!([]));

    let store = GitTrailerStore::default();
    let started = store
        .start_review(repository, "HEAD", Some("custom/review"))
        .unwrap();
    store
        .add_note(repository, "custom/review", "Please explain", None)
        .unwrap();
    git(repository, &["switch", "custom/review"]);
    std::fs::write(repository.join("suggestion.txt"), "Suggested change\n").unwrap();
    git(repository, &["add", "suggestion.txt"]);
    git(repository, &["commit", "-m", "Clarify behavior"]);
    let tip = git(repository, &["rev-parse", "HEAD"]);
    // A matching tag must not make the local branch name ambiguous, and
    // remote tracking refs are not additional reviews.
    git(repository, &["tag", "custom/review", "main"]);
    git(
        repository,
        &["update-ref", "refs/remotes/origin/custom/review", &tip],
    );
    git(repository, &["switch", "main"]);
    let before = git(repository, &["status", "--porcelain"]);
    let output = list(repository, &["--subject", "HEAD", "--json"]);
    let index = json(&output);
    assert!(output.stderr.is_empty());
    assert_eq!(index["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(index["reviews"][0]["branch"], "custom/review");
    assert_eq!(index["reviews"][0]["marker"], started.marker);
    assert_eq!(index["reviews"][0]["subject"], started.subject);
    assert_eq!(index["reviews"][0]["tip"], tip);
    assert_eq!(index["reviews"][0]["notes"], 1);
    assert_eq!(index["reviews"][0]["suggestions"], 1);
    assert_eq!(git(repository, &["status", "--porcelain"]), before);
    assert_eq!(
        git(repository, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );

    let text = list(repository, &[]);
    assert!(text.status.success());
    assert!(String::from_utf8(text.stdout)
        .unwrap()
        .contains("1 notes  1 suggestions"));
    let no_match = json(&list(repository, &["--subject", &tip, "--json"]));
    assert_eq!(no_match["reviews"], serde_json::json!([]));
    assert!(!list(repository, &["--subject", "missing-revision"])
        .status
        .success());

    let worktree = repository.join("linked");
    git(
        repository,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            "main",
        ],
    );
    assert_eq!(json(&list(&worktree, &["--json"])), index);

    // Ordinary commits are observed on the next read, including invalid ones.
    git(repository, &["tag", "-d", "custom/review"]);
    git(repository, &["switch", "custom/review"]);
    git(
        repository,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "Another note\n\nNota-Note: true",
        ],
    );
    let refreshed = json(&list(repository, &["--json"]));
    assert_eq!(refreshed["reviews"][0]["notes"], 2);
    git(repository, &["switch", "main"]);
    git(repository, &["branch", "-D", "custom/review"]);
    assert_eq!(
        json(&list(repository, &["--json"]))["reviews"],
        serde_json::json!([])
    );
}

#[test]
fn cli_reports_malformed_reviews_in_text_and_json() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path();
    git(repository, &["init", "-b", "main"]);
    git(repository, &["config", "user.name", "Nota test"]);
    git(repository, &["config", "user.email", "nota@example.test"]);
    git(repository, &["commit", "--allow-empty", "-m", "Subject"]);
    let store = GitTrailerStore::default();
    store
        .start_review(repository, "HEAD", Some("valid"))
        .unwrap();
    store
        .start_review(repository, "HEAD", Some("invalid"))
        .unwrap();
    git(repository, &["switch", "invalid"]);
    git(
        repository,
        &["commit", "--allow-empty", "-m", "Invalid suggestion"],
    );
    let index = json(&list(repository, &["--json"]));
    assert_eq!(index["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(index["reviews"][0]["branch"], "valid");
    assert_eq!(index["diagnostics"][0]["branch"], "invalid");
    assert!(index["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("no changed project files"));
    let text = list(repository, &[]);
    assert!(text.status.success());
    assert!(String::from_utf8(text.stdout).unwrap().contains("valid"));
    assert!(String::from_utf8(text.stderr)
        .unwrap()
        .contains("warning: invalid:"));
}
