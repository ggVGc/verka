use super::*;

const FILE: &str = "fn main() {\n    let a = 1;\n    let b = 2;\n}\n";

#[test]
fn an_edit_is_placed_where_its_new_text_is() {
    let diff = "@@ edit @@\n-    let b = 3;\n+    let b = 2;";
    assert_eq!(
        place(diff, FILE).unwrap(),
        "@@ -3,1 +3,1 @@\n-    let b = 3;\n+    let b = 2;"
    );
}

/// A MultiEdit is several snippets, each placed on its own.
#[test]
fn each_snippet_is_placed_separately() {
    let diff = "@@ edit @@\n-fn start() {\n+fn main() {\n@@ edit @@\n-}\n+}";
    assert_eq!(
        place(diff, FILE).unwrap(),
        "@@ -1,1 +1,1 @@\n-fn start() {\n+fn main() {\n@@ -4,1 +4,1 @@\n-}\n+}"
    );
}

#[test]
fn a_write_starts_at_the_top_of_the_file() {
    let diff = "@@ write src/main.rs @@\n+fn main() {\n+}";
    assert_eq!(
        place(diff, "").unwrap(),
        "@@ -0,0 +1,2 @@\n+fn main() {\n+}"
    );
}

/// The file has moved on since: the snippet is drawn, just not numbered.
#[test]
fn a_snippet_no_longer_in_the_file_keeps_its_bare_header() {
    let diff = "@@ edit @@\n-old\n+gone since";
    assert_eq!(place(diff, FILE).unwrap(), diff);
    let deletion = "@@ edit @@\n-    let a = 1;";
    assert_eq!(place(deletion, FILE).unwrap(), deletion);
}

#[test]
fn a_diff_that_already_has_positions_is_left_alone() {
    assert_eq!(place("@@ -1,1 +1,1 @@\n-a\n+b", FILE), None);
}
