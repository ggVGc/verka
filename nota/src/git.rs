//! Every Git operation Nota performs.
//!
//! Review logic asks [`Git`] questions and never builds a `git` process
//! itself. [`SystemGit`] answers them by running `git`; tests can supply an
//! in-memory implementation instead.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The Git operations Nota needs. `repository` is always a root returned by
/// [`Git::repository_root`], and commits are full object ids.
pub trait Git {
    /// The root of the working tree containing `path`.
    fn repository_root(&self, path: &Path) -> Result<PathBuf>;

    /// The full id of the commit `revision` names.
    fn resolve_commit(&self, repository: &Path, revision: &str) -> Result<String>;

    /// The checked-out branch, or `None` when `HEAD` is detached.
    fn current_branch(&self, repository: &Path) -> Result<Option<String>>;

    /// Reject `branch` unless Git accepts it as a branch name.
    fn validate_branch_name(&self, repository: &Path, branch: &str) -> Result<()>;

    fn branch_exists(&self, repository: &Path, branch: &str) -> Result<bool>;

    /// The commit `branch` points at. Unlike [`Git::resolve_commit`], a tag or
    /// other ref with the same name is never chosen instead.
    fn branch_tip(&self, repository: &Path, branch: &str) -> Result<String>;

    /// Local branch names and their captured tip object ids. Excludes remote
    /// tracking refs and tags, even when a short name would be ambiguous.
    fn local_branches(&self, repository: &Path) -> Result<Vec<(String, String)>>;

    /// Create `branch` at `commit` without checking it out. Fails if the
    /// branch already exists.
    fn create_branch(&self, repository: &Path, branch: &str, commit: &str) -> Result<()>;

    /// Create a commit whose only parent is `parent` and whose tree is the
    /// parent's, without changing any ref, the index, or the checkout.
    fn commit_empty(&self, repository: &Path, parent: &str, message: &str) -> Result<String>;

    /// Move existing `branch` from `expected` to `commit`, failing if it no
    /// longer points at `expected`.
    fn update_branch(
        &self,
        repository: &Path,
        branch: &str,
        commit: &str,
        expected: &str,
    ) -> Result<()>;

    /// `revision` and its first-parent ancestors, newest first.
    fn first_parent_history(&self, repository: &Path, revision: &str) -> Result<Vec<String>>;

    /// Commits on the first-parent histories of `tips` whose message has a
    /// line starting with `<key>:` for any of `keys`, in no particular order.
    /// Such a line need not be a trailer, so callers must still parse them.
    fn first_parent_commits_with_trailers(
        &self,
        repository: &Path,
        tips: &[String],
        keys: &[&str],
    ) -> Result<Vec<Commit>>;

    /// Read `commits`, in the order given.
    fn commits(&self, repository: &Path, commits: &[String]) -> Result<Vec<Commit>>;

    /// The text of `path` as `revision` has it, or `None` when it has no such
    /// file.
    fn read_file(&self, repository: &Path, revision: &str, path: &str) -> Result<Option<String>>;

    /// Store `contents` as a blob, converted as Git would convert a file at
    /// `path`, and return its id.
    fn write_blob(&self, repository: &Path, path: &str, contents: &str) -> Result<String>;

    /// The line changes from `from` to `to` in `paths`, without context, for
    /// each file that differs. `to` is a revision, or the working tree for
    /// `None`. Renames are reported as a deletion and an addition.
    fn diff(
        &self,
        repository: &Path,
        from: &str,
        to: Option<&str>,
        paths: &[String],
    ) -> Result<Vec<FileDiff>>;

    /// The line changes from blob `from` to blob `to`, without context. Either
    /// may be any name Git resolves to a blob, such as `<revision>:<path>`.
    fn diff_blobs(&self, repository: &Path, from: &str, to: &str) -> Result<Vec<Hunk>>;
}

/// The changes to one file, as [`Git::diff`] reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// The file's path on the `to` side, or on the `from` side when deleted.
    pub path: String,
    /// In file order; empty when only binary content or the mode changed.
    pub hunks: Vec<Hunk>,
}

/// One change of a line diff without context, numbered as Git numbers it: a
/// side with no lines starts at the line it follows, 0 for the top.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    /// The lines removed and added, without line endings.
    pub old: Vec<String>,
    pub new: Vec<String>,
}

/// A commit as Nota reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    /// `None` for a root commit.
    pub first_parent: Option<String>,
    /// The full message, without surrounding whitespace.
    pub message: String,
    /// The paths a non-merge commit changes relative to its parent.
    pub paths: Vec<String>,
}

/// [`Git`] answered by running the `git` executable.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGit;

impl Git for SystemGit {
    fn repository_root(&self, path: &Path) -> Result<PathBuf> {
        let root = checked(path, &["rev-parse", "--show-toplevel"])
            .with_context(|| format!("{} is not inside a Git repository", path.display()))?;
        Ok(root.into())
    }

    fn resolve_commit(&self, repository: &Path, revision: &str) -> Result<String> {
        checked(
            repository,
            &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
        )
        .with_context(|| format!("resolving Git revision `{revision}`"))
    }

    fn current_branch(&self, repository: &Path) -> Result<Option<String>> {
        let args = ["symbolic-ref", "--quiet", "--short", "HEAD"];
        // `--quiet` makes a detached HEAD exit 1 silently; anything else is
        // a real failure.
        if output(repository, &args)?.status.code() == Some(1) {
            return Ok(None);
        }
        checked(repository, &args).map(Some)
    }

    fn validate_branch_name(&self, repository: &Path, branch: &str) -> Result<()> {
        checked(repository, &["check-ref-format", "--branch", branch]).map(drop)
    }

    fn branch_exists(&self, repository: &Path, branch: &str) -> Result<bool> {
        let refname = format!("refs/heads/{branch}");
        Ok(
            output(repository, &["show-ref", "--verify", "--quiet", &refname])?
                .status
                .success(),
        )
    }

    fn branch_tip(&self, repository: &Path, branch: &str) -> Result<String> {
        self.resolve_commit(repository, &format!("refs/heads/{branch}"))
    }

    fn local_branches(&self, repository: &Path) -> Result<Vec<(String, String)>> {
        let refs = checked(
            repository,
            &[
                "for-each-ref",
                "--format=%(refname)%09%(objectname)",
                "refs/heads/",
            ],
        )?;
        refs.lines()
            .map(|line| {
                let (name, tip) = line.split_once('\t').context("invalid branch listing")?;
                let branch = name
                    .strip_prefix("refs/heads/")
                    .context("invalid local ref")?;
                Ok((branch.to_string(), tip.to_string()))
            })
            .collect()
    }

    fn create_branch(&self, repository: &Path, branch: &str, commit: &str) -> Result<()> {
        let refname = format!("refs/heads/{branch}");
        checked(repository, &["update-ref", &refname, commit, ""]).map(drop)
    }

    fn commit_empty(&self, repository: &Path, parent: &str, message: &str) -> Result<String> {
        let tree = checked(repository, &["rev-parse", &format!("{parent}^{{tree}}")])?;
        checked_with_input(
            repository,
            &["commit-tree", &tree, "-p", parent, "-F", "-"],
            message,
        )
    }

    fn update_branch(
        &self,
        repository: &Path,
        branch: &str,
        commit: &str,
        expected: &str,
    ) -> Result<()> {
        let refname = format!("refs/heads/{branch}");
        checked(repository, &["update-ref", &refname, commit, expected]).map(drop)
    }

    fn first_parent_history(&self, repository: &Path, revision: &str) -> Result<Vec<String>> {
        Ok(lines(&checked(
            repository,
            &["rev-list", "--first-parent", revision],
        )?))
    }

    fn first_parent_commits_with_trailers(
        &self,
        repository: &Path,
        tips: &[String],
        keys: &[&str],
    ) -> Result<Vec<Commit>> {
        let mut args = vec!["--first-parent".to_string()];
        args.extend(keys.iter().map(|key| format!("--grep=^{key}:")));
        log(repository, &args, tips)
    }

    fn commits(&self, repository: &Path, commits: &[String]) -> Result<Vec<Commit>> {
        log(repository, &["--no-walk=unsorted".to_string()], commits)
    }

    fn read_file(&self, repository: &Path, revision: &str, path: &str) -> Result<Option<String>> {
        // A `^{blob}` suffix would be read as part of the path.
        let blob = format!("{revision}:{path}");
        let args = ["rev-parse", "--verify", "--quiet", &blob];
        // `--quiet` makes a missing object exit 1 silently.
        if output(repository, &args)?.status.code() == Some(1) {
            return Ok(None);
        }
        let id = checked(repository, &args)?;
        raw(repository, &["cat-file", "blob", &id]).map(Some)
    }

    fn write_blob(&self, repository: &Path, path: &str, contents: &str) -> Result<String> {
        checked_with_input(
            repository,
            &["hash-object", "-w", "--stdin", &format!("--path={path}")],
            contents,
        )
    }

    fn diff(
        &self,
        repository: &Path,
        from: &str,
        to: Option<&str>,
        paths: &[String],
    ) -> Result<Vec<FileDiff>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec!["--literal-pathspecs"];
        args.extend(DIFF);
        args.push(from);
        args.extend(to);
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
        parse_diff(&raw(repository, &args)?)
    }

    fn diff_blobs(&self, repository: &Path, from: &str, to: &str) -> Result<Vec<Hunk>> {
        let mut args = DIFF.to_vec();
        args.extend([from, to]);
        let files = parse_diff(&raw(repository, &args)?)?;
        Ok(files.into_iter().flat_map(|file| file.hunks).collect())
    }
}

/// `git diff` with output that [`parse_diff`] can read, whatever the user's
/// configuration.
const DIFF: [&str; 15] = [
    "-c",
    "core.quotePath=false",
    "-c",
    "diff.mnemonicPrefix=false",
    "-c",
    "diff.noprefix=false",
    "diff",
    "--unified=0",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-renames",
    "--no-relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// Read the files and hunks of `git diff --unified=0` output.
fn parse_diff(output: &str) -> Result<Vec<FileDiff>> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut old_path = None;
    // Lines still to read of the current hunk's old and new sides; content
    // lines may themselves start with `---` or `+++`.
    let mut pending = (0, 0);
    for line in output.lines() {
        if pending != (0, 0) {
            let hunk = files
                .last_mut()
                .and_then(|file| file.hunks.last_mut())
                .context("invalid `git diff` output: lines outside a hunk")?;
            if let Some(text) = line.strip_prefix('-').filter(|_| pending.0 > 0) {
                hunk.old.push(text.to_string());
                pending.0 -= 1;
            } else if let Some(text) = line.strip_prefix('+').filter(|_| pending.1 > 0) {
                hunk.new.push(text.to_string());
                pending.1 -= 1;
            } else if !line.starts_with('\\') {
                bail!("invalid `git diff` output: unexpected line `{line}`");
            }
            continue;
        }
        if line.starts_with("diff --git ") {
            old_path = None;
        } else if let Some(path) = line.strip_prefix("--- ") {
            old_path = diff_path(path, "a/");
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let path = diff_path(path, "b/")
                .or(old_path.take())
                .context("invalid `git diff` output: a file without a path")?;
            files.push(FileDiff {
                path,
                hunks: Vec::new(),
            });
        } else if let Some(header) = line.strip_prefix("@@ -") {
            let file = files
                .last_mut()
                .context("invalid `git diff` output: a hunk without a file")?;
            let (ranges, _) = header
                .split_once(" @@")
                .context("invalid `git diff` hunk header")?;
            let (old, new) = ranges
                .split_once(" +")
                .context("invalid `git diff` hunk header")?;
            let (old_start, old_count) = hunk_range(old)?;
            let (new_start, new_count) = hunk_range(new)?;
            pending = (old_count, new_count);
            file.hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                old: Vec::new(),
                new: Vec::new(),
            });
        }
    }
    if pending != (0, 0) {
        bail!("invalid `git diff` output: a truncated hunk");
    }
    Ok(files)
}

/// A `---` or `+++` path, or `None` for `/dev/null`. Git quotes unusual
/// paths, and ends paths containing spaces with a tab.
fn diff_path(value: &str, prefix: &str) -> Option<String> {
    let value = value.strip_suffix('\t').unwrap_or(value);
    let value = match value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        Some(quoted) => unquote(quoted),
        None => value.to_string(),
    };
    value.strip_prefix(prefix).map(str::to_string)
}

/// Undo Git's C-style quoting of a path.
fn unquote(quoted: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = quoted.bytes().peekable();
    while let Some(byte) = chars.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'a') => bytes.push(7),
            Some(b'b') => bytes.push(8),
            Some(b'f') => bytes.push(12),
            Some(b'v') => bytes.push(11),
            Some(b'r') => bytes.push(b'\r'),
            Some(digit @ b'0'..=b'7') => {
                let mut value = u32::from(digit - b'0');
                for _ in 0..2 {
                    if let Some(next @ b'0'..=b'7') = chars.peek().copied() {
                        value = value * 8 + u32::from(next - b'0');
                        chars.next();
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => bytes.push(other),
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `start,count` from a hunk header; a missing count is 1.
fn hunk_range(range: &str) -> Result<(usize, usize)> {
    let (start, count) = range.split_once(',').unwrap_or((range, "1"));
    Ok((
        start.parse().context("invalid `git diff` hunk header")?,
        count.parse().context("invalid `git diff` hunk header")?,
    ))
}

/// One `git log` over `revisions`, which are passed on stdin so their number
/// is not limited by the command line.
fn log(repository: &Path, args: &[String], revisions: &[String]) -> Result<Vec<Commit>> {
    // Without revisions, `git log` would read `HEAD`.
    if revisions.is_empty() {
        return Ok(Vec::new());
    }
    // Each record is `RS id NUL parents NUL message NUL`, followed by the
    // changed paths, which `--name-only` prints after the format.
    let mut command = vec![
        "-c",
        "log.showSignature=false",
        "log",
        "--no-renames",
        "--name-only",
        "--format=%x1e%H%x00%P%x00%B%x00",
    ];
    command.extend(args.iter().map(String::as_str));
    command.push("--stdin");
    let output = checked_with_input(repository, &command, &(revisions.join("\n") + "\n"))?;
    output
        .split('\x1e')
        .skip(1)
        .map(|record| {
            let mut fields = record.splitn(4, '\0');
            let (Some(id), Some(parents), Some(message), Some(paths)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                bail!("invalid `git log` output in {}", repository.display());
            };
            Ok(Commit {
                id: id.to_string(),
                first_parent: parents.split_whitespace().next().map(str::to_string),
                message: message.trim().to_string(),
                paths: lines(paths),
            })
        })
        .collect()
}

fn lines(value: &str) -> Vec<String> {
    value
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn output(repository: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "running `git {}` in {}",
                args.join(" "),
                repository.display()
            )
        })
}

fn checked(repository: &Path, args: &[&str]) -> Result<String> {
    Ok(raw(repository, args)?.trim().to_string())
}

/// The output of a successful command, untrimmed.
fn raw(repository: &Path, args: &[&str]) -> Result<String> {
    let result = output(repository, args)?;
    if !result.status.success() {
        bail!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            repository.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}

fn checked_with_input(repository: &Path, args: &[&str], input: &str) -> Result<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "running `git {}` in {}",
                args.join(" "),
                repository.display()
            )
        })?;
    use std::io::Write as _;
    child
        .stdin
        .take()
        .expect("piped git stdin")
        .write_all(input.as_bytes())?;
    let result = child.wait_with_output()?;
    if !result.status.success() {
        bail!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            repository.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_keep_content_that_looks_like_headers() {
        let output = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n\
            @@ -1,2 +1 @@\n--- a\n-+++ b\n\\ No newline at end of file\n+@@ c\n\
            diff --git \"a/sp\\303\\244ce \\\"q\\\"\" \"b/sp\\303\\244ce \\\"q\\\"\"\n\
            deleted file mode 100644\n--- \"a/sp\\303\\244ce \\\"q\\\"\"\n+++ /dev/null\n\
            @@ -3 +2,0 @@\n-gone\n";
        let files = parse_diff(output).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "x");
        assert_eq!(
            files[0].hunks,
            vec![Hunk {
                old_start: 1,
                old_count: 2,
                new_start: 1,
                new_count: 1,
                old: vec!["-- a".into(), "+++ b".into()],
                new: vec!["@@ c".into()],
            }]
        );
        assert_eq!(files[1].path, "späce \"q\"");
        assert_eq!(
            (files[1].hunks[0].old_start, files[1].hunks[0].new_count),
            (3, 0)
        );
    }
}
