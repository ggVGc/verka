use super::*;

#[test]
fn change_kinds_cover_metadata_only_and_content_changes() {
    let cases = [
            ("new file mode 100644\n", FileChange::Added),
            ("deleted file mode 100644\n", FileChange::Deleted),
            ("--- /dev/null\n+++ b/file\n", FileChange::Added),
            ("--- a/file\n+++ /dev/null\n", FileChange::Deleted),
            ("@@ -1 +1 @@\n-old\n+new\n", FileChange::Edited),
            ("similarity index 100%\nrename from old\nrename to file\n", FileChange::Moved),
            ("similarity index 80%\nrename from old\nrename to file\n@@ -1 +1 @@\n-old\n+new\n", FileChange::MovedEdited),
            ("similarity index 80%\nrename from old\nrename to file\nBinary files a/old and b/file differ\n", FileChange::MovedEdited),
            ("old mode 100644\nnew mode 100755\nrename from old\nrename to file\n", FileChange::MovedEdited),
            ("similarity index 100%\ncopy from old\ncopy to file\n", FileChange::Copied),
            ("similarity index 80%\ncopy from old\ncopy to file\n", FileChange::CopiedEdited),
            ("old mode 100644\nnew mode 100755\n", FileChange::Mode),
            ("old mode 100644\nnew mode 100755\n@@ -1 +1 @@\n-old\n+new\n", FileChange::Edited),
            ("Binary files a/file and b/file differ\n", FileChange::Edited),
            ("@@ -1 +1 @@\n rename from code\n+new file mode 100644\n", FileChange::Edited),
        ];
    for (metadata, expected) in cases {
        let patch = format!("diff --git a/file b/file\n{metadata}");
        assert_eq!(file_diffs(&patch)[0].change(), expected, "{patch}");
    }
}

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
