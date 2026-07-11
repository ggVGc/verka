use std::fs;
use std::path::Path;

#[test]
fn core_has_no_execution_or_review_dependencies_or_vocabulary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("crates/llaundry-core/Cargo.toml")).unwrap();
    assert!(!manifest.contains("llaundry-work"));
    assert!(!manifest.contains("llaundry-review"));

    let source = fs::read_to_string(root.join("crates/llaundry-core/src/lib.rs")).unwrap();
    for forbidden in [
        "AttemptMeta",
        "worktree",
        "candidate_branch",
        "ReviewDecision",
        "publication_pending",
        "integrated_commit",
    ] {
        assert!(
            !source.contains(forbidden),
            "core contains application vocabulary `{forbidden}`"
        );
    }
}

#[test]
fn application_crates_depend_only_inward() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let work = fs::read_to_string(root.join("crates/llaundry-work/Cargo.toml")).unwrap();
    let review = fs::read_to_string(root.join("crates/llaundry-review/Cargo.toml")).unwrap();
    assert!(work.contains("llaundry-core"));
    assert!(review.contains("llaundry-core"));
    assert!(!work.contains("llaundry-review"));
    assert!(!review.contains("llaundry-work"));
}
