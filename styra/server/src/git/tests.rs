use super::*;

#[test]
fn finds_the_checkout_from_a_nested_directory() {
    let root = temporary_directory("nested");
    let nested = root.join("one/two");
    std::fs::create_dir_all(&nested).unwrap();
    let git = FakeGit::new();
    git.init(&root);

    let repository = git.discover(&nested).unwrap().unwrap();
    assert_eq!(repository.root, root.canonicalize().unwrap());
    assert_eq!(
        repository.common_dir,
        root.join(".git").canonicalize().unwrap()
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_directory_outside_git_is_not_a_repository() {
    let root = temporary_directory("plain");
    std::fs::create_dir_all(&root).unwrap();
    let git = FakeGit::new();

    assert_eq!(git.discover(&root).unwrap(), None);
    let error = git.repository_root(&root).unwrap_err().to_string();
    assert!(error.contains("not inside a Git repository"), "{error}");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn linked_worktrees_report_the_shared_common_directory() {
    let root = temporary_directory("linked");
    let checkout = root.join("checkout");
    let linked = root.join("linked");
    let git = FakeGit::new();
    git.init(&checkout);
    git.create_worktree(&checkout, "feature", &linked).unwrap();

    let repository = git.discover(&linked).unwrap().unwrap();
    assert_eq!(repository.root, linked.canonicalize().unwrap());
    assert_eq!(
        repository.common_dir,
        checkout.join(".git").canonicalize().unwrap()
    );
    // The two mechanisms answer the same question for different callers,
    // so what the layout reader reports has to be what the Git seam
    // reports — which is why the fake writes the layout it describes.
    assert_eq!(
        enclosing_root(&linked.join("nested")),
        Some(linked.canonicalize().unwrap())
    );
    assert!(history_directories(&linked).contains(&repository.common_dir));

    std::fs::remove_dir_all(root).unwrap();
}

/// An ordinary checkout mounts its root read-only and the one directory
/// holding its history writable.
#[test]
fn a_plain_checkout_mounts_its_root_and_its_history() {
    let root = temporary_directory("mounts-plain");
    let git = FakeGit::new();
    let repository = git.init(&root);

    let mounts = git.mounts(&root).unwrap();

    let writable: Vec<_> = mounts
        .iter()
        .filter(|mount| mount.writable)
        .map(|mount| mount.destination.clone())
        .collect();
    assert_eq!(writable, vec![repository.common_dir.clone()]);
    assert!(mounts
        .iter()
        .any(|mount| mount.destination == repository.root && !mount.writable));
    // The checkout root is also the common directory's parent: bind it once.
    let mut destinations: Vec<_> = mounts.iter().map(|mount| &mount.destination).collect();
    let total = destinations.len();
    destinations.sort();
    destinations.dedup();
    assert_eq!(destinations.len(), total, "duplicate mount: {mounts:?}");
    // Every mount is its own source: these are host paths bound in place.
    assert!(mounts.iter().all(|mount| mount.source == mount.destination));

    std::fs::remove_dir_all(root).unwrap();
}

/// A linked worktree keeps its index and HEAD in a per-worktree directory
/// and its objects in the shared one. Both have to arrive writable, or the
/// agent gets a checkout it cannot commit in — but they arrive as *one*
/// mount, because Git nests the first inside the second. What the test
/// pins is the reachability, not the number of mounts that deliver it.
#[test]
fn a_linked_worktree_can_write_both_its_own_metadata_and_the_shared_history() {
    let base = temporary_directory("mounts-linked");
    let checkout = base.join("checkout");
    let linked = base.join("linked");
    let git = FakeGit::new();
    let main = git.init(&checkout);
    git.create_worktree(&checkout, "feature", &linked).unwrap();
    let worktree_git = git.directories(&linked).unwrap().git_dir;

    let mounts = git.mounts(&linked).unwrap();

    let writable_cover = |path: &Path| {
        mounts
            .iter()
            .any(|mount| mount.writable && path.starts_with(&mount.destination))
    };
    assert!(
        writable_cover(&main.common_dir),
        "the shared history must be writable: {mounts:?}"
    );
    assert!(
        writable_cover(&worktree_git),
        "the worktree's own metadata must be writable: {mounts:?}"
    );
    // The checkout itself is not: the agent edits files through the
    // Workspace mount, and nothing should be able to rewrite it here.
    assert!(mounts
        .iter()
        .any(|mount| mount.destination == linked.canonicalize().unwrap() && !mount.writable));

    std::fs::remove_dir_all(base).unwrap();
}

/// The other shape: a `git_dir` that is not inside the common directory,
/// which is the case the mount set has an explicit branch for. Nesting is
/// what makes the linked-worktree case one mount; without it, both
/// directories have to be named.
#[test]
fn a_git_dir_outside_the_common_directory_is_mounted_in_its_own_right() {
    let base = temporary_directory("mounts-separate");
    let root = base.join("checkout");
    let git_dir = base.join("elsewhere/git");
    std::fs::create_dir_all(&git_dir).unwrap();
    let git = FakeGit::new();
    git.init(&root);
    let common_dir = root.join(".git").canonicalize().unwrap();
    git.relocate_git_dir(&root, &git_dir.canonicalize().unwrap());

    let mounts = git.mounts(&root).unwrap();

    let writable: Vec<_> = mounts
        .iter()
        .filter(|mount| mount.writable)
        .map(|mount| mount.destination.clone())
        .collect();
    assert!(writable.contains(&common_dir), "{writable:?}");
    assert!(
        writable.contains(&git_dir.canonicalize().unwrap()),
        "{writable:?}"
    );

    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn the_enclosing_root_is_the_nearest_directory_holding_a_git() {
    let base = temporary_directory("enclosing");
    let root = base.join("checkout");
    let nested = root.join("one/two");
    std::fs::create_dir_all(nested.join(".git-not-this-one")).unwrap();
    let git = FakeGit::new();
    git.init(&root);

    assert_eq!(enclosing_root(&nested), Some(root));
    // A sibling outside the checkout does not borrow its root. Whether
    // anything above the temporary directory is itself a checkout is not
    // this test's business, so the claim is only about the tree it built.
    let outside = base.join("plain/deeper");
    std::fs::create_dir_all(&outside).unwrap();
    assert!(!matches!(enclosing_root(&outside), Some(found) if found.starts_with(&base)));

    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_worktree_contributes_the_directories_holding_its_history() {
    let base = temporary_directory("history");
    let main = base.join("main");
    let worktree_git = main.join(".git/worktrees/feature");
    let worktree = base.join("feature");
    std::fs::create_dir_all(&worktree_git).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../main/.git/worktrees/feature\n",
    )
    .unwrap();
    std::fs::write(worktree_git.join("commondir"), "../..\n").unwrap();

    let directories = history_directories(&worktree);

    assert_eq!(
        directories,
        vec![
            std::fs::canonicalize(&worktree_git).unwrap(),
            std::fs::canonicalize(main.join(".git")).unwrap(),
        ]
    );
    assert!(history_directories(&main).is_empty());
    std::fs::remove_dir_all(base).unwrap();
}

/// The fake refuses what Git refuses, so a caller cannot pass a test by
/// doing something the real thing would reject.
#[test]
fn the_fake_refuses_a_branch_that_already_exists() {
    let base = temporary_directory("duplicate-branch");
    let checkout = base.join("checkout");
    let git = FakeGit::new();
    git.init(&checkout);
    git.create_worktree(&checkout, "feature", &base.join("one"))
        .unwrap();

    let error = git
        .create_worktree(&checkout, "feature", &base.join("two"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("already exists"), "{error}");
    std::fs::remove_dir_all(base).unwrap();
}
