//! A multi-file diff split into the patch for each file it changes, which is
//! what the per-file view lists and shows one at a time.

/// A complete patch for one changed file, including metadata for renames,
/// binary changes, deletions, and changes of mode that have no hunks.
pub struct FileDiff<'a> {
    pub path: &'a str,
    pub patch: &'a str,
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
mod tests {
    use super::*;

    #[test]
    fn file_sections_include_deleted_renamed_binary_and_mode_only_changes() {
        let patches = [
            "diff --git a/src/a.rs b/src/a.rs\nindex 1111111..2222222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n+fn d() {}\n".to_owned(),
            "diff --git a/gone.txt b/gone.txt\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n".into(),
            "diff --git a/old name b/a/new name\nsimilarity index 100%\nrename from old name\nrename to a/new name\n".into(),
            "diff --git a/a/image.png b/a/image.png\nBinary files a/a/image.png and b/a/image.png differ\n".into(),
            "diff --git a/script b/script\nold mode 100644\nnew mode 100755\n".into(),
            "diff --git \"a/tab\\tname\" \"b/tab\\tname\"\n--- \"a/tab\\tname\"\n+++ \"b/tab\\tname\"\n@@ -1 +1 @@\n-old\n+new\n".into(),
        ];
        let text = patches.concat();
        let files = file_diffs(&text);
        assert_eq!(
            files.iter().map(|file| file.path).collect::<Vec<_>>(),
            [
                "src/a.rs",
                "gone.txt",
                "a/new name",
                "a/image.png",
                "script",
                "tab\\tname"
            ]
        );
        for (file, patch) in files.iter().zip(&patches) {
            assert_eq!(file.patch, patch);
        }
        assert!(file_diffs("").is_empty());
    }

    #[test]
    fn a_row_is_placed_in_its_file_at_the_new_files_line() {
        let text = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -10,4 +10,4 @@ fn a() {
 keep
-old
+new
 after
\\ No newline at end of file
diff --git a/gone.rs b/gone.rs
--- a/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
-gone
";
        let rows: Vec<_> = (0..text.lines().count() + 1)
            .map(|row| position(text, row))
            .collect();
        assert_eq!(
            rows,
            [
                Some(("a.rs", None)),
                Some(("a.rs", None)),
                Some(("a.rs", None)),
                Some(("a.rs", Some(10))),
                Some(("a.rs", Some(10))),
                Some(("a.rs", Some(11))),
                Some(("a.rs", Some(11))),
                Some(("a.rs", Some(12))),
                Some(("a.rs", Some(12))),
                Some(("gone.rs", None)),
                Some(("gone.rs", None)),
                Some(("gone.rs", None)),
                Some(("gone.rs", Some(1))),
                Some(("gone.rs", Some(1))),
                None,
            ]
        );
    }
}
