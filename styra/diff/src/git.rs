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

/// The text of the file at `path`, relative to the repository's root, as
/// `commit` left it.
pub(crate) fn file_at(worktree: &Path, commit: &str, path: &str) -> Result<String, String> {
    git(worktree, &["show", &format!("{commit}:{path}")])
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
    let branch = branch(directory);
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

/// The local branches of the repository `directory` is in, newest tip first,
/// to offer as a [`target`]'s base.
pub fn branches(directory: &Path) -> Result<Vec<String>, String> {
    Ok(git(
        directory,
        &[
            "for-each-ref",
            "--sort=refname",
            "--sort=-committerdate",
            "--format=%(refname:strip=2)",
            "refs/heads/",
        ],
    )?
    .lines()
    .map(str::to_owned)
    .collect())
}

/// The branch checked out in the repository `directory` is in. `None` for a
/// detached head, or when `directory` is not in a repository at all.
fn branch(directory: &Path) -> Option<String> {
    git(directory, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
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
    // A diff, or a file's text, is kept to its first and last whitespace.
    Ok(if matches!(command, "diff" | "show") {
        stdout.into_owned()
    } else {
        stdout.trim().to_owned()
    })
}

#[cfg(test)]
mod tests;
