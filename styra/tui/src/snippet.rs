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
mod tests;
