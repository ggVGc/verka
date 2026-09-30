//! Giving Claude's edit snippets a position, so the preview can number them.
//!
//! Claude's Edit, MultiEdit and Write calls carry the text they replace and
//! the text they put in, but not where: genta rebuilds them as hunks headed
//! `@@ edit @@` or `@@ write <path> @@` (see `claude_file_change`), with no
//! line numbers to draw. The file itself still has the new text in it, so
//! finding it there says which lines the edit landed on.
//!
//! Best effort by nature. The file is read as it is now, not as it was just
//! after the edit, so a later change above the snippet moves it, and a later
//! change to the snippet itself loses it; a hunk that cannot be found keeps
//! its bare header and is drawn without numbers. Where the new text occurs
//! more than once, the first occurrence is taken.

/// `diff` with every positionless snippet header replaced by a real hunk
/// header, placed by finding its new text in `file`. `None` when the diff has
/// no such header, so there is nothing to do.
pub fn place(diff: &str, file: &str) -> Option<String> {
    let lines: Vec<&str> = diff.lines().collect();
    if !has_snippets(diff) {
        return None;
    }
    let file: Vec<&str> = file.lines().collect();
    let mut placed = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if !is_snippet_header(line) {
            placed.push((*line).to_owned());
            continue;
        }
        let body: Vec<&str> = lines[index + 1..]
            .iter()
            .take_while(|line| !line.starts_with("@@"))
            .copied()
            .collect();
        let removed = body.iter().filter(|line| line.starts_with('-')).count();
        let added: Vec<&str> = body
            .iter()
            .filter_map(|line| line.strip_prefix('+'))
            .collect();
        let header = if line.starts_with("@@ write ") {
            // A write is the whole file, so it starts at the top.
            Some(format!("@@ -0,0 +1,{} @@", added.len()))
        } else {
            find(&file, &added).map(|at| format!("@@ -{at},{removed} +{at},{} @@", added.len()))
        };
        placed.push(header.unwrap_or_else(|| (*line).to_owned()));
    }
    Some(placed.join("\n"))
}

/// Whether `diff` has a snippet [`place`] would give a position to, so the
/// caller knows whether the file is worth reading.
pub fn has_snippets(diff: &str) -> bool {
    diff.lines().any(is_snippet_header)
}

fn is_snippet_header(line: &str) -> bool {
    line == "@@ edit @@" || (line.starts_with("@@ write ") && line.ends_with(" @@"))
}

/// The one-based line at which `needle` first occurs in `file` as a run of
/// whole lines. An empty needle — a pure deletion — is nowhere in particular.
fn find(file: &[&str], needle: &[&str]) -> Option<usize> {
    if needle.is_empty() || needle.len() > file.len() {
        return None;
    }
    file.windows(needle.len())
        .position(|window| window == needle)
        .map(|index| index + 1)
}

#[cfg(test)]
mod tests {
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
}
