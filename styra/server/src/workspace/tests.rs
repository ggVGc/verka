use super::*;
use crate::git::FakeGit;

fn temp_dir(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("styra-workspace-{tag}-{}", new_id(now_ms())));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn deletion_hides_and_blocks_a_workspace_until_restored_without_removing_files() {
    let store = temp_dir("delete-store");
    let host = temp_dir("delete-host");
    let workspace = create(&store, &host, Some("preserved".into())).unwrap();
    let session = workspace.path.join("sessions/session-1");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("journal.jsonl"), "preserved history").unwrap();
    let worktrees = workspace.path.join("worktrees/checkout-1");
    std::fs::create_dir_all(&worktrees).unwrap();
    std::fs::write(worktrees.join("uncommitted"), "preserved changes").unwrap();
    change_launch(
        &store,
        &workspace.id,
        WorkspaceLaunchChange::SetNetwork(Some(true)),
    )
    .unwrap();

    delete(&store, &workspace.id).unwrap();
    assert!(list(&store).unwrap().is_empty());
    assert!(for_path(&store, &host).unwrap().is_none());
    assert_eq!(list_deleted(&store).unwrap()[0].id, workspace.id);
    assert!(get(&store, &workspace.id).is_err());
    assert!(access(&store, &workspace.id).is_err());
    assert!(rename(&store, &workspace.id, Some("hidden")).is_err());
    assert!(set_host_path(&store, &workspace.id, &host).is_err());
    assert!(set_git_repository(&FakeGit::new(), &store, &workspace.id, None).is_err());
    assert!(launch(&store, &workspace.id).is_err());
    assert!(change_launch(
        &store,
        &workspace.id,
        WorkspaceLaunchChange::SetNetwork(None)
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(session.join("journal.jsonl")).unwrap(),
        "preserved history"
    );
    assert_eq!(
        std::fs::read_to_string(worktrees.join("uncommitted")).unwrap(),
        "preserved changes"
    );

    let restored = restore(&store, &workspace.id).unwrap();
    assert_eq!(restored.id, workspace.id);
    assert_eq!(restored.name, workspace.name);
    assert_eq!(restored.host_path, workspace.host_path);
    assert_eq!(restored.session_count, 1);
    assert_eq!(restored.launch.network, Some(true));
    assert!(list_deleted(&store).unwrap().is_empty());
    assert_eq!(for_path(&store, &host).unwrap().unwrap().id, workspace.id);
    assert!(restore(&store, &workspace.id).is_err());
    std::fs::remove_dir_all(store).unwrap();
    std::fs::remove_dir_all(host).unwrap();
}

#[test]
fn creates_and_lists_distinct_workspaces_for_the_same_host_path() {
    let store = temp_dir("store");
    let host = temp_dir("host");
    let first = create(&store, &host, Some("first".into())).unwrap();
    let second = create(&store, &host, Some("second".into())).unwrap();

    assert_ne!(first.id, second.id);
    assert_eq!(first.host_path, host.canonicalize().unwrap());
    assert!(first.path.join("workspace.json").is_file());
    assert!(first.path.join("sessions").is_dir());
    assert!(!first.path.join("worktrees").exists());

    let listed = list(&store).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, second.id);
    assert_eq!(
        get(&store, &first.id).unwrap().name.as_deref(),
        Some("first")
    );

    access(&store, &first.id).unwrap();
    let listed = list(&store).unwrap();
    assert_eq!(listed[0].id, first.id);
    assert_eq!(listed[1].id, second.id);

    std::fs::remove_dir_all(store).unwrap();
    std::fs::remove_dir_all(host).unwrap();
}

/// A stored policy is the Workspace's, so it has to survive everything
/// else that rewrites the metadata around it.
#[test]
fn a_stored_launch_policy_outlives_other_metadata_edits() {
    let store = temp_dir("launch-store");
    let host = temp_dir("launch-host");
    let workspace = create(&store, &host, None).unwrap();
    assert!(workspace.launch.is_empty());

    let launch = LaunchPolicy {
        network: Some(true),
        writable_workspace: None,
        templates: vec!["rust".into()],
        mounts: vec![crate::protocol::LaunchMount {
            source: PathBuf::from("/srv/corpus"),
            destination: Some(PathBuf::from("/mnt/corpus")),
            writable: false,
        }],
        // A Workspace policy has no layer below it to ignore, so this is
        // dropped rather than stored as a contradiction.
        ignore_workspace: true,
        run: Some("cargo run --release".into()),
        test: None,
    };
    let stored = change_launch(
        &store,
        &workspace.id,
        WorkspaceLaunchChange::Replace(launch.clone()),
    )
    .unwrap();
    assert!(!stored.ignore_workspace);
    assert_eq!(stored.templates, launch.templates);

    // Each field is edited on its own too, against the latest stored copy
    // rather than a client's cached one.
    let stored = change_launch(
        &store,
        &workspace.id,
        WorkspaceLaunchChange::SetWritableWorkspace(Some(false)),
    )
    .unwrap();
    assert!(!stored.grants_writable_workspace(true));
    assert_eq!(stored.templates, launch.templates, "the rest is untouched");

    let stored = change_launch(
        &store,
        &workspace.id,
        WorkspaceLaunchChange::SetCommand {
            action: crate::protocol::ProjectAction::Test,
            command: Some("cargo nextest run".into()),
        },
    )
    .unwrap();
    assert_eq!(stored.test.as_deref(), Some("cargo nextest run"));
    assert_eq!(stored.run, launch.run, "the other action is untouched");
    // A blank command is no command: the templates' default applies again.
    let stored = change_launch(
        &store,
        &workspace.id,
        WorkspaceLaunchChange::SetCommand {
            action: crate::protocol::ProjectAction::Test,
            command: Some("  ".into()),
        },
    )
    .unwrap();
    assert_eq!(stored.test, None);

    // The access bump the picker relies on rewrites `workspace.json`;
    // it is not allowed to drop the policy.
    access(&store, &workspace.id).unwrap();
    let reread = get(&store, &workspace.id).unwrap();
    assert_eq!(reread.launch.mounts, launch.mounts);
    assert_eq!(reread.launch.network, Some(true));

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

#[test]
fn renaming_a_workspace_persists_and_a_blank_name_restores_the_fallback() {
    let store = temp_dir("rename-store");
    let host = temp_dir("rename-host");
    let workspace = create(&store, &host, None).unwrap();

    let renamed = rename(&store, &workspace.id, Some(" payments ")).unwrap();
    assert_eq!(renamed.name.as_deref(), Some("payments"));
    assert_eq!(
        get(&store, &workspace.id).unwrap().name.as_deref(),
        Some("payments")
    );

    let cleared = rename(&store, &workspace.id, Some("   ")).unwrap();
    assert_eq!(cleared.name, None);
    assert_eq!(get(&store, &workspace.id).unwrap().name, None);

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

#[test]
fn moving_a_workspace_keeps_its_identity_and_finds_it_from_the_new_directory() {
    let store = temp_dir("move-store");
    let old_host = temp_dir("move-old-host");
    let new_host = temp_dir("move-new-host");
    let workspace = create(&store, &old_host, Some("moved".into())).unwrap();

    let moved = set_host_path(&store, &workspace.id, &new_host).unwrap();
    assert_eq!(moved.id, workspace.id);
    assert_eq!(moved.name.as_deref(), Some("moved"));
    assert_eq!(moved.host_path, new_host.canonicalize().unwrap());
    assert_eq!(
        get(&store, &workspace.id).unwrap().host_path,
        moved.host_path
    );
    assert_eq!(
        for_path(&store, &new_host).unwrap().map(|found| found.id),
        Some(workspace.id.clone())
    );
    assert!(for_path(&store, &old_host).unwrap().is_none());

    // A directory that is missing, relative, or a file is refused, and the
    // stored directory is left as it was.
    assert!(set_host_path(&store, &workspace.id, &new_host.join("missing")).is_err());
    assert!(set_host_path(&store, &workspace.id, Path::new("src")).is_err());
    let file = new_host.join("file");
    std::fs::write(&file, "").unwrap();
    assert!(set_host_path(&store, &workspace.id, &file).is_err());
    assert_eq!(
        get(&store, &workspace.id).unwrap().host_path,
        moved.host_path
    );

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(old_host).ok();
    std::fs::remove_dir_all(new_host).ok();
}

/// Metadata written before Workspaces carried a policy must still read, as
/// the empty policy: those launches ran on their own inputs alone.
#[test]
fn old_metadata_defaults_optional_features_to_off() {
    let store = temp_dir("legacy-store");
    let host = temp_dir("legacy-host");
    let workspace = create(&store, &host, None).unwrap();
    let meta_path = workspace.path.join(WORKSPACE_META_FILE);
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
    json.as_object_mut().unwrap().remove("launch");
    json.as_object_mut().unwrap().remove("deleted");
    std::fs::write(&meta_path, serde_json::to_string(&json).unwrap()).unwrap();

    let reread = get(&store, &workspace.id).unwrap();
    assert!(reread.launch.is_empty());

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
}

#[test]
fn a_git_repository_association_is_canonical_and_durable() {
    let store = temp_dir("git-store");
    let host = temp_dir("git-host");
    let repository = temp_dir("git-repository");
    let git = FakeGit::new();
    git.init(&repository);
    std::fs::create_dir(repository.join("subdir")).unwrap();

    // Associating a subdirectory stores the checkout root, so every later
    // launch mounts the repository and not the fragment that named it.
    let workspace =
        create_with_repository(&git, &store, &host, None, Some(&repository.join("subdir")))
            .unwrap();
    let expected = repository.canonicalize().unwrap();
    assert_eq!(
        workspace.git_repository.as_deref(),
        Some(expected.as_path())
    );
    assert_eq!(
        get(&store, &workspace.id).unwrap().git_repository,
        Some(expected.clone())
    );
    let metadata = std::fs::read_to_string(workspace.path.join(WORKSPACE_META_FILE)).unwrap();
    assert!(metadata.contains("git_repository"));

    let cleared = set_git_repository(&git, &store, &workspace.id, None).unwrap();
    assert_eq!(cleared.git_repository, None);
    let restored = set_git_repository(&git, &store, &workspace.id, Some(&repository)).unwrap();
    assert_eq!(restored.git_repository.as_deref(), Some(expected.as_path()));

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
    std::fs::remove_dir_all(repository).ok();
}

#[test]
fn malformed_git_metadata_is_rejected_before_it_is_stored() {
    let store = temp_dir("invalid-git-store");
    let host = temp_dir("invalid-git-host");
    let repository = temp_dir("invalid-git-repository");
    // A directory with a `.git` that names nothing: the layout reader
    // would call it a checkout, and Git would not. The association has to
    // believe Git, or every launch in this Workspace fails later instead.
    std::fs::write(repository.join(".git"), "not a gitdir pointer\n").unwrap();
    let git = FakeGit::new();
    let workspace = create(&store, &host, None).unwrap();

    let error = set_git_repository(&git, &store, &workspace.id, Some(&repository)).unwrap_err();
    assert!(error.to_string().contains("not inside a Git repository"));
    assert_eq!(get(&store, &workspace.id).unwrap().git_repository, None);

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
    std::fs::remove_dir_all(repository).ok();
}

/// The question an editor asks: "I am in this directory — whose work is
/// it?" A subdirectory has to answer with the Workspace above it, or the
/// caller is left comparing paths itself.
#[test]
fn a_directory_finds_the_innermost_workspace_over_it() {
    let store = temp_dir("for-path-store");
    let host = temp_dir("for-path-host");
    let nested = host.join("crates/inner");
    std::fs::create_dir_all(host.join("src/deep")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    let outer = create(&store, &host, Some("outer".into())).unwrap();

    assert_eq!(for_path(&store, &host).unwrap().unwrap().id, outer.id);
    assert_eq!(
        for_path(&store, &host.join("src/deep"))
            .unwrap()
            .unwrap()
            .id,
        outer.id
    );

    // A Workspace over a subdirectory is not shadowed by the one above it.
    let inner = create(&store, &nested, Some("inner".into())).unwrap();
    assert_eq!(for_path(&store, &nested).unwrap().unwrap().id, inner.id);
    assert_eq!(for_path(&store, &host).unwrap().unwrap().id, outer.id);

    // Two Workspaces over one directory: the most recently accessed wins,
    // the order the pickers show them in.
    let sibling = create(&store, &host, Some("sibling".into())).unwrap();
    assert_eq!(for_path(&store, &host).unwrap().unwrap().id, sibling.id);
    access(&store, &outer.id).unwrap();
    assert_eq!(for_path(&store, &host).unwrap().unwrap().id, outer.id);

    // A directory no Workspace covers is an ordinary empty answer; one
    // that does not exist is the caller's mistake.
    let elsewhere = temp_dir("for-path-elsewhere");
    assert!(for_path(&store, &elsewhere).unwrap().is_none());
    assert!(for_path(&store, &host.join("missing")).is_err());
    assert!(for_path(&store, Path::new("src")).is_err());

    std::fs::remove_dir_all(store).ok();
    std::fs::remove_dir_all(host).ok();
    std::fs::remove_dir_all(elsewhere).ok();
}
