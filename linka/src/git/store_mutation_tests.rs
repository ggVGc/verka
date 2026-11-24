use super::*;
use crate::model::Author;
use crate::ops::{self, NewNode};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn workbench() -> (TempDir, Store, GitVcs) {
    let root = std::env::temp_dir().join(format!(
        "linka-store-mutation-{}-{}",
        std::process::id(),
        ulid::Ulid::new()
    ));
    std::fs::create_dir_all(&root).unwrap();
    checked(&root, &["init"]).unwrap();
    checked(&root, &["config", "user.name", "linka test"]).unwrap();
    checked(&root, &["config", "user.email", "test@linka.invalid"]).unwrap();
    let store = Store::init(root.join(".linka")).unwrap();
    std::fs::write(store.root().join("store.toml"), "schema = 1\n").unwrap();
    checked(&root, &["add", ".linka"]).unwrap();
    checked(&root, &["commit", "-m", "initial store"]).unwrap();
    let vcs = GitVcs::for_store(&store);
    (TempDir(root), store, vcs)
}

fn new_node(description: &str) -> NewNode {
    NewNode {
        description: description.into(),
        author: Author::Human,
        assignee: None,
        depends_on: vec![],
        derived_from: vec![],
    }
}

#[test]
fn clean_store_mutation_is_one_commit_and_finishes_clean() {
    let (_temp, store, vcs) = workbench();
    let before = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();

    let id = ops::add(&store, &vcs, new_node("atomic addition")).unwrap();

    let after = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();
    assert_ne!(before, after);
    vcs.require_clean_store(&store.store_name()).unwrap();
    let changed = checked(
        &store.workbench_root(),
        &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    )
    .unwrap();
    assert_eq!(
        changed.lines().collect::<Vec<_>>(),
        vec![
            format!(".linka/nodes/{id}/description.md"),
            format!(".linka/nodes/{id}/node.toml")
        ]
    );
    assert!(store
        .workbench_root()
        .join(".git/linka-mutation.lock")
        .is_file());
}

#[test]
fn dirty_store_refuses_mutation_before_any_edit() {
    let (_temp, store, vcs) = workbench();
    let id = ops::add(&store, &vcs, new_node("existing")).unwrap();
    let before = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();
    std::fs::write(store.node_dir(&id).join("description.md"), "hand edit").unwrap();

    let error = ops::add(&store, &vcs, new_node("must not be created")).unwrap_err();

    assert!(
        format!("{error:#}").contains("uncommitted store changes"),
        "{error:#}"
    );
    assert_eq!(
        checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap(),
        before
    );
    assert_eq!(store.list_ids().unwrap(), vec![id]);
}

#[test]
fn ignored_store_files_are_included_in_the_mutation_commit() {
    let (_temp, store, vcs) = workbench();
    std::fs::write(
        store.workbench_root().join(".gitignore"),
        "description.md\n",
    )
    .unwrap();
    checked(&store.workbench_root(), &["add", ".gitignore"]).unwrap();
    checked(
        &store.workbench_root(),
        &["commit", "-m", "ignore descriptions"],
    )
    .unwrap();

    let id = ops::add(&store, &vcs, new_node("must be committed")).unwrap();

    let changed = checked(
        &store.workbench_root(),
        &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
    )
    .unwrap();
    assert_eq!(
        changed.lines().collect::<Vec<_>>(),
        vec![
            format!(".linka/nodes/{id}/description.md"),
            format!(".linka/nodes/{id}/node.toml")
        ]
    );
    vcs.require_clean_store(&store.store_name()).unwrap();
}

#[test]
fn preexisting_ignored_store_content_blocks_mutation() {
    let (_temp, store, vcs) = workbench();
    std::fs::write(store.workbench_root().join(".gitignore"), "*.tmp\n").unwrap();
    checked(&store.workbench_root(), &["add", ".gitignore"]).unwrap();
    checked(
        &store.workbench_root(),
        &["commit", "-m", "ignore temporary files"],
    )
    .unwrap();
    std::fs::write(store.root().join("leftover.tmp"), "incomplete mutation\n").unwrap();
    let before = checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap();

    let error = ops::add(&store, &vcs, new_node("must not be created")).unwrap_err();

    assert!(
        format!("{error:#}").contains("!! .linka/leftover.tmp"),
        "{error:#}"
    );
    assert_eq!(
        checked(&store.workbench_root(), &["rev-parse", "HEAD"]).unwrap(),
        before
    );
    assert!(store.list_ids().unwrap().is_empty());
}

#[test]
fn dirty_paths_enumerates_files_in_untracked_directories() {
    let (temp, store, _) = workbench();
    let project = store.project_root();
    ensure_repo(&project).unwrap();
    std::fs::create_dir_all(project.join("test/nested")).unwrap();
    std::fs::write(project.join("test/nested/output.txt"), "produced\n").unwrap();
    let vcs = GitVcs::new(project, temp.0.clone());

    assert_eq!(vcs.dirty_paths().unwrap(), vec!["test/nested/output.txt"]);
}

#[test]
fn concurrent_store_mutation_is_refused_until_lock_release() {
    let (_temp, store, vcs) = workbench();
    let lock = store.mutation_lock(&vcs).unwrap();

    let error = ops::add(&store, &vcs, new_node("contended")).unwrap_err();
    assert!(error.to_string().contains("mutation is in progress"));
    drop(lock);

    ops::add(&store, &vcs, new_node("after release")).unwrap();
}
