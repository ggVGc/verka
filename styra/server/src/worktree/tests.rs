use super::*;
use crate::git::FakeGit;

fn temporary_directory(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "styra-worktrees-{tag}-{}-{}",
        std::process::id(),
        crate::journal::now_ms()
    ))
}

/// A Workspace on a repository, with the Git that backs it. No process and
/// no history: what these tests are about is which checkout an interaction
/// gets and what it is called, and neither depends on there being commits.
fn workspace(tag: &str) -> (PathBuf, Arc<FakeGit>, Worktrees) {
    let root = temporary_directory(tag);
    let git = Arc::new(FakeGit::new());
    let repository = git.init(&root.join("checkout"));
    let worktrees =
        Worktrees::prepare(git.clone(), repository, root.join("state/worktrees")).unwrap();
    (root, git, worktrees)
}

#[test]
fn an_interaction_gets_a_branch_and_checkout_of_its_own() {
    let (root, git, worktrees) = workspace("create");
    let host_root = root.join("state/worktrees");

    let checkout = worktrees
        .checkout("1757000000000-1-0", None)
        .unwrap()
        .path
        .unwrap();

    assert_eq!(checkout, host_root.join("1757000000000-1-0"));
    assert_eq!(
        git.current_branch(&checkout).unwrap().as_deref(),
        Some("styra/1757000000000-1-0")
    );
    assert!(checkout.join(".git").is_file());
    let metadata = worktrees.metadata_mount();
    let common_dir = root.join("checkout/.git").canonicalize().unwrap();
    assert_eq!(metadata.source, common_dir);
    assert_eq!(metadata.destination, common_dir);
    assert!(metadata.writable);

    std::fs::remove_dir_all(root).unwrap();
}

/// A new checkout says where its branch came from: the branch the
/// repository had checked out, at the commit it was at then — which stays
/// the answer after that branch moves on.
#[test]
fn a_new_checkout_records_the_branch_and_commit_it_was_made_from() {
    let (root, git, worktrees) = workspace("branched-from");
    let start = git.commit(&root.join("checkout"), "HEAD").unwrap().unwrap();

    let checkout = worktrees
        .checkout("1757000000000-1-8", Some("record-the-origin"))
        .unwrap();
    let moved_on = git.commit_on("main");

    assert_eq!(
        checkout.branched_from,
        Some(BranchPoint {
            branch: Some("main".to_owned()),
            commit: start.clone(),
        })
    );
    assert_ne!(moved_on, start);
    assert_eq!(
        git.commit(checkout.path.as_deref().unwrap(), "HEAD")
            .unwrap(),
        Some(start),
        "the branch starts where the record says it does"
    );
    // Asking again finds the checkout and does not invent its origin.
    assert_eq!(
        worktrees
            .checkout("1757000000000-1-8", None)
            .unwrap()
            .branched_from,
        None
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// A Session branched from another records the source's branch, and the
/// commit that branch was at when the two diverged.
#[test]
fn a_forked_checkout_records_the_branch_it_was_forked_from() {
    let (root, git, worktrees) = workspace("forked-from");
    let source = worktrees
        .checkout("1757000000000-1-9", Some("record-the-origin"))
        .unwrap();
    let source_commit = git.commit_on(&source.branch);

    let forked = worktrees
        .fork(
            "1757000000000-2-0",
            Some("record-the-origin"),
            &source.branch,
        )
        .unwrap();

    assert_eq!(
        forked.branched_from,
        Some(BranchPoint {
            branch: Some(source.branch.clone()),
            commit: source_commit,
        })
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// A Session branched from another one gets a checkout of its own, on a
/// branch of the source's rather than of the repository's own head — and
/// under the source's topic, because it is about the same work.
#[test]
fn a_branched_interaction_forks_the_checkout_it_came_from() {
    let (root, git, worktrees) = workspace("fork");
    let host_root = root.join("state/worktrees");
    let source = worktrees
        .checkout("1757000000000-1-0", Some("teach-the-picker-to-filter"))
        .unwrap()
        .path
        .unwrap();
    let topic = topic_of(&source, "1757000000000-1-0");
    assert_eq!(topic.as_deref(), Some("teach-the-picker-to-filter"));

    let forked = worktrees
        .fork(
            "1757000000000-1-1",
            topic.as_deref(),
            "styra/teach-the-picker-to-filter-1757000000000-1-0",
        )
        .unwrap()
        .path
        .unwrap();

    assert_eq!(
        forked,
        host_root.join("teach-the-picker-to-filter-1757000000000-1-1")
    );
    assert_ne!(forked, source, "a branch works apart from its source");
    assert_eq!(
        git.current_branch(&forked).unwrap().as_deref(),
        Some("styra/teach-the-picker-to-filter-1757000000000-1-1")
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// A checkout named by id alone — an interaction that never had a topic —
/// passes none on, rather than handing its sibling half an id as a name.
#[test]
fn a_checkout_named_by_id_alone_has_no_topic() {
    assert_eq!(
        topic_of(
            Path::new("/state/worktrees/1757000000000-1-0"),
            "1757000000000-1-0"
        ),
        None
    );
}

/// Resuming an interaction asks for its checkout again. The uncommitted
/// file stands in for the work a resumed agent expects to find.
#[test]
fn asking_twice_returns_the_same_checkout() {
    let (root, _git, worktrees) = workspace("resume");

    let first = worktrees
        .checkout("1757000000000-1-1", Some("teach-the-picker-to-filter"))
        .unwrap()
        .path
        .unwrap();
    std::fs::write(first.join("in-progress.txt"), "half-done").unwrap();
    // A resume knows only the id, and the topic is not repeated to it.
    let second = worktrees
        .checkout("1757000000000-1-1", None)
        .unwrap()
        .path
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(
        std::fs::read_to_string(second.join("in-progress.txt")).unwrap(),
        "half-done"
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// Two interactions in one Workspace share a repository and nothing else:
/// separate branches, separate indexes, separate files.
#[test]
fn two_interactions_do_not_share_a_checkout() {
    let (root, git, worktrees) = workspace("parallel");

    // Two Sessions given the same task are named the same thing, and the
    // id each carries is what keeps their branches apart.
    let one = worktrees
        .checkout("1757000000000-1-2", Some("fix-the-flaky-test"))
        .unwrap()
        .path
        .unwrap();
    let two = worktrees
        .checkout("1757000000000-1-3", Some("fix-the-flaky-test"))
        .unwrap()
        .path
        .unwrap();

    assert_ne!(one, two);
    assert_ne!(
        git.current_branch(&one).unwrap(),
        git.current_branch(&two).unwrap()
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// What the operator sees in `git branch` and in their worktree directory
/// is the work, with the interaction it belongs to after it.
#[test]
fn a_named_interaction_gets_a_branch_that_says_what_it_is_for() {
    let (root, git, worktrees) = workspace("named");
    let host_root = root.join("state/worktrees");

    let checkout = worktrees
        .checkout("1757000000000-1-4", Some("fix-flaky-checkout-test"))
        .unwrap()
        .path
        .unwrap();

    assert_eq!(
        checkout,
        host_root.join("fix-flaky-checkout-test-1757000000000-1-4")
    );
    assert_eq!(
        git.current_branch(&checkout).unwrap().as_deref(),
        Some("styra/fix-flaky-checkout-test-1757000000000-1-4")
    );
    // And the Session finds it again from the id alone, which is all a
    // resume or a plan has.
    assert_eq!(worktrees.path("1757000000000-1-4"), checkout);

    std::fs::remove_dir_all(root).unwrap();
}

/// Cleaning up after a finished interaction takes the directory and
/// leaves the branch, and returning to it is the branch being checked out
/// where it was — under the name the operator has already seen in their
/// worktree directory and in `git branch`.
#[test]
fn a_removed_checkout_comes_back_on_the_branch_it_left_behind() {
    let (root, git, worktrees) = workspace("restore");

    let checkout = worktrees
        .checkout("1757000000000-1-6", Some("tidy-the-worktrees"))
        .unwrap()
        .path
        .unwrap();
    let branch = git.current_branch(&checkout).unwrap().unwrap();
    worktrees.remove(&checkout).unwrap();

    assert!(!checkout.exists());
    assert!(git.has_branch(&branch), "the branch went with the checkout");
    assert_eq!(worktrees.path_for_branch(&branch), checkout);

    let restored = worktrees.restore(&branch).unwrap();

    assert_eq!(restored, checkout);
    assert_eq!(git.current_branch(&restored).unwrap(), Some(branch));

    std::fs::remove_dir_all(root).unwrap();
}

/// Asking for a checkout that is already there is what a second cleanup
/// pass, or a resume racing one, has to be safe to do.
#[test]
fn restoring_a_checkout_that_is_still_there_returns_it() {
    let (root, git, worktrees) = workspace("restore-existing");

    let checkout = worktrees
        .checkout("1757000000000-1-7", None)
        .unwrap()
        .path
        .unwrap();
    let branch = git.current_branch(&checkout).unwrap().unwrap();
    std::fs::write(checkout.join("in-progress.txt"), "half-done").unwrap();

    assert_eq!(worktrees.restore(&branch).unwrap(), checkout);
    assert!(checkout.join("in-progress.txt").exists());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_planned_checkout_is_named_without_being_created() {
    let (root, _git, worktrees) = workspace("planned");
    let host_root = root.join("state/worktrees");

    assert_eq!(worktrees.path("<pending>"), host_root.join("<pending>"));
    assert!(!host_root.join("<pending>").exists());

    std::fs::remove_dir_all(root).unwrap();
}

/// The branch name is the operator's view of the work, so the prefix that
/// marks it as Styra's is part of the contract, not decoration.
#[test]
fn every_branch_styra_creates_is_under_its_own_prefix() {
    let (root, git, worktrees) = workspace("prefix");

    let checkout = worktrees
        .checkout("1757000000000-1-5", Some("rename-the-thing"))
        .unwrap()
        .path
        .unwrap();

    let branch = git.current_branch(&checkout).unwrap().unwrap();
    assert!(
        branch.starts_with(&format!("{BRANCH_PREFIX}/")),
        "{branch} is not recognisable as Styra's"
    );

    std::fs::remove_dir_all(root).unwrap();
}
