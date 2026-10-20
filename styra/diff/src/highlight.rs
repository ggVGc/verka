//! Match surviving turn edits by file, sign, and exact content. Hunk line
//! numbers belong to different baselines and cannot be compared directly.
use std::collections::{HashMap, HashSet};

/// Only unambiguous changed lines are marked. Repeated text is insufficient
/// evidence to attribute a line to a turn, so leave it unmarked.
pub(crate) fn marked_rows(text: &str, turn: &str) -> HashSet<usize> {
    fn changes(text: &str) -> HashMap<(&str, &str), Vec<usize>> {
        let mut changes = HashMap::<_, Vec<_>>::new();
        let mut start = text
            .lines()
            .take_while(|line| !line.starts_with("diff --git "))
            .count();
        for file in crate::files::file_diffs(text) {
            let mut in_hunk = false;
            for (row, line) in file.patch.lines().enumerate() {
                if line.starts_with("@@") {
                    in_hunk = true;
                } else if in_hunk && line.starts_with(['+', '-']) {
                    changes
                        .entry((file.path, line))
                        .or_default()
                        .push(start + row);
                }
            }
            start += file.patch.lines().count();
        }
        changes
    }
    let selected = changes(turn);
    changes(text)
        .into_iter()
        .filter_map(|(key, rows)| {
            (rows.len() == 1 && selected.get(&key).is_some_and(|rows| rows.len() == 1))
                .then(|| rows[0])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_surviving_edits_across_baselines_without_crossing_files_or_signs() {
        let full = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n+other\ndiff --git a/b b/b\n@@ -1 +1 @@\n+new\n";
        let turn = "diff --git a/a b/a\n@@ -80 +90 @@\n-old\n+new\n-other\n+rewritten\n";
        assert_eq!(marked_rows(full, turn), HashSet::from([2, 3]));
        assert!(marked_rows(full, "").is_empty());
    }

    #[test]
    fn repeated_changed_lines_are_ambiguous_and_metadata_is_not_code() {
        let full = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1,2 @@\n-old\n+repeat\n+repeat\n";
        let turn = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -9 +9 @@\n-else\n+repeat\n";
        assert!(marked_rows(full, turn).is_empty());
    }
}
