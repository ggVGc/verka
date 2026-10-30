//! Reading a diff, and what it compares, from Git.

use std::path::Path;
use std::process::Command;

use crate::DiffTarget;

/// The working tree against `base`, so what has not been committed yet is
/// shown alongside what has. Colour and external drivers are turned off: the
/// view styles the diff itself, and needs Git's plain format to.
pub fn diff(worktree: &Path, base: &str) -> Result<String, String> {
    let (_scratch, index) = scratch_index(worktree)?;
    git_with_index(
        worktree,
        &[
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            base,
            "--",
        ],
        Some(&index),
    )
}

/// An index holding the complete working tree, untracked files included,
/// made from a copy of the real one so what the operator has staged is left
/// alone. It lasts as long as the directory returned with it.
fn scratch_index(worktree: &Path) -> Result<(tempfile::TempDir, std::path::PathBuf), String> {
    let scratch = tempfile::tempdir().map_err(|error| format!("creating diff index: {error}"))?;
    let index = scratch.path().join("index");
    let current = git(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    if Path::new(&current).exists() {
        std::fs::copy(&current, &index).map_err(|error| format!("copying diff index: {error}"))?;
    }
    git_with_index(worktree, &["add", "--all", "--", ":/"], Some(&index))?;
    Ok((scratch, index))
}

/// The working tree as a tree object, as [`diff`] compares it, so it can be
/// diffed like any commit.
pub(crate) fn worktree_tree(worktree: &Path) -> Result<String, String> {
    let (_scratch, index) = scratch_index(worktree)?;
    git_with_index(worktree, &["write-tree"], Some(&index))
}

/// A commit as the layers say who made it, when, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub author: String,
    /// When it was made, as `git log` says it relative to now.
    pub date: String,
    pub summary: String,
    /// The rest of its message after the summary line and the blank line
    /// beneath it.
    pub body: String,
}

/// The commits made on the way from `base` to `HEAD`, oldest first and
/// following first parents. None when `base` is a snapshot tree rather than
/// a commit, or `HEAD` has no commit.
pub(crate) fn commits_since(worktree: &Path, base: &str) -> Vec<Commit> {
    read_commits(worktree, base, true)
}

/// Every commit reachable from HEAD but not from the comparison base.
pub fn commits_between(worktree: &Path, base: &str) -> Vec<Commit> {
    read_commits(worktree, base, false)
}

fn read_commits(worktree: &Path, base: &str, first_parent: bool) -> Vec<Commit> {
    if git(worktree, &["cat-file", "-t", base]).as_deref() != Ok("commit") {
        return Vec::new();
    }
    let range = format!("{base}..HEAD");
    git(
        worktree,
        &[
            "log",
            "--reverse",
            if first_parent {
                "--first-parent"
            } else {
                "--topo-order"
            },
            // Unit separators between fields and record separators between
            // commits, which no message holds, as a body runs over lines.
            "--format=%H%x1f%an%x1f%ar%x1f%s%x1f%b%x1e",
            &range,
        ],
    )
    .unwrap_or_default()
    .split('\x1e')
    .filter_map(|record| {
        let mut fields = record.trim_start_matches('\n').splitn(5, '\x1f');
        let id = fields.next().filter(|id| !id.is_empty())?.to_owned();
        let mut field = || fields.next().unwrap_or_default().trim_end().to_owned();
        Some(Commit {
            id,
            author: field(),
            date: field(),
            summary: field(),
            body: field(),
        })
    })
    .collect()
}

/// What changed from `from` to `to`, commits or trees, with every file's
/// whole text as context, so each changed file's lines can all be followed
/// from one step to the next.
pub(crate) fn step_diff(worktree: &Path, from: &str, to: &str) -> Result<String, String> {
    git(
        worktree,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--unified=2147483647",
            from,
            to,
            "--",
        ],
    )
}

/// A commit's own changes against its first parent, including merge commits.
pub fn commit_diff(worktree: &Path, commit: &str) -> Result<String, String> {
    let parent = format!("{commit}^1");
    git(
        worktree,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            &parent,
            commit,
            "--",
        ],
    )
}

/// The branches and tags at `commit`, named the way `git log --decorate`
/// names them: a local branch by its name, a remote one with its remote's,
/// and a tag as `tag: v1.2`. A remote's `HEAD` only repeats one of its
/// branches and is left out. Only ever a decoration, so a repository that
/// cannot say has none rather than an error. For a snapshot tree, names refer
/// to branch tips and tags with exactly the same tree contents.
pub fn names_at(worktree: &Path, commit: &str) -> Vec<String> {
    // Snapshot baselines are trees, not commits. Match their exact contents
    // against ref tips; a snapshot with uncommitted changes must not inherit
    // the decorations of the commit it was captured from.
    let tree = git(worktree, &["cat-file", "-t", commit]).as_deref() == Ok("tree");
    let points_at = format!("--points-at={commit}");
    let mut args = vec!["for-each-ref"];
    if !tree {
        args.push(&points_at);
    }
    args.extend([
        "--format=%(refname) %(tree) %(*tree)",
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ]);
    let refs = git(worktree, &args);
    refs.unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            if tree && !fields.any(|value| value == commit) {
                return None;
            }
            if let Some(branch) = name.strip_prefix("refs/heads/") {
                Some(branch.to_owned())
            } else if let Some(tag) = name.strip_prefix("refs/tags/") {
                Some(format!("tag: {tag}"))
            } else {
                name.strip_prefix("refs/remotes/")
                    .filter(|remote| !remote.ends_with("/HEAD"))
                    .map(str::to_owned)
            }
        })
        .collect()
}

/// The diff `styra-diff` shows for the repository `directory` is in: its
/// working tree against where its history left `base` — the two's merge base,
/// so that what `base` has gained since is not shown as undone here — or,
/// with no base, against `HEAD`, which leaves what is not committed yet.
pub fn target(directory: &Path, base: Option<&str>) -> Result<DiffTarget, String> {
    let worktree = toplevel(directory)?;
    // Fails only for a detached head, the repository having just answered.
    let branch = git(directory, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let commit = match base {
        Some(base) => git(directory, &["merge-base", base, "HEAD"])?,
        None => git(directory, &["rev-parse", "--verify", "HEAD"])?,
    };
    Ok(DiffTarget {
        worktree: worktree.into(),
        branch,
        base_branch: Some(base.unwrap_or("HEAD").to_owned()),
        base: commit,
        branch_point: base.is_some(),
    })
}

/// The top of the working tree `directory` is in, where a diff's paths start.
pub fn toplevel(directory: &Path) -> Result<String, String> {
    git(directory, &["rev-parse", "--show-toplevel"])
}

/// Run `git` in `directory`, returning what it printed or what went wrong.
/// Only a diff's output is kept verbatim; anything else is one trimmed line.
fn git(directory: &Path, args: &[&str]) -> Result<String, String> {
    git_with_index(directory, args, None)
}

fn git_with_index(directory: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    let command = args[0];
    let mut invocation = Command::new("git");
    if let Some(index) = index {
        invocation.env("GIT_INDEX_FILE", index);
    }
    let output = invocation
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .map_err(|error| format!("could not run git {command}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {command} failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(if command == "diff" {
        stdout.into_owned()
    } else {
        stdout.trim().to_owned()
    })
}

#[cfg(test)]
mod tests {
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

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
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
}
