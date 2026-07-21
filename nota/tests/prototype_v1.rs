use nota::{
    add_note, load_review, load_review_ref, start_review, GitProvider, ReviewEntryKind,
    ReviewProvider,
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
fn review_branch_records_notes_and_ordinary_project_commits_as_suggestions() {
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

    git(
        &repository,
        &["commit", "--quiet", "-m", "Make the behavior explicit."],
    );
    let suggestion = load_review(&repository).unwrap().entries[1].clone();
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
fn loading_a_review_rejects_suggestions_containing_nota_files() {
    let (_temp, repository, _) = project();
    start_review(&GitProvider::new(&repository), "HEAD", Some("nota/invalid")).unwrap();
    git(&repository, &["switch", "--quiet", "nota/invalid"]);
    std::fs::create_dir_all(repository.join(".nota")).unwrap();
    std::fs::write(repository.join(".nota/metadata"), "invalid\n").unwrap();
    git(&repository, &["add", "--force", ".nota/metadata"]);
    git(
        &repository,
        &["commit", "--quiet", "-m", "invalid suggestion"],
    );

    let error = load_review(&repository).unwrap_err();
    assert!(format!("{error:#}").contains("may not contain Nota files"));
}

#[test]
fn loading_a_review_rejects_empty_suggestion_commits() {
    let (_temp, repository, _) = project();
    start_review(&GitProvider::new(&repository), "HEAD", Some("nota/empty")).unwrap();
    git(&repository, &["switch", "--quiet", "nota/empty"]);
    git(
        &repository,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "empty suggestion",
        ],
    );

    let error = load_review(&repository).unwrap_err();
    assert!(format!("{error:#}").contains("has no changed project files"));
}

#[test]
fn loading_a_review_rejects_suggestions_without_a_comment() {
    let (_temp, repository, _) = project();
    start_review(
        &GitProvider::new(&repository),
        "HEAD",
        Some("nota/no-comment"),
    )
    .unwrap();
    git(&repository, &["switch", "--quiet", "nota/no-comment"]);
    std::fs::write(repository.join("suggested.txt"), "suggested\n").unwrap();
    git(&repository, &["add", "suggested.txt"]);
    git(
        &repository,
        &["commit", "--quiet", "--allow-empty-message", "-m", ""],
    );

    let error = load_review(&repository).unwrap_err();
    assert!(format!("{error:#}").contains("has an empty review comment"));
}
