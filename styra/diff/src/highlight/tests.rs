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
