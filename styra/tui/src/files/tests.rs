use super::*;

fn sandbox() -> PathBuf {
    SandboxLayout::default().workspace
}

#[test]
fn paths_resolve_from_the_sandbox_workspace_or_host() {
    for (reported, expected) in [
        (
            sandbox().join("src/main.rs"),
            "/home/me/project/src/main.rs",
        ),
        (PathBuf::from("src/main.rs"), "/home/me/project/src/main.rs"),
        (PathBuf::from("/etc/hosts"), "/etc/hosts"),
    ] {
        assert_eq!(
            resolve(Path::new("/home/me/project"), reported.to_str().unwrap()),
            PathBuf::from(expected),
            "{}",
            reported.display()
        );
    }
}

/// A directory tree for the search to walk: `root/sub/deeper`, with a
/// file at each level so that which one is found says where it stopped.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("styra-locate-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    std::fs::write(root.join("top.md"), "").unwrap();
    std::fs::write(root.join("sub/middle.md"), "").unwrap();
    std::fs::write(root.join("sub/deeper/here.md"), "").unwrap();
    std::fs::write(root.join("sub/deeper/top.md"), "").unwrap();
    root
}

#[test]
fn a_relative_link_is_found_where_the_interaction_is_working_first() {
    let root = project("first");
    let start = root.join("sub/deeper");

    assert_eq!(
        locate(&start, Some(&root), None, "here.md"),
        Ok(start.join("here.md"))
    );
    // Present both here and at the top: the nearer one wins.
    assert_eq!(
        locate(&start, Some(&root), None, "top.md"),
        Ok(start.join("top.md"))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_relative_link_is_looked_for_up_to_the_workspace() {
    let root = project("up");
    let start = root.join("sub/deeper");

    assert_eq!(
        locate(&start, Some(&root), None, "middle.md"),
        Ok(root.join("sub/middle.md"))
    );
    assert_eq!(
        locate(&start, Some(&root), None, "sub/middle.md"),
        Ok(root.join("sub/middle.md"))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_search_stops_at_the_workspace() {
    let outer = project("stops");
    let root = outer.join("sub");
    let start = root.join("deeper");

    // `top.md` exists above the Workspace, and is not its file to name.
    let error = locate(&root, Some(&root), None, "top.md").unwrap_err();
    assert!(error.contains("not found"), "{error}");
    assert!(locate(&start, Some(&root), None, "missing.md").is_err());
    let _ = std::fs::remove_dir_all(&outer);
}

#[test]
fn a_relative_link_is_also_looked_for_from_the_top_of_the_repository() {
    let repository = project("repository");
    let workspace = repository.join("sub");
    let start = workspace.join("deeper");

    // `top.md` is above the Workspace, but at the top of its checkout.
    assert_eq!(
        locate(&workspace, Some(&workspace), Some(&repository), "top.md"),
        Ok(repository.join("top.md"))
    );
    // Still nearest first: the working directory's own wins.
    assert_eq!(
        locate(&start, Some(&workspace), Some(&repository), "top.md"),
        Ok(start.join("top.md"))
    );
    let error = locate(&start, Some(&workspace), Some(&repository), "gone.md").unwrap_err();
    assert!(
        error.contains(&format!("nor in {}", repository.display())),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&repository);
}

#[test]
fn a_checkout_is_found_by_its_git_directory_or_worktree_file() {
    let root = project("checkout");
    assert_eq!(enclosing_checkout(&root.join("sub")), None);
    std::fs::create_dir(root.join(".git")).unwrap();
    assert_eq!(
        enclosing_checkout(&root.join("sub/deeper")),
        Some(root.clone())
    );
    std::fs::write(root.join("sub/.git"), "gitdir: elsewhere").unwrap();
    assert_eq!(
        enclosing_checkout(&root.join("sub/deeper")),
        Some(root.join("sub"))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_absolute_link_is_only_re_rooted() {
    let root = project("absolute");

    assert_eq!(
        locate(
            &root,
            Some(&root),
            None,
            sandbox().join("top.md").to_str().unwrap()
        ),
        Ok(root.join("top.md"))
    );
    assert!(locate(&root, Some(&root), None, "/no/such/file.md").is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn workspace_files_are_grouped_under_it_and_others_under_their_own_parent() {
    let root = Path::new("/home/me/project");
    let items = items(
        root,
        vec![
            "src/main.rs".into(),
            "/etc/hosts".into(),
            "README.md".into(),
        ],
    );

    let grouped: Vec<_> = items
        .iter()
        .map(|item| (item.root.clone(), item.relative.clone()))
        .collect();
    assert_eq!(
        grouped,
        vec![
            (PathBuf::from("/etc"), PathBuf::from("hosts")),
            (root.to_path_buf(), PathBuf::from("README.md")),
            (root.to_path_buf(), PathBuf::from("src/main.rs")),
        ],
        "external roots sort before the Workspace, and each root's files by path"
    );
}

/// The renderer draws this order and `e` indexes into it, so a row and the
/// file it opens are the same thing by construction.
#[test]
fn the_reported_spelling_survives_grouping() {
    let items = items(Path::new("/work"), vec!["src/main.rs".into()]);

    assert_eq!(items[0].reported, "src/main.rs");
    assert_eq!(items[0].resolved, PathBuf::from("/work/src/main.rs"));
}

#[test]
fn punctuation_around_a_path_in_prose_is_stripped() {
    assert_eq!(path_like("`src/main.rs`"), Some("src/main.rs"));
    assert_eq!(path_like("(src/main.rs)"), Some("src/main.rs"));
    assert_eq!(path_like("src/main.rs,"), Some("src/main.rs"));
}

/// A markdown link names a path twice — once as its text, once as its
/// destination — and the destination arrives from the parser with no
/// syntax left around it.
#[test]
fn both_paths_in_a_markdown_link_are_candidates() {
    let found = candidates("At [monitor.c:484](/home/me/src/monitor.c:484), the");

    assert_eq!(found, ["/home/me/src/monitor.c:484", "monitor.c:484"]);
}

/// Code spans and inline emphasis are markup, not part of the path the
/// span holds.
#[test]
fn a_path_in_a_code_span_is_a_candidate_without_its_backticks() {
    assert_eq!(candidates("touched `src/main.rs` today"), ["src/main.rs"]);
    assert_eq!(candidates("touched **src/main.rs**"), ["src/main.rs"]);
}

#[test]
fn a_word_that_could_not_be_a_path_is_not_one() {
    assert_eq!(path_like("running"), None);
    assert_eq!(path_like("``"), None);
    // A trailing full stop is not trimmed: it is as likely to be part of
    // a filename as it is to be the end of a sentence, so the `is_file`
    // check settles it rather than a guess here.
    assert_eq!(path_like("word."), Some("word."));
}

#[test]
fn the_selection_holds_at_the_last_row_and_resets_when_the_scope_changes() {
    let mut view = FilesView::default();
    view.select_next(2);
    view.select_next(2);
    view.select_next(2);
    assert_eq!(view.selected_index(), 2);

    view.toggle_scope();

    assert!(view.shows_all());
    assert_eq!(view.selected_index(), 0, "a different list, from its start");
}
