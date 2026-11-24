use super::*;

/// Every file goes, named and fingerprinted as the view names and
/// fingerprints it, with a long patch cut short on a character.
#[test]
fn each_file_of_the_diff_is_sent_as_the_view_lists_it() {
    let long = format!(
        "diff --git a/long.txt b/long.txt\n@@ -1 +1 @@\n+{}\n",
        "é".repeat(PATCH_BYTES)
    );
    let text = format!("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n{long}");
    let files = request_files(&text, None);

    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "a.txt");
    assert_eq!(
        files[0].patch,
        "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n"
    );
    assert_eq!(
        files[0].fingerprint,
        styra_diff::report::fingerprint(&files[0].patch)
    );
    assert_eq!(files[1].path, "long.txt");
    assert!(files[1].patch.len() <= PATCH_BYTES && files[1].patch.len() > PATCH_BYTES - 2);
    assert_eq!(files[1].fingerprint, styra_diff::report::fingerprint(&long));
    assert!(request_files("", None).is_empty());

    let only = request_files(&text, Some("long.txt"));
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].path, "long.txt");
    assert!(request_files(&text, Some("gone.txt")).is_empty());
}

#[test]
fn a_report_is_shown_by_path() {
    let report = EditReport {
        generated_at_ms: 1,
        base: "4bf5c35d".into(),
        files: vec![styra_protocol::FileEditReport {
            path: "a.rs".into(),
            fingerprint: "f".into(),
            description: "Adds a.".into(),
        }],
        requested: 1,
        failures: vec!["timed out".into()],
    };
    let shown = for_view(&report);
    assert_eq!(shown.files["a.rs"].description, "Adds a.");
    assert_eq!(shown.files["a.rs"].fingerprint, "f");
    assert_eq!(shown.failures, ["timed out"]);
}
