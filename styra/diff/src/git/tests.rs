use super::*;
use crate::test_support::{git, repository};

#[cfg(feature = "renderer")]
#[test]
fn commit_browser_previews_each_patch_and_restores_all_changes() {
    use crate::{CheckoutDiff, Outcome};
    use crossterm::event::{KeyCode, KeyEvent};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("base.txt"), "base\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    let base = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("first.txt"), "first\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "first commit"]);
    let first = git(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("second.txt"), "second\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "second commit"]);
    std::fs::write(root.join("dirty.txt"), "dirty\n").unwrap();
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let mut view = CheckoutDiff::read(DiffTarget {
        worktree: root.into(),
        branch: None,
        base_branch: None,
        base: base.clone(),
        branch_point: false,
    });
    let all = view.diff.clone();
    assert_eq!(view.commits.entries.len(), 2);
    let press = |view: &mut CheckoutDiff, code| view.handle_key(KeyEvent::from(code));
    press(&mut view, KeyCode::Char('h'));
    assert!(view.commits.focused);
    view.involved_layers = Some(vec![0]);
    press(&mut view, KeyCode::Char('j'));
    assert!(view.involved_layers.is_none());
    assert_eq!(view.commits.entries[view.commits.selected - 1].id, first);
    let patch = view.diff.as_deref().unwrap();
    assert!(patch.contains("first.txt"));
    assert!(!patch.contains("second.txt"));
    assert!(!patch.contains("dirty.txt"));
    press(&mut view, KeyCode::Down);
    assert!(view.diff.as_deref().unwrap().contains("second.txt"));
    assert!(!view.diff.as_deref().unwrap().contains("first.txt"));

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| {
            crate::render(
                frame,
                &view.view(ratatui::widgets::Block::default()),
                frame.area(),
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(screen.contains("first commit"));
    assert!(screen.contains("second commit"));
    assert!(screen.contains("All changes"));

    press(&mut view, KeyCode::Enter);
    assert!(!view.commits.focused);
    assert!(view.line_cursor.is_none());
    // The descriptions page keeps its own shortcut and modal navigation.
    press(&mut view, KeyCode::Char('o'));
    assert!(view.report_page.is_some());
    let selected_commit = view.commits.selected;
    press(&mut view, KeyCode::Char('j'));
    assert_eq!(view.commits.selected, selected_commit);
    press(&mut view, KeyCode::Esc);
    assert!(view.report_page.is_none());
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    // Refreshing after another commit retains the selected commit's patch.
    let selected_patch = view.diff.clone();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "third commit"]);
    assert!(view.refresh());
    assert_eq!(view.commits.entries.len(), 3);
    assert_eq!(view.diff, selected_patch);
    assert_eq!(press(&mut view, KeyCode::Esc), Outcome::Handled);
    assert_eq!(view.commits.selected, 0);
    assert_eq!(view.diff, all);
    press(&mut view, KeyCode::Tab);
    press(&mut view, KeyCode::Char('G'));
    assert_eq!(view.commits.selected, 3);
    press(&mut view, KeyCode::Char('h'));
    assert!(!view.commits.open);
    assert_eq!(view.diff, all);
    // Rewriting history drops a selection that no longer belongs to the range.
    press(&mut view, KeyCode::Char('h'));
    press(&mut view, KeyCode::Char('G'));
    git(root, &["reset", "--soft", &base]);
    assert!(view.refresh());
    assert!(view.commits.entries.is_empty());
    assert_eq!(view.commits.selected, 0);
}

#[test]
fn commit_browser_includes_side_branch_commits_and_merge_patch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let base = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-qb", "side"]);
    std::fs::write(root.join("side.txt"), "side\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "side commit"]);
    git(root, &["checkout", "-q", "main"]);
    git(
        root,
        &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
    );
    let commits = commits_between(root, &base);
    assert_eq!(
        commits
            .iter()
            .map(|c| c.summary.as_str())
            .collect::<Vec<_>>(),
        ["side commit", "merge side"]
    );
    assert!(commit_diff(root, &commits[1].id).unwrap().contains("+side"));
    assert_eq!(commits_since(root, &base).len(), 1);
    let tree = git(root, &["rev-parse", "HEAD^{tree}"]);
    assert!(commits_between(root, &tree).is_empty());
}

/// Committed and uncommitted work both count: the diff is of the working
/// tree, not of the branch's last commit.
#[test]
fn the_diff_covers_work_committed_since_the_start_and_work_not_yet_committed() {
    let (_directory, root) = repository("styra-checkout-diff");
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "start"]);
    let start = git(&root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("a.txt"), "one\ncommitted\n").unwrap();
    git(&root, &["commit", "-q", "-am", "more"]);
    std::fs::write(root.join("a.txt"), "one\ncommitted\nloose\n").unwrap();

    let diff = diff(&root, &start).unwrap();

    assert!(diff.contains("+committed"), "{diff}");
    assert!(diff.contains("+loose"), "{diff}");
}

#[test]
fn moves_are_detected_even_when_disabled_in_repository_config() {
    let (_directory, root) = repository("styra-diff-renames");
    std::fs::write(root.join("old.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "start"]);
    git(&root, &["config", "diff.renames", "false"]);
    git(&root, &["mv", "old.txt", "new.txt"]);
    let patch = diff(&root, "HEAD").unwrap();
    let files = crate::files::file_diffs(&patch);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "new.txt");
    assert_eq!(files[0].change(), crate::files::FileChange::Moved);

    std::fs::write(root.join("new.txt"), "one\ntwo\nthree\nfour\nedited\n").unwrap();
    let patch = diff(&root, "HEAD").unwrap();
    let files = crate::files::file_diffs(&patch);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].change(), crate::files::FileChange::MovedEdited);
}

#[test]
fn a_start_commit_git_does_not_know_is_reported_rather_than_shown_as_empty() {
    let (_directory, root) = repository("styra-checkout-diff-unknown");

    let diff = diff(&root, "0123456789abcdef0123456789abcdef01234567");

    assert!(diff.unwrap_err().starts_with("git diff failed"));
}

/// Given a base, the diff starts where the branch left it, so what the
/// base gained afterwards is not shown as removed.
#[test]
fn a_base_is_measured_from_where_the_branch_left_it() {
    let (_directory, root) = repository("styra-diff-target");
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "start"]);
    let start = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("a.txt"), "one\nfeature\n").unwrap();
    git(&root, &["commit", "-q", "-am", "feature"]);
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("b.txt"), "main moved on\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "main"]);
    git(&root, &["checkout", "-q", "feature"]);
    let toplevel = git(&root, &["rev-parse", "--show-toplevel"]);

    let since_main = target(&root.join("."), Some("main")).unwrap();
    assert_eq!(
        since_main,
        DiffTarget {
            worktree: toplevel.into(),
            branch: Some("feature".into()),
            base_branch: Some("main".into()),
            base: start,
            branch_point: true,
        }
    );
    let shown = diff(&since_main.worktree, &since_main.base).unwrap();
    assert!(shown.contains("+feature"), "{shown}");
    assert!(!shown.contains("main moved on"), "{shown}");

    let head = target(&root, None).unwrap();
    assert_eq!(head.base, git(&root, &["rev-parse", "HEAD"]));
    assert_eq!(head.base_branch.as_deref(), Some("HEAD"));
    assert!(!head.branch_point);

    assert!(target(&root, Some("no-such-branch"))
        .unwrap_err()
        .starts_with("git merge-base failed"));
}

#[test]
fn snapshot_trees_include_untracked_work_and_preserve_the_real_index() {
    let (_directory, root) = repository("styra-diff-snapshot-tree");
    std::fs::write(root.join("tracked.txt"), "committed\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "start"]);
    std::fs::write(root.join("tracked.txt"), "initial dirty\n").unwrap();
    std::fs::write(root.join("untracked.txt"), "initial untracked\n").unwrap();
    git(&root, &["add", "."]);
    let base = git(&root, &["write-tree"]);
    git(&root, &["reset", "--mixed", "HEAD"]);
    let index = std::fs::read(root.join(".git/index")).unwrap();

    assert!(diff(&root, &base).unwrap().is_empty());
    std::fs::write(root.join("untracked.txt"), "changed\n").unwrap();
    std::fs::write(root.join("added.txt"), "new\n").unwrap();
    let shown = diff(&root, &base).unwrap();
    assert!(shown.contains("-initial untracked"), "{shown}");
    assert!(shown.contains("+changed"), "{shown}");
    assert!(shown.contains("+new"), "{shown}");
    assert!(!shown.contains("tracked.txt b/tracked.txt"), "{shown}");
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
}

#[test]
fn a_snapshot_tree_can_be_diffed_without_any_checkout_commit() {
    let (_directory, root) = repository("styra-diff-snapshot-unborn");
    std::fs::write(root.join("file.txt"), "initial\n").unwrap();
    git(&root, &["add", "."]);
    let base = git(&root, &["write-tree"]);
    git(&root, &["rm", "--cached", "file.txt"]);
    assert!(diff(&root, &base).unwrap().is_empty());
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    let shown = diff(&root, &base).unwrap();
    assert!(shown.contains("-initial"), "{shown}");
    assert!(shown.contains("+changed"), "{shown}");
}

/// Branches, remote branches, and both kinds of tag at a commit are
/// named; a ref elsewhere, and a remote's `HEAD`, are not.
#[test]
fn the_names_at_a_commit_are_its_branches_and_tags() {
    let (_directory, root) = repository("styra-diff-names");
    git(&root, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let start = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["tag", "v1"]);
    git(&root, &["tag", "-a", "-m", "annotated", "v1-annotated"]);
    git(&root, &["branch", "release"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        &root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    git(&root, &["commit", "-q", "--allow-empty", "-m", "later"]);
    git(&root, &["tag", "v2"]);

    assert_eq!(
        names_at(&root, &start),
        ["release", "origin/main", "tag: v1", "tag: v1-annotated"]
    );
    assert_eq!(names_at(&root, "HEAD"), ["main", "tag: v2"]);
    let tree = git(&root, &["rev-parse", "HEAD^{tree}"]);
    // Both commits are empty, so both generations have the same tree.
    assert_eq!(
        names_at(&root, &tree),
        [
            "main",
            "release",
            "origin/main",
            "tag: v1",
            "tag: v1-annotated",
            "tag: v2"
        ]
    );
    std::fs::write(root.join("dirty.txt"), "snapshot-only work\n").unwrap();
    git(&root, &["add", "."]);
    let dirty_tree = git(&root, &["write-tree"]);
    assert!(names_at(&root, &dirty_tree).is_empty());
    assert!(names_at(&root, "not-a-commit").is_empty());
}
