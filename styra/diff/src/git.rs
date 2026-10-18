//! Reading a diff, and what it compares, from Git.

use std::path::Path;
use std::process::Command;

use crate::DiffTarget;

/// The working tree against `base`, so what has not been committed yet is
/// shown alongside what has. Colour and external drivers are turned off: the
/// view styles the diff itself, and needs Git's plain format to.
pub fn diff(worktree: &Path, base: &str) -> Result<String, String> {
    // Compare the complete working tree, including untracked files present
    // in a snapshot, without changing what the operator has staged.
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

/// The branches and tags at `commit`, named the way `git log --decorate`
/// names them: a local branch by its name, a remote one with its remote's,
/// and a tag as `tag: v1.2`. A remote's `HEAD` only repeats one of its
/// branches and is left out. Only ever a decoration, so a repository that
/// cannot say has none rather than an error.
pub fn names_at(worktree: &Path, commit: &str) -> Vec<String> {
    let points_at = format!("--points-at={commit}");
    let refs = git(
        worktree,
        &[
            "for-each-ref",
            &points_at,
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    );
    refs.unwrap_or_default()
        .lines()
        .filter_map(|name| {
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
    let worktree = git(directory, &["rev-parse", "--show-toplevel"])?;
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

    fn git(directory: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn repository(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        root
    }

    /// Committed and uncommitted work both count: the diff is of the working
    /// tree, not of the branch's last commit.
    #[test]
    fn the_diff_covers_work_committed_since_the_start_and_work_not_yet_committed() {
        let root = repository("styra-checkout-diff");
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn moves_are_detected_even_when_disabled_in_repository_config() {
        let root = repository("styra-diff-renames");
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_start_commit_git_does_not_know_is_reported_rather_than_shown_as_empty() {
        let root = repository("styra-checkout-diff-unknown");

        let diff = diff(&root, "0123456789abcdef0123456789abcdef01234567");

        assert!(diff.unwrap_err().starts_with("git diff failed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Given a base, the diff starts where the branch left it, so what the
    /// base gained afterwards is not shown as removed.
    #[test]
    fn a_base_is_measured_from_where_the_branch_left_it() {
        let root = repository("styra-diff-target");
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_trees_include_untracked_work_and_preserve_the_real_index() {
        let root = repository("styra-diff-snapshot-tree");
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
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_snapshot_tree_can_be_diffed_without_any_checkout_commit() {
        let root = repository("styra-diff-snapshot-unborn");
        std::fs::write(root.join("file.txt"), "initial\n").unwrap();
        git(&root, &["add", "."]);
        let base = git(&root, &["write-tree"]);
        git(&root, &["rm", "--cached", "file.txt"]);
        assert!(diff(&root, &base).unwrap().is_empty());
        std::fs::write(root.join("file.txt"), "changed\n").unwrap();
        let shown = diff(&root, &base).unwrap();
        assert!(shown.contains("-initial"), "{shown}");
        assert!(shown.contains("+changed"), "{shown}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Branches, remote branches, and both kinds of tag at a commit are
    /// named; a ref elsewhere, and a remote's `HEAD`, are not.
    #[test]
    fn the_names_at_a_commit_are_its_branches_and_tags() {
        let root = repository("styra-diff-names");
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
        assert!(names_at(&root, "not-a-commit").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
