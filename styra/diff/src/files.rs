//! A multi-file diff split into the patch for each file it changes, which is
//! what the per-file view lists and shows one at a time.

/// A complete patch for one changed file, including metadata for renames,
/// binary changes, deletions, and changes of mode that have no hunks.
pub struct FileDiff<'a> {
    pub path: &'a str,
    pub patch: &'a str,
}

/// The change described by a file patch's metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Deleted,
    Edited,
    Moved,
    MovedEdited,
    Copied,
    CopiedEdited,
    Mode,
}

impl FileChange {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Edited => "M",
            Self::Moved => "R",
            Self::MovedEdited => "R+",
            Self::Copied => "C",
            Self::CopiedEdited => "C+",
            Self::Mode => "T",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Edited => "edited",
            Self::Moved => "moved",
            Self::MovedEdited => "moved with edits",
            Self::Copied => "copied",
            Self::CopiedEdited => "copied with edits",
            Self::Mode => "file mode changed",
        }
    }
}

impl FileDiff<'_> {
    /// The mode Git gives the file when the patch adds it, such as `100755`
    /// for an executable.
    pub fn new_mode(&self) -> Option<&str> {
        self.patch
            .lines()
            .take_while(|line| !line.starts_with("@@"))
            .find_map(|line| line.strip_prefix("new file mode "))
    }

    pub fn change(&self) -> FileChange {
        // Inspect headers only: code in a hunk can resemble Git metadata.
        let headers: Vec<_> = self
            .patch
            .lines()
            .take_while(|line| !line.starts_with("@@"))
            .collect();
        let has = |prefix: &str| headers.iter().any(|line| line.starts_with(prefix));
        if has("new file mode ") || has("--- /dev/null") {
            return FileChange::Added;
        }
        if has("deleted file mode ") || has("+++ /dev/null") {
            return FileChange::Deleted;
        }
        let content_changed = self.patch.lines().any(|line| line.starts_with("@@"))
            || has("Binary files ")
            || has("GIT binary patch")
            || headers.iter().any(|line| {
                line.starts_with("similarity index ") && *line != "similarity index 100%"
            });
        let mode_changed = has("old mode ") || has("new mode ");
        match (
            has("rename from "),
            has("copy from "),
            content_changed || mode_changed,
        ) {
            (true, _, true) => FileChange::MovedEdited,
            (true, _, false) => FileChange::Moved,
            (_, true, true) => FileChange::CopiedEdited,
            (_, true, false) => FileChange::Copied,
            _ if mode_changed && !content_changed => FileChange::Mode,
            _ => FileChange::Edited,
        }
    }
}

/// Split only on Git's file headers, retaining each patch verbatim.
pub fn file_diffs(text: &str) -> Vec<FileDiff<'_>> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).copied().unwrap_or(text.len());
            let patch = &text[start..end];
            let header = patch.lines().next().unwrap_or_default();
            // The destination marker handles spaces in paths. A deleted file
            // uses its source; renames without hunks use their destination.
            let marker_path = |marker: &str| {
                patch
                    .lines()
                    .take_while(|line| !line.starts_with("@@"))
                    .find_map(|line| {
                        line.strip_prefix(marker)
                            .filter(|path| *path != "/dev/null")
                    })
                    .map(|path| {
                        let path = path.trim_matches('"');
                        path.strip_prefix("b/")
                            .or_else(|| path.strip_prefix("a/"))
                            .unwrap_or(path)
                    })
            };
            let path = marker_path("+++ ")
                .or_else(|| {
                    patch
                        .lines()
                        .find_map(|line| line.strip_prefix("rename to "))
                })
                .or_else(|| marker_path("--- "))
                .unwrap_or_else(|| {
                    header
                        .rsplit_once(" b/")
                        .or_else(|| header.rsplit_once(" \"b/"))
                        .map_or(header, |(_, path)| path.trim_end_matches('"'))
                });
            FileDiff { path, patch }
        })
        .collect()
}

/// Where row `row` of `text` is in the working tree, the view drawing one
/// row for each line of a diff: the file whose patch it is in, and — from its
/// first hunk on — the line of the new file it is at. A removed line is at
/// the line now where it was. `None` for a row before any file's patch.
pub fn position(text: &str, row: usize) -> Option<(&str, Option<u32>)> {
    let mut start = text
        .lines()
        .take_while(|line| !line.starts_with("diff --git "))
        .count();
    for file in file_diffs(text) {
        let rows = file.patch.lines().count();
        if row < start + rows {
            return Some((file.path, line_in(file.patch, row - start)));
        }
        start += rows;
    }
    None
}

/// The new file's line at `row` of one file's `patch`; `None` before its
/// first hunk, among the headers that describe the file.
fn line_in(patch: &str, row: usize) -> Option<u32> {
    // The new file's next line, once a hunk has said where it starts.
    let mut next: Option<u32> = None;
    for (index, line) in patch.lines().enumerate() {
        let at = if line.starts_with("@@") {
            next = hunk_start(line);
            next
        } else {
            next.map(|number| match line.as_bytes().first() {
                Some(b'-') => number,
                // `\ No newline at end of file`, about the line before.
                Some(b'\\') => number.saturating_sub(1),
                _ => {
                    next = Some(number + 1);
                    number
                }
            })
        };
        if index == row {
            // A hunk that only removes from the top starts at line zero.
            return at.map(|number| number.max(1));
        }
    }
    None
}

/// Where the new file's side of a hunk (`@@ -12,3 +14,4 @@`) starts.
fn hunk_start(header: &str) -> Option<u32> {
    let new = header
        .split_whitespace()
        .find_map(|part| part.strip_prefix('+'))?;
    new.split(',').next()?.parse().ok()
}

/// The diff row containing a working-tree line, including unchanged context.
/// Lines outside the patch's hunks have no corresponding row.
pub fn row_at_line(patch: &str, wanted: u32) -> Option<usize> {
    let mut next = None;
    for (row, line) in patch.lines().enumerate() {
        if line.starts_with("@@") {
            next = hunk_start(line);
        } else if let Some(number) = next {
            if matches!(line.as_bytes().first(), Some(b' ' | b'+')) {
                if number == wanted {
                    return Some(row);
                }
                next = Some(number.saturating_add(1));
            }
        }
    }
    None
}

/// File-name filtering starts with the first character and ignores case.
pub fn filtered_file_diffs<'a>(text: &'a str, query: Option<&str>) -> Vec<(usize, FileDiff<'a>)> {
    let query = query.unwrap_or_default().to_lowercase();
    let mut files: Vec<_> = file_diffs(text)
        .into_iter()
        .enumerate()
        .filter(|(_, file)| file.path.to_lowercase().contains(&query))
        .collect();
    // Compare components so a directory's descendants stay together even
    // when a sibling filename shares its prefix (src/a.rs and src/a/b.rs).
    files.sort_by(|(_, a), (_, b)| a.path.split('/').cmp(b.path.split('/')));
    files
}

#[cfg(test)]
mod tests;
