use nota::{
    add_note, commit_suggestion, load_review, load_review_ref, start_review, GitProvider,
    ReviewEntryKind, ReviewProvider,
};
use std::path::{Path, PathBuf};

fn git(repository: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn configure(repository: &Path) {
    git(repository, &["config", "user.name", "Nota Test"]);
    git(
        repository,
        &["config", "user.email", "nota-test@example.invalid"],
    );
}

fn project() -> (tempfile::TempDir, PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let repository = temp.path().join("project");
    std::fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "--quiet"]);
    configure(&repository);
    std::fs::write(repository.join("README.md"), "subject\n").unwrap();
    git(&repository, &["add", "README.md"]);
    git(&repository, &["commit", "--quiet", "-m", "subject"]);
    let subject = git(&repository, &["rev-parse", "HEAD"]);
    (temp, repository, subject)
}

#[test]
fn git_provider_resolves_an_exact_commit() {
    let (_temp, repository, head) = project();
    let subject = GitProvider::new(&repository)
        .resolve_subject("HEAD")
        .unwrap();
    assert_eq!(subject.repository, repository);
    assert_eq!(subject.revision, head);
}

#[test]
fn review_branch_records_notes_and_staged_suggestions_as_commits() {
    let (_temp, repository, subject) = project();
    let original_branch = git(&repository, &["branch", "--show-current"]);
    let started = start_review(
        &GitProvider::new(&repository),
        "HEAD",
        Some("nota/review-one"),
    )
    .unwrap();

    assert_eq!(started.subject, subject);
    assert_eq!(
        git(&repository, &["branch", "--show-current"]),
        original_branch
    );
    assert_eq!(
        git(&repository, &["rev-parse", &format!("{}^", started.marker)]),
        subject
    );
    let review = load_review_ref(&repository, "nota/review-one").unwrap();
    assert_eq!(review.subject, subject);
    assert!(review.entries.is_empty());

    git(&repository, &["switch", "--quiet", "nota/review-one"]);
    std::fs::write(repository.join("suggested.txt"), "suggested\n").unwrap();
    git(&repository, &["add", "suggested.txt"]);

    let note = add_note(&repository, "Please explain this behavior.").unwrap();
    assert_eq!(note.kind, ReviewEntryKind::Note);
    assert_eq!(
        git(&repository, &["diff", "--cached", "--name-only"]),
        "suggested.txt",
        "adding a note must preserve unrelated staged edits"
    );

    let suggestion = commit_suggestion(&repository, "Make the behavior explicit.").unwrap();
    assert_eq!(suggestion.kind, ReviewEntryKind::Suggestion);
    assert_eq!(suggestion.paths, vec!["suggested.txt"]);

    let review = load_review(&repository).unwrap();
    assert_eq!(review.branch, "nota/review-one");
    assert_eq!(review.subject, subject);
    assert_eq!(review.entries.len(), 2);
    assert_eq!(review.entries[0].commit, note.commit);
    assert_eq!(review.entries[1].commit, suggestion.commit);
}

#[test]
fn suggestion_requires_a_review_branch_and_staged_changes() {
    let (_temp, repository, _) = project();
    let error = commit_suggestion(&repository, "comment").unwrap_err();
    assert!(format!("{error:#}").contains("no review marker"));

    start_review(&GitProvider::new(&repository), "HEAD", Some("nota/empty")).unwrap();
    git(&repository, &["switch", "--quiet", "nota/empty"]);
    let error = commit_suggestion(&repository, "comment").unwrap_err();
    assert!(format!("{error:#}").contains("no staged changes"));
}
