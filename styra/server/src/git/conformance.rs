use super::*;

/// The working-tree question as `git` actually answers it, including the
/// part Styra depends on that a status summary could plausibly omit: an
/// untracked file counts as uncommitted work.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_reports_uncommitted_work_in_a_checkout() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let root = temporary_directory("conformance-status");
    std::fs::create_dir_all(&root).unwrap();
    fixture::init(&root);
    fixture::commit_empty(&root, "root");

    assert!(!SystemGit.has_uncommitted_changes(&root).unwrap());

    std::fs::write(root.join("left-behind.txt"), "work").unwrap();
    assert!(SystemGit.has_uncommitted_changes(&root).unwrap());

    std::fs::remove_dir_all(root).unwrap();
}

/// A turn's commit takes everything `git status` reported — the new file
/// as well as the edit — and keeps the message as written, Markdown
/// headings included.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_commits_all_of_a_turns_work() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let root = temporary_directory("conformance-commit");
    std::fs::create_dir_all(&root).unwrap();
    fixture::init(&root);
    fixture::identify(&root);
    fixture::commit_empty(&root, "root");
    std::fs::write(root.join("new.txt"), "work").unwrap();

    let message = "Add the file\n\n# Heading\nbody";
    let commit = SystemGit.commit_all(&root, message).unwrap();

    assert!(!commit.is_empty());
    assert!(!SystemGit.has_uncommitted_changes(&root).unwrap());
    let logged = Invocation::new(&root, "read the commit message")
        .args(["log", "-1", "--format=%B"])
        .output()
        .unwrap();
    assert_eq!(logged.trim_end(), message);

    std::fs::remove_dir_all(root).unwrap();
}

/// A snapshot's diff takes the edit, the untracked file and the binary
/// one, applies back onto the commit it was taken against, and leaves
/// what the operator had staged as it was.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_diffs_the_whole_working_tree_without_touching_the_index() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let root = temporary_directory("conformance-snapshot");
    std::fs::create_dir_all(&root).unwrap();
    fixture::init(&root);
    fixture::identify(&root);
    std::fs::write(root.join("tracked.txt"), "before\n").unwrap();
    SystemGit.commit_all(&root, "root").unwrap();

    std::fs::write(root.join("tracked.txt"), "after\n").unwrap();
    std::fs::write(root.join("untracked.txt"), "new\n").unwrap();
    std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150, 255]).unwrap();
    Invocation::new(&root, "stage one file")
        .args(["add", "tracked.txt"])
        .succeed()
        .unwrap();
    let staged = || {
        Invocation::new(&root, "read the staged files")
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap()
    };
    let before = staged();

    let diff = SystemGit.working_tree_diff(&root).unwrap();
    let text = String::from_utf8_lossy(&diff);
    for file in ["tracked.txt", "untracked.txt", "blob.bin"] {
        assert!(
            text.contains(&format!("b/{file}")),
            "missing {file}: {text}"
        );
    }
    assert!(text.contains("GIT binary patch"), "{text}");
    assert_eq!(staged(), before, "the operator's index is untouched");

    Invocation::new(&root, "discard the work")
        .args(["reset", "--hard", "--quiet"])
        .succeed()
        .unwrap();
    Invocation::new(&root, "remove untracked files")
        .args(["clean", "-fdq"])
        .succeed()
        .unwrap();
    let patch = root.join(".git/snapshot.diff");
    std::fs::write(&patch, &diff).unwrap();
    Invocation::new(&root, "apply the snapshot")
        .args(["apply", "--binary"])
        .arg(&patch)
        .succeed()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).unwrap(),
        "after\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("untracked.txt")).unwrap(),
        "new\n"
    );
    assert_eq!(
        std::fs::read(root.join("blob.bin")).unwrap(),
        [0u8, 159, 146, 150, 255]
    );

    std::fs::remove_dir_all(root).unwrap();
}

/// A snapshot's tree is rebuilt from its commit and diff — from a
/// subdirectory, with the diff naming paths outside it — and two of them
/// diff to exactly what changed between.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_rebuilds_snapshot_trees_and_diffs_them() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let root = temporary_directory("conformance-snapshot-trees");
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    fixture::init(&root);
    fixture::identify(&root);
    std::fs::write(root.join("kept.txt"), "kept\n").unwrap();
    std::fs::write(nested.join("inner.txt"), "inner\n").unwrap();
    SystemGit.commit_all(&root, "root").unwrap();
    let commit = SystemGit.commit(&root, "HEAD").unwrap();

    std::fs::write(root.join("kept.txt"), "before the turn\n").unwrap();
    let before = SystemGit.working_tree_diff(&root).unwrap();
    std::fs::write(root.join("added.txt"), "by the turn\n").unwrap();
    let after = SystemGit.working_tree_diff(&root).unwrap();

    let from = SystemGit
        .snapshot_tree(&nested, commit.as_deref(), Some(&before))
        .unwrap();
    let to = SystemGit
        .snapshot_tree(&nested, commit.as_deref(), Some(&after))
        .unwrap();
    let clean = SystemGit
        .snapshot_tree(&nested, commit.as_deref(), None)
        .unwrap();
    assert_ne!(from, clean, "the diff was applied, paths outside included");

    let diff = SystemGit.tree_diff(&nested, &from, &to).unwrap();
    assert!(diff.contains("+by the turn"), "{diff}");
    assert!(!diff.contains("before the turn"), "{diff}");
    assert_eq!(SystemGit.tree_diff(&nested, &to, &to).unwrap(), "");

    std::fs::remove_dir_all(root).unwrap();
}

/// On a branch with no commits yet the whole tree is the diff.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_diffs_an_unborn_branch_as_additions() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let root = temporary_directory("conformance-snapshot-unborn");
    std::fs::create_dir_all(&root).unwrap();
    fixture::init(&root);
    std::fs::write(root.join("first.txt"), "hello\n").unwrap();

    assert_eq!(SystemGit.commit(&root, "HEAD").unwrap(), None);
    let diff = String::from_utf8(SystemGit.working_tree_diff(&root).unwrap()).unwrap();
    assert!(diff.contains("new file mode"), "{diff}");
    assert!(diff.contains("+hello"), "{diff}");

    std::fs::remove_dir_all(root).unwrap();
}

/// Both implementations must answer a nested lookup the same way.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_describes_a_plain_checkout_as_the_fake_does() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let root = temporary_directory("conformance-plain");
    let nested = root.join("one/two");
    std::fs::create_dir_all(&nested).unwrap();
    fixture::init(&root);

    let real = SystemGit.discover(&nested).unwrap().unwrap();
    assert_eq!(real.root, root.canonicalize().unwrap());
    assert_eq!(real.common_dir, root.join(".git").canonicalize().unwrap());
    assert_eq!(
        SystemGit.directories(&real.root).unwrap(),
        Directories {
            git_dir: real.common_dir.clone(),
            common_dir: real.common_dir.clone(),
        }
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires a real git binary"]
fn real_git_is_silent_about_a_directory_outside_a_repository() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let root = temporary_directory("conformance-outside");
    std::fs::create_dir_all(&root).unwrap();

    assert_eq!(SystemGit.discover(&root).unwrap(), None);

    std::fs::remove_dir_all(root).unwrap();
}

/// Removing a worktree is the half of the cleanup that has to be exactly
/// Git's: the directory goes, the branch stays, and the branch can be
/// checked out again where it was. A fake that let the branch go with the
/// checkout would make every cleanup test pass while the real thing
/// refused to create the branch a second time.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_keeps_the_branch_when_the_worktree_goes_as_the_fake_does() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let base = temporary_directory("conformance-remove");
    let checkout = base.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    fixture::init(&checkout);
    fixture::commit_empty(&checkout, "initial");
    let real_linked = base.join("real");
    SystemGit
        .create_worktree(&checkout, "styra/removed", &real_linked)
        .unwrap();

    let fake = FakeGit::new();
    let fake_checkout = base.join("fake-checkout");
    fake.init(&fake_checkout);
    let fake_linked = base.join("fake");
    fake.create_worktree(&fake_checkout, "styra/removed", &fake_linked)
        .unwrap();

    SystemGit.remove_worktree(&checkout, &real_linked).unwrap();
    fake.remove_worktree(&fake_checkout, &fake_linked).unwrap();

    // The directory is gone in both.
    assert!(!real_linked.exists());
    assert!(!fake_linked.exists());
    // The branch is not.
    let listed = Invocation::new(&checkout, "list the branches")
        .args(["branch", "--list", "styra/removed"])
        .output()
        .unwrap();
    assert!(!listed.is_empty(), "the branch went with the worktree");
    assert!(fake.has_branch("styra/removed"));
    // And creating it again is refused in both, because it is still there.
    assert!(SystemGit
        .create_worktree(&checkout, "styra/removed", &real_linked)
        .is_err());
    assert!(fake
        .create_worktree(&fake_checkout, "styra/removed", &fake_linked)
        .is_err());
    // Checking it out again is how a cleaned-up Session comes back, and
    // the work it committed comes back with it.
    SystemGit
        .add_worktree(&checkout, "styra/removed", &real_linked)
        .unwrap();
    fake.add_worktree(&fake_checkout, "styra/removed", &fake_linked)
        .unwrap();
    assert_eq!(
        SystemGit.current_branch(&real_linked).unwrap().as_deref(),
        Some("styra/removed")
    );
    assert_eq!(
        fake.current_branch(&fake_linked).unwrap().as_deref(),
        Some("styra/removed")
    );

    std::fs::remove_dir_all(base).unwrap();
}

/// A worktree with work in it is not something either implementation
/// deletes, whatever the caller thought it knew.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_refuses_to_remove_a_dirty_worktree_as_the_fake_does() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let base = temporary_directory("conformance-remove-dirty");
    let checkout = base.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    fixture::init(&checkout);
    fixture::commit_empty(&checkout, "initial");
    let real_linked = base.join("real");
    SystemGit
        .create_worktree(&checkout, "styra/dirty", &real_linked)
        .unwrap();
    std::fs::write(real_linked.join("left-behind.txt"), "half-done").unwrap();

    let fake = FakeGit::new();
    let fake_checkout = base.join("fake-checkout");
    fake.init(&fake_checkout);
    let fake_linked = base.join("fake");
    fake.create_worktree(&fake_checkout, "styra/dirty", &fake_linked)
        .unwrap();
    fake.set_uncommitted_changes(&fake_linked, true);

    assert!(SystemGit.remove_worktree(&checkout, &real_linked).is_err());
    assert!(fake.remove_worktree(&fake_checkout, &fake_linked).is_err());
    assert!(real_linked.join("left-behind.txt").exists());
    assert!(fake_linked.exists());

    std::fs::remove_dir_all(base).unwrap();
}

/// A commit is named by a branch or by itself, a branch made at one starts
/// there, and a head with nothing committed names none — in both.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_resolves_commits_as_the_fake_does() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let base = temporary_directory("conformance-commit");
    let checkout = base.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    fixture::init(&checkout);
    assert_eq!(SystemGit.commit(&checkout, "HEAD").unwrap(), None);
    fixture::commit_empty(&checkout, "initial");
    let branch = SystemGit.current_branch(&checkout).unwrap().unwrap();
    let head = SystemGit.commit(&checkout, "HEAD").unwrap().unwrap();
    assert_eq!(head.len(), 40);
    assert_eq!(
        SystemGit.commit(&checkout, &branch).unwrap(),
        Some(head.clone())
    );
    assert_eq!(SystemGit.commit(&checkout, "no-such-branch").unwrap(), None);
    let real_linked = base.join("real");
    SystemGit
        .fork_worktree(&checkout, "styra/pinned", &real_linked, &head)
        .unwrap();
    assert_eq!(SystemGit.commit(&real_linked, "HEAD").unwrap(), Some(head));

    let fake = FakeGit::new();
    let fake_checkout = base.join("fake-checkout");
    fake.init(&fake_checkout);
    let fake_head = fake.commit(&fake_checkout, "HEAD").unwrap().unwrap();
    assert_eq!(fake_head.len(), 40);
    assert_eq!(
        fake.commit(&fake_checkout, "main").unwrap(),
        Some(fake_head.clone())
    );
    assert_eq!(fake.commit(&fake_checkout, "no-such-branch").unwrap(), None);
    let fake_linked = base.join("fake");
    fake.fork_worktree(&fake_checkout, "styra/pinned", &fake_linked, &fake_head)
        .unwrap();
    assert_eq!(fake.commit(&fake_linked, "HEAD").unwrap(), Some(fake_head));

    std::fs::remove_dir_all(base).unwrap();
}

/// The shape [`FakeGit::create_worktree`] writes is the shape `git
/// worktree add` writes: a `.git` file, a per-worktree metadata directory
/// under the main checkout, and a shared common directory. If this drifts,
/// every test built on the fake is testing the wrong layout.
#[test]
#[ignore = "requires a real git binary"]
fn real_git_lays_a_worktree_out_the_way_the_fake_does() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let base = temporary_directory("conformance-worktree");
    let checkout = base.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    fixture::init(&checkout);
    fixture::commit_empty(&checkout, "initial");
    let real_linked = base.join("real");
    SystemGit
        .create_worktree(&checkout, "styra/conformance", &real_linked)
        .unwrap();

    let fake = FakeGit::new();
    let fake_checkout = base.join("fake-checkout");
    fake.init(&fake_checkout);
    let fake_linked = base.join("fake");
    fake.create_worktree(&fake_checkout, "styra/conformance", &fake_linked)
        .unwrap();

    // A pointer file, not a directory, in both.
    assert!(real_linked.join(".git").is_file());
    assert!(fake_linked.join(".git").is_file());
    // The metadata directory sits under the main checkout's .git in both.
    let real_dirs = SystemGit.directories(&real_linked).unwrap();
    let fake_dirs = fake.directories(&fake_linked).unwrap();
    assert!(real_dirs
        .git_dir
        .starts_with(checkout.canonicalize().unwrap().join(".git")));
    assert!(fake_dirs
        .git_dir
        .starts_with(fake_checkout.canonicalize().unwrap().join(".git")));
    assert_ne!(real_dirs.git_dir, real_dirs.common_dir);
    assert_ne!(fake_dirs.git_dir, fake_dirs.common_dir);
    // And the layout reader, which parses those files rather than asking,
    // finds the common directory through both.
    assert!(history_directories(&real_linked).contains(&real_dirs.common_dir));
    assert!(history_directories(&fake_linked).contains(&fake_dirs.common_dir));
    // The branch reads back in both.
    assert_eq!(
        SystemGit.current_branch(&real_linked).unwrap().as_deref(),
        Some("styra/conformance")
    );
    assert_eq!(
        fake.current_branch(&fake_linked).unwrap().as_deref(),
        Some("styra/conformance")
    );

    std::fs::remove_dir_all(base).unwrap();
}

/// The mount set is derived logic, so it must come out the same whichever
/// implementation answered the questions underneath it.
#[test]
#[ignore = "requires a real git binary"]
fn both_implementations_derive_the_same_shape_of_mount_set() {
    if !fixture::git_available() {
        eprintln!("skipping: no usable git");
        return;
    }
    let base = temporary_directory("conformance-mounts");
    let checkout = base.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    fixture::init(&checkout);

    let fake = FakeGit::new();
    let fake_checkout = base.join("fake-checkout");
    fake.init(&fake_checkout);

    let shape = |mounts: Vec<MountSpec>, root: &Path| {
        let root = root.canonicalize().unwrap();
        let mut relative: Vec<_> = mounts
            .iter()
            .map(|mount| {
                (
                    mount
                        .destination
                        .strip_prefix(&root)
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|_| mount.destination.display().to_string()),
                    mount.writable,
                )
            })
            .collect();
        relative.sort();
        relative
    };

    assert_eq!(
        shape(SystemGit.mounts(&checkout).unwrap(), &checkout),
        shape(fake.mounts(&fake_checkout).unwrap(), &fake_checkout)
    );

    std::fs::remove_dir_all(base).unwrap();
}
