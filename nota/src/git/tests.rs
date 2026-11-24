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
