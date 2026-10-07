//! Carry line numbers between versions of a file.
//!
//! Review items are numbered against the version of a file they were made
//! for: a suggestion's hunks against its parent and itself, a note against
//! the commit in its [`NoteSource`]. A diff from that version to another
//! moves each line by the changes before it, and tells whether the lines
//! themselves survived.

use crate::git::{FileDiff, Git, Hunk};
use crate::review::{NoteSource, Review, ReviewEntry, ReviewEntryKind};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// The version of the files to place a review's items in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The files in the working tree, with any uncommitted edits.
    Worktree,
    /// A Git revision.
    Revision(String),
}

/// A review with each entry's items placed in a [`Target`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlacedReview {
    pub branch: String,
    pub marker: String,
    pub subject: String,
    /// `worktree`, or the full id of the target commit.
    pub at: String,
    pub entries: Vec<PlacedEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlacedEntry {
    #[serde(flatten)]
    pub entry: ReviewEntry,
    /// A note's lines, or each hunk of a suggestion in file order; empty for
    /// a general note.
    pub locations: Vec<Location>,
}

/// Where one item is in the target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Location {
    pub path: String,
    /// The first line the item covers or, when `count` is 0, the line it
    /// follows (0 for the top). `None` when it cannot be placed.
    pub line: Option<usize>,
    pub count: usize,
    pub status: Status,
    /// A suggestion's change, numbered against its parent and itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hunk: Option<Hunk>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// A note whose lines the target still has.
    Current,
    /// A note whose lines the target changed; placed where they were.
    Changed,
    /// A hunk whose original lines the target has, as in a checkout that
    /// has not taken the suggestion.
    Pending,
    /// A hunk whose suggested lines the target has, as in a checkout of the
    /// review branch.
    Applied,
    /// A hunk whose lines the target has neither way; placed where its
    /// original lines were.
    Stale,
    /// Not placed: a change without lines, such as to a binary file, or lines
    /// of a commit that is no longer in the repository.
    Unknown,
}

/// Place every note and suggestion hunk of `review` in `target`. Each item
/// is carried from the version it was made for by a diff from that version
/// to the target, so it follows lines added or removed before it.
pub fn place(git: &dyn Git, path: &Path, review: Review, target: &Target) -> Result<PlacedReview> {
    let repository = git.repository_root(path)?;
    let to = match target {
        Target::Worktree => None,
        Target::Revision(revision) => Some(git.resolve_commit(&repository, revision)?),
    };
    // Each suggestion's own changes, and the versions every item needs
    // carrying from, so that each version is diffed against the target once.
    let mut changes = Vec::new();
    let mut needed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in &review.entries {
        let files = match entry.kind {
            ReviewEntryKind::Suggestion => {
                let parent = format!("{}^", entry.commit);
                let mut hunks: HashMap<_, _> = git
                    .diff(&repository, &parent, Some(&entry.commit), &entry.paths)?
                    .into_iter()
                    .map(|file| (file.path, file.hunks))
                    .collect();
                // Git reports no lines for a binary change.
                let files = entry
                    .paths
                    .iter()
                    .map(|path| FileDiff {
                        path: path.clone(),
                        hunks: hunks.remove(path).unwrap_or_default(),
                    })
                    .collect::<Vec<_>>();
                for file in &files {
                    for revision in [&parent, &entry.commit] {
                        needed
                            .entry(revision.clone())
                            .or_default()
                            .insert(file.path.clone());
                    }
                }
                files
            }
            ReviewEntryKind::Note => {
                if let Some(source) = &entry.source {
                    needed
                        .entry(source.revision.clone())
                        .or_default()
                        .insert(source.path.clone());
                }
                Vec::new()
            }
        };
        changes.push(files);
    }
    // `None` for a version Git cannot diff, such as a pruned commit.
    let mut diffs: HashMap<String, Option<HashMap<String, Vec<Hunk>>>> = HashMap::new();
    for (revision, paths) in needed {
        let paths = paths.into_iter().collect::<Vec<_>>();
        let diff = git.diff(&repository, &revision, to.as_deref(), &paths).ok();
        let files = diff.map(|files| {
            let mut by_path: HashMap<_, _> = paths
                .iter()
                .map(|path| (path.clone(), Vec::new()))
                .collect();
            by_path.extend(files.into_iter().map(|file| (file.path, file.hunks)));
            by_path
        });
        diffs.insert(revision, files);
    }
    let hunks = |revision: &str, path: &str| {
        diffs
            .get(revision)
            .and_then(Option::as_ref)
            .and_then(|files| files.get(path))
    };

    let mut entries = Vec::new();
    for (entry, files) in review.entries.into_iter().zip(changes) {
        let mut locations = Vec::new();
        if let Some(source) = &entry.source {
            locations.push(match hunks(&source.revision, &source.path) {
                Some(hunks) => place_note(hunks, source),
                None => unknown(&source.path),
            });
        }
        let parent = format!("{}^", entry.commit);
        for file in files {
            if file.hunks.is_empty() {
                locations.push(unknown(&file.path));
            }
            for change in file.hunks {
                let to_parent = hunks(&parent, &file.path);
                let to_commit = hunks(&entry.commit, &file.path);
                locations.push(match (to_parent, to_commit) {
                    (Some(to_parent), Some(to_commit)) => {
                        place_hunk(to_parent, to_commit, &file.path, change)
                    }
                    _ => unknown(&file.path),
                });
            }
        }
        entries.push(PlacedEntry { entry, locations });
    }
    Ok(PlacedReview {
        branch: review.branch,
        marker: review.marker,
        subject: review.subject,
        at: to.unwrap_or_else(|| "worktree".to_string()),
        entries,
    })
}

fn unknown(path: &str) -> Location {
    Location {
        path: path.to_string(),
        line: None,
        count: 0,
        status: Status::Unknown,
        hunk: None,
    }
}

/// A note's lines, carried by `hunks` from its source to the target.
fn place_note(hunks: &[Hunk], source: &NoteSource) -> Location {
    let count = source.last - source.first + 1;
    let first = map_line(hunks, source.first, Side::Old).max(1);
    let last = map_line(hunks, source.last, Side::Old).max(first);
    Location {
        path: source.path.clone(),
        line: Some(first),
        count: last - first + 1,
        status: if intact(hunks, (source.first, count)) {
            Status::Current
        } else {
            Status::Changed
        },
        hunk: None,
    }
}

/// A suggestion hunk, given the diffs from its parent and from itself to the
/// target. It is pending where the target keeps its original lines, applied
/// where the target keeps its suggested ones, and stale otherwise.
fn place_hunk(to_parent: &[Hunk], to_commit: &[Hunk], path: &str, hunk: Hunk) -> Location {
    let old = (hunk.old_start, hunk.old_count);
    let new = (hunk.new_start, hunk.new_count);
    let at = |hunks: &[Hunk], (start, count): (usize, usize)| {
        if count == 0 {
            map_gap(hunks, start, Side::Old)
        } else {
            map_line(hunks, start, Side::Old)
        }
    };
    let (status, line, count) = if intact(to_parent, old) {
        (Status::Pending, at(to_parent, old), old.1)
    } else if intact(to_commit, new) {
        (Status::Applied, at(to_commit, new), new.1)
    } else {
        (Status::Stale, at(to_parent, old), old.1)
    };
    Location {
        path: path.to_string(),
        line: Some(line),
        count,
        status,
        hunk: Some(hunk),
    }
}

/// The source of a note about lines `first..=last` of `path` as `contents`
/// has it, or as the working tree has it for `None`. The lines are recorded
/// against `revision`; lines that exist only in `contents` are recorded as
/// the nearest lines `revision` has.
pub fn note_source(
    git: &dyn Git,
    path: &Path,
    revision: &str,
    file: &str,
    lines: (usize, usize),
    contents: Option<&str>,
) -> Result<NoteSource> {
    let (first, last) = lines;
    if first == 0 || last < first {
        bail!("invalid line range {first}-{last}");
    }
    let repository = git.repository_root(path)?;
    let revision = git.resolve_commit(&repository, revision)?;
    let Some(text) = git.read_file(&repository, &revision, file)? else {
        bail!("`{file}` is not in {revision}; a note's lines must be in a commit");
    };
    let contents = match contents {
        Some(contents) => contents.to_string(),
        None => std::fs::read_to_string(repository.join(file))
            .with_context(|| format!("reading `{file}` in {}", repository.display()))?,
    };
    let blob = git.write_blob(&repository, file, &contents)?;
    let hunks = git.diff_blobs(&repository, &format!("{revision}:{file}"), &blob)?;
    let length = text.lines().count().max(1);
    let back = |line| map_line(&hunks, line, Side::New).clamp(1, length);
    Ok(NoteSource {
        revision,
        path: file.to_string(),
        first: back(first),
        last: back(last),
    })
}

/// Which side of a diff a line number counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Old,
    New,
}

/// A hunk's `(start, count)` on `side`, and on the other side.
fn sides(hunk: &Hunk, side: Side) -> ((usize, usize), (usize, usize)) {
    let old = (hunk.old_start, hunk.old_count);
    let new = (hunk.new_start, hunk.new_count);
    match side {
        Side::Old => (old, new),
        Side::New => (new, old),
    }
}

/// The last line a range covers, or the line it follows when empty.
fn end((start, count): (usize, usize)) -> usize {
    if count == 0 {
        start
    } else {
        start + count - 1
    }
}

/// Where `line` of `side` is on the other side. A line the diff changes
/// keeps its place among the hunk's lines on the other side, or goes to the
/// line before them when the other side has none.
pub(crate) fn map_line(hunks: &[Hunk], line: usize, side: Side) -> usize {
    let mut shift: isize = 0;
    for hunk in hunks {
        let (from, to) = sides(hunk, side);
        if from.1 > 0 && from.0 <= line && line <= end(from) {
            return if to.1 == 0 {
                to.0
            } else {
                to.0 + (line - from.0).min(to.1 - 1)
            };
        }
        if end(from) < line {
            shift += to.1 as isize - from.1 as isize;
        }
    }
    line.saturating_add_signed(shift)
}

/// Where the gap after `line` of `side` is on the other side, as the line it
/// follows. Lines inserted right at the gap end up before it.
pub(crate) fn map_gap(hunks: &[Hunk], line: usize, side: Side) -> usize {
    let mut shift: isize = 0;
    for hunk in hunks {
        let (from, to) = sides(hunk, side);
        if end(from) <= line {
            shift += to.1 as isize - from.1 as isize;
        }
    }
    line.saturating_add_signed(shift)
}

/// Whether the diff leaves `(start, count)` of the old side alone: none of
/// its lines change and nothing is inserted between them. An empty range is
/// the gap after `start`, intact when nothing is inserted there and the lines
/// on both sides of it stay.
pub(crate) fn intact(hunks: &[Hunk], (start, count): (usize, usize)) -> bool {
    hunks.iter().all(|hunk| {
        let changed = (hunk.old_start, hunk.old_count);
        match (count, changed.1) {
            (0, 0) => changed.0 != start,
            (0, _) => {
                let covers = |line| changed.0 <= line && line <= end(changed);
                !covers(start) && !covers(start + 1)
            }
            (_, 0) => !(start <= changed.0 && changed.0 < start + count - 1),
            _ => end(changed) < start || start + count - 1 < changed.0,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(old: (usize, usize), new: (usize, usize)) -> Hunk {
        Hunk {
            old_start: old.0,
            old_count: old.1,
            new_start: new.0,
            new_count: new.1,
            old: Vec::new(),
            new: Vec::new(),
        }
    }

    #[test]
    fn lines_move_by_the_changes_before_them() {
        // Two lines inserted after line 1, line 5 replaced by three lines.
        let hunks = [hunk((1, 0), (2, 2)), hunk((5, 1), (7, 3))];
        assert_eq!(map_line(&hunks, 1, Side::Old), 1);
        assert_eq!(map_line(&hunks, 2, Side::Old), 4);
        assert_eq!(map_line(&hunks, 5, Side::Old), 7);
        assert_eq!(map_line(&hunks, 6, Side::Old), 10);
        // Back again; inserted lines go to the line they follow.
        assert_eq!(map_line(&hunks, 3, Side::New), 1);
        assert_eq!(map_line(&hunks, 4, Side::New), 2);
        assert_eq!(map_line(&hunks, 9, Side::New), 5);
        assert_eq!(map_line(&hunks, 10, Side::New), 6);
    }

    #[test]
    fn deleted_lines_go_to_the_line_before_them() {
        let hunks = [hunk((3, 2), (2, 0))];
        assert_eq!(map_line(&hunks, 4, Side::Old), 2);
        assert_eq!(map_line(&hunks, 5, Side::Old), 3);
        assert_eq!(map_gap(&hunks, 2, Side::Old), 2);
        assert_eq!(map_gap(&hunks, 4, Side::Old), 2);
        assert_eq!(map_gap(&hunks, 5, Side::Old), 3);
    }

    #[test]
    fn ranges_are_intact_unless_changed_or_split() {
        let hunks = [hunk((2, 1), (2, 1)), hunk((5, 0), (6, 1))];
        assert!(intact(&hunks, (1, 1)));
        assert!(!intact(&hunks, (1, 2)));
        assert!(intact(&hunks, (3, 3)));
        assert!(!intact(&hunks, (5, 2)));
        assert!(intact(&hunks, (6, 2)));
        // Gaps: nothing inserted there, neighbours unchanged.
        assert!(!intact(&hunks, (5, 0)));
        assert!(!intact(&hunks, (1, 0)));
        assert!(!intact(&hunks, (2, 0)));
        assert!(intact(&hunks, (3, 0)));
        assert!(intact(&[], (0, 0)));
    }
}
